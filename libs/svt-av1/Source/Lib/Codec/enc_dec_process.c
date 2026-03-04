/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdlib.h>

#include "svt_psnr.h"

#include "enc_handle.h"
#include "enc_dec_tasks.h"
#include "enc_dec_results.h"
#include "coding_loop.h"
#include "EbSvtAv1ErrorCodes.h"
#include "utility.h"
//To fix warning C4013: 'svt_convert_16bit_to_8bit' undefined; assuming extern returning int
#include "common_dsp_rtcd.h"
#include "rd_cost.h"
#include "pd_process.h"
#include "firstpass.h"
#include "pic_analysis_process.h"
#include "resize.h"
#include "enc_mode_config.h"
#include "rc_process.h"

#include "pack_unpack_c.h"
#include "deblocking_filter.h"

static void copy_mv_rate(PictureControlSet* pcs, MdRateEstimationContext* dst_rate) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;

    memcpy(dst_rate->nmv_vec_cost, pcs->md_rate_est_ctx->nmv_vec_cost, MV_JOINTS * sizeof(int32_t));

    if (frm_hdr->allow_high_precision_mv) {
        memcpy(dst_rate->nmv_costs_hp, pcs->md_rate_est_ctx->nmv_costs_hp, 2 * MV_VALS * sizeof(int32_t));
    } else {
        memcpy(dst_rate->nmv_costs, pcs->md_rate_est_ctx->nmv_costs, 2 * MV_VALS * sizeof(int32_t));
    }

    dst_rate->nmvcoststack[0] = frm_hdr->allow_high_precision_mv ? &dst_rate->nmv_costs_hp[0][MV_MAX]
                                                                 : &dst_rate->nmv_costs[0][MV_MAX];
    dst_rate->nmvcoststack[1] = frm_hdr->allow_high_precision_mv ? &dst_rate->nmv_costs_hp[1][MV_MAX]
                                                                 : &dst_rate->nmv_costs[1][MV_MAX];

    if (frm_hdr->allow_intrabc) {
        memcpy(dst_rate->dv_cost, pcs->md_rate_est_ctx->dv_cost, 2 * MV_VALS * sizeof(int32_t));
        memcpy(dst_rate->dv_joint_cost, pcs->md_rate_est_ctx->dv_joint_cost, MV_JOINTS * sizeof(int32_t));
    }
}

static void enc_dec_context_dctor(EbPtr p) {
    EbThreadContext* thread_ctx = (EbThreadContext*)p;
    EncDecContext*   obj        = (EncDecContext*)thread_ctx->priv;
    EB_DELETE(obj->md_ctx);
    EB_DELETE(obj->input_sample16bit_buffer);
    EB_FREE_ARRAY(obj);
}

/******************************************************
 * Enc Dec Context Constructor
 ******************************************************/
EbErrorType svt_aom_enc_dec_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index,
                                         int tasks_index) {
    SequenceControlSet*             scs                      = enc_handle_ptr->scs_instance->scs;
    const EbSvtAv1EncConfiguration* static_config            = &scs->static_config;
    EbColorFormat                   color_format             = static_config->encoder_color_format;
    int8_t                          enable_hbd_mode_decision = scs->enable_hbd_mode_decision;

    EncDecContext* ed_ctx;
    EB_CALLOC_ARRAY(ed_ctx, 1);
    thread_ctx->priv  = ed_ctx;
    thread_ctx->dctor = enc_dec_context_dctor;

    ed_ctx->is_16bit = scs->is_16bit_pipeline;

    // Input/Output System Resource Manager FIFOs
    ed_ctx->mode_decision_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->enc_dec_tasks_resource_ptr, index);
    ed_ctx->enc_dec_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->enc_dec_results_resource_ptr, index);
    ed_ctx->enc_dec_feedback_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->enc_dec_tasks_resource_ptr, tasks_index);

    // Prediction Buffer
    ed_ctx->input_sample16bit_buffer = NULL;
    if (ed_ctx->is_16bit) {
        EB_NEW(ed_ctx->input_sample16bit_buffer,
               svt_picture_buffer_desc_ctor,
               &(EbPictureBufferDescInitData){
                   .buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK,
                   .max_width          = scs->super_block_size,
                   .max_height         = scs->super_block_size,
                   .bit_depth          = EB_SIXTEEN_BIT,
                   .left_padding       = 0,
                   .right_padding      = 0,
                   .top_padding        = 0,
                   .bot_padding        = 0,
                   .split_mode         = false,
                   .color_format       = color_format,
               });
    }
    // Mode Decision Context
    EB_NEW(ed_ctx->md_ctx,
           svt_aom_mode_decision_context_ctor,
           scs,
           color_format,
           scs->super_block_size,
           static_config->enc_mode,
           scs->max_block_cnt,
           static_config->encoder_bit_depth,
           0,
           0,
           enable_hbd_mode_decision == DEFAULT ? 2 : enable_hbd_mode_decision,
           scs->seq_qp_mod);

    if (enable_hbd_mode_decision) {
        ed_ctx->md_ctx->input_sample16bit_buffer = ed_ctx->input_sample16bit_buffer;
    }

    ed_ctx->md_ctx->ed_ctx = ed_ctx;

    return EB_ErrorNone;
}

/**************************************************
 * Reset Segmentation Map
 *************************************************/
static void reset_segmentation_map(SegmentationNeighborMap* segmentation_map) {
    if (segmentation_map->data != NULL) {
        EB_MEMSET(segmentation_map->data, ~0, segmentation_map->map_size);
    }
}

/**************************************************
 * Reset Mode Decision Neighbor Arrays
 *************************************************/
static void reset_encode_pass_neighbor_arrays(PictureControlSet* pcs, uint16_t tile_idx) {
    svt_aom_neighbor_array_unit_reset(pcs->ep_luma_recon_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_cb_recon_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_cr_recon_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_luma_dc_sign_level_coeff_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_cb_dc_sign_level_coeff_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_cr_dc_sign_level_coeff_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_luma_dc_sign_level_coeff_na_update[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_cb_dc_sign_level_coeff_na_update[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_cr_dc_sign_level_coeff_na_update[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_partition_context_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->ep_txfm_context_na[tile_idx]);
    // TODO(Joel): 8-bit ep_luma_recon_na (Cb,Cr) when is_16bit==0?
    if (pcs->ppcs->scs->is_16bit_pipeline) {
        svt_aom_neighbor_array_unit_reset(pcs->ep_luma_recon_na_16bit[tile_idx]);
        svt_aom_neighbor_array_unit_reset(pcs->ep_cb_recon_na_16bit[tile_idx]);
        svt_aom_neighbor_array_unit_reset(pcs->ep_cr_recon_na_16bit[tile_idx]);
    }
    return;
}

/**************************************************
 * Reset Coding Loop
 **************************************************/
static void reset_enc_dec(EncDecContext* ed_ctx, PictureControlSet* pcs, SequenceControlSet* scs,
                          uint32_t segment_index) {
    ed_ctx->is_16bit        = scs->is_16bit_pipeline;
    ed_ctx->bit_depth       = scs->static_config.encoder_bit_depth;
    uint16_t tile_group_idx = ed_ctx->tile_group_index;
    svt_aom_lambda_assign(pcs,
                          &ed_ctx->pic_fast_lambda[EB_8_BIT_MD],
                          &ed_ctx->pic_full_lambda[EB_8_BIT_MD],
                          EB_EIGHT_BIT,
                          pcs->ppcs->frm_hdr.quantization_params.base_q_idx,
                          true);

    svt_aom_lambda_assign(pcs,
                          &ed_ctx->pic_fast_lambda[EB_10_BIT_MD],
                          &ed_ctx->pic_full_lambda[EB_10_BIT_MD],
                          EB_TEN_BIT,
                          pcs->ppcs->frm_hdr.quantization_params.base_q_idx,
                          true);
    if (segment_index == 0) {
        if (ed_ctx->tile_group_index == 0) {
            reset_segmentation_map(pcs->segmentation_neighbor_map);
        }

        for (uint16_t r = pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_start_y;
             r < pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_end_y;
             r++) {
            for (uint16_t c = pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_start_x;
                 c < pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_end_x;
                 c++) {
                uint16_t tile_idx = c + r * pcs->ppcs->av1_cm->tiles_info.tile_cols;
                reset_encode_pass_neighbor_arrays(pcs, tile_idx);
            }
        }
    }

    return;
}

/******************************************************
 * Update MD Segments
 *
 * This function is responsible for synchronizing the
 *   processing of MD Segment-rows.
 *   In short, the function starts processing
 *   of MD segment-rows as soon as their inputs are available
 *   and the previous segment-row has completed.  At
 *   any given time, only one segment row per picture
 *   is being processed.
 *
 * The function has two functions:
 *
 * (1) Update the Segment Completion Mask which tracks
 *   which MD Segment inputs are available.
 *
 * (2) Increment the segment-row counter (current_row_idx)
 *   as the segment-rows are completed.
 *
 * Since there is the potentential for thread collusion,
 *   a MUTEX a used to protect the sensitive data and
 *   the execution flow is separated into two paths
 *
 * (A) Initial update.
 *  -Update the Completion Mask [see (1) above]
 *  -If the picture is not currently being processed,
 *     check to see if the next segment-row is available
 *     and start processing.
 * (b) Continued processing
 *  -Upon the completion of a segment-row, check
 *     to see if the next segment-row's inputs have
 *     become available and begin processing if so.
 *
 * On last important point is that the thread-safe
 *   code section is kept minimally short. The MUTEX
 *   should NOT be locked for the entire processing
 *   of the segment-row (b) as this would block other
 *   threads from performing an update (A).
 ******************************************************/
static bool assign_enc_dec_segments(EncDecSegments* segmentPtr, uint16_t* segmentInOutIndex, EncDecTasks* taskPtr,
                                    EbFifo* srmFifoPtr) {
    bool     continue_processing_flag = false;
    uint32_t row_segment_index        = 0;
    uint32_t segment_index;
    uint32_t right_segment_index;
    uint32_t bottom_left_segment_index;

    int16_t feedback_row_index = -1;

    uint32_t self_assigned = false;

    switch (taskPtr->input_type) {
    case ENCDEC_TASKS_MDC_INPUT:

        // The entire picture is provided by the MDC process, so
        // no logic is necessary to clear input dependencies.
        // Reset enc_dec segments
        for (uint32_t row_index = 0; row_index < segmentPtr->segment_row_count; ++row_index) {
            segmentPtr->row_array[row_index].current_seg_index = segmentPtr->row_array[row_index].starting_seg_index;
        }

        // Start on Segment 0 immediately
        *segmentInOutIndex  = segmentPtr->row_array[0].current_seg_index;
        taskPtr->input_type = ENCDEC_TASKS_CONTINUE;
        ++segmentPtr->row_array[0].current_seg_index;
        continue_processing_flag = true;
        break;

    case ENCDEC_TASKS_ENCDEC_INPUT:
        // Start on the assigned row immediately
        *segmentInOutIndex  = segmentPtr->row_array[taskPtr->enc_dec_segment_row].current_seg_index;
        taskPtr->input_type = ENCDEC_TASKS_CONTINUE;
        ++segmentPtr->row_array[taskPtr->enc_dec_segment_row].current_seg_index;
        continue_processing_flag = true;
        break;

    case ENCDEC_TASKS_CONTINUE:
        // Update the Dependency List for Right and Bottom Neighbors
        segment_index     = *segmentInOutIndex;
        row_segment_index = segment_index / segmentPtr->segment_band_count;

        right_segment_index       = segment_index + 1;
        bottom_left_segment_index = segment_index + segmentPtr->segment_band_count;

        // Right Neighbor
        if (segment_index < segmentPtr->row_array[row_segment_index].ending_seg_index) {
            svt_block_on_mutex(segmentPtr->row_array[row_segment_index].assignment_mutex);

            --segmentPtr->dep_map.dependency_map[right_segment_index];

            if (segmentPtr->dep_map.dependency_map[right_segment_index] == 0) {
                *segmentInOutIndex = segmentPtr->row_array[row_segment_index].current_seg_index;
                ++segmentPtr->row_array[row_segment_index].current_seg_index;
                self_assigned            = true;
                continue_processing_flag = true;
            }

            svt_release_mutex(segmentPtr->row_array[row_segment_index].assignment_mutex);
        }

        // Bottom-left Neighbor
        if (row_segment_index < segmentPtr->segment_row_count - 1 &&
            bottom_left_segment_index >= segmentPtr->row_array[row_segment_index + 1].starting_seg_index) {
            svt_block_on_mutex(segmentPtr->row_array[row_segment_index + 1].assignment_mutex);

            --segmentPtr->dep_map.dependency_map[bottom_left_segment_index];

            if (segmentPtr->dep_map.dependency_map[bottom_left_segment_index] == 0) {
                if (self_assigned == true) {
                    feedback_row_index = (int16_t)row_segment_index + 1;
                } else {
                    *segmentInOutIndex = segmentPtr->row_array[row_segment_index + 1].current_seg_index;
                    ++segmentPtr->row_array[row_segment_index + 1].current_seg_index;
                    continue_processing_flag = true;
                }
            }
            svt_release_mutex(segmentPtr->row_array[row_segment_index + 1].assignment_mutex);
        }

        if (feedback_row_index > 0) {
            EbObjectWrapper* wrapper_ptr;
            svt_get_empty_object(srmFifoPtr, &wrapper_ptr);
            EncDecTasks* feedback_task         = (EncDecTasks*)wrapper_ptr->object_ptr;
            feedback_task->input_type          = ENCDEC_TASKS_ENCDEC_INPUT;
            feedback_task->enc_dec_segment_row = feedback_row_index;
            feedback_task->pcs_wrapper         = taskPtr->pcs_wrapper;
            feedback_task->tile_group_index    = taskPtr->tile_group_index;
            svt_post_full_object(wrapper_ptr);
        }

        break;

    default:
        break;
    }

    return continue_processing_flag;
}

#if CONFIG_ENABLE_FILM_GRAIN
static void svt_av1_add_film_grain(EbPictureBufferDesc* src, EbPictureBufferDesc* dst, AomFilmGrain* film_grain_ptr) {
    uint8_t *luma, *cb, *cr;
    int32_t  height, width, luma_stride, chroma_stride;
    int32_t  use_high_bit_depth = 0;
    int32_t  chroma_subsamp_x   = 0;
    int32_t  chroma_subsamp_y   = 0;

    AomFilmGrain params = *film_grain_ptr;

    switch (src->bit_depth) {
    case EB_EIGHT_BIT:
        params.bit_depth   = 8;
        use_high_bit_depth = 0;
        chroma_subsamp_x   = 1;
        chroma_subsamp_y   = 1;
        break;
    case EB_TEN_BIT:
        params.bit_depth   = 10;
        use_high_bit_depth = 1;
        chroma_subsamp_x   = 1;
        chroma_subsamp_y   = 1;
        break;
    default: //todo: Throw an error if unknown format?
        params.bit_depth   = 10;
        use_high_bit_depth = 1;
        chroma_subsamp_x   = 1;
        chroma_subsamp_y   = 1;
    }

    dst->max_width  = src->max_width;
    dst->max_height = src->max_height;

    svt_aom_fgn_copy_rect(src->buffer_y + ((src->org_y * src->stride_y + src->org_x) << use_high_bit_depth),
                          src->stride_y,
                          dst->buffer_y + ((dst->org_y * dst->stride_y + dst->org_x) << use_high_bit_depth),
                          dst->stride_y,
                          dst->width,
                          dst->height,
                          use_high_bit_depth);

    const int32_t chroma_width  = (dst->width + chroma_subsamp_x) >> chroma_subsamp_x;
    const int32_t chroma_height = (dst->height + chroma_subsamp_y) >> chroma_subsamp_y;

    svt_aom_fgn_copy_rect(src->buffer_cb +
                              ((src->stride_cb * (src->org_y >> chroma_subsamp_y) + (src->org_x >> chroma_subsamp_x))
                               << use_high_bit_depth),
                          src->stride_cb,
                          dst->buffer_cb +
                              ((dst->stride_cb * (dst->org_y >> chroma_subsamp_y) + (dst->org_x >> chroma_subsamp_x))
                               << use_high_bit_depth),
                          dst->stride_cb,
                          chroma_width,
                          chroma_height,
                          use_high_bit_depth);

    svt_aom_fgn_copy_rect(src->buffer_cr +
                              ((src->stride_cr * (src->org_y >> chroma_subsamp_y) + (src->org_x >> chroma_subsamp_x))
                               << use_high_bit_depth),
                          src->stride_cr,
                          dst->buffer_cr +
                              ((dst->stride_cr * (dst->org_y >> chroma_subsamp_y) + (dst->org_x >> chroma_subsamp_x))
                               << use_high_bit_depth),
                          dst->stride_cr,
                          chroma_width,
                          chroma_height,
                          use_high_bit_depth);

    luma = dst->buffer_y + ((dst->org_y * dst->stride_y + dst->org_x) << use_high_bit_depth);
    cb   = dst->buffer_cb +
        ((dst->stride_cb * (dst->org_y >> chroma_subsamp_y) + (dst->org_x >> chroma_subsamp_x)) << use_high_bit_depth);
    cr = dst->buffer_cr +
        ((dst->stride_cr * (dst->org_y >> chroma_subsamp_y) + (dst->org_x >> chroma_subsamp_x)) << use_high_bit_depth);

    luma_stride   = dst->stride_y;
    chroma_stride = dst->stride_cb;

    width  = dst->width;
    height = dst->height;

    svt_av1_add_film_grain_run(&params,
                               luma,
                               cb,
                               cr,
                               height,
                               width,
                               luma_stride,
                               chroma_stride,
                               use_high_bit_depth,
                               chroma_subsamp_y,
                               chroma_subsamp_x);
    return;
}
#endif

void svt_aom_recon_output(PictureControlSet* pcs, SequenceControlSet* scs) {
    EncodeContext* enc_ctx = scs->enc_ctx;
    // The totalNumberOfReconFrames counter has to be write/read protected as
    //   it is used to determine the end of the stream.  If it is not protected
    //   the encoder might not properly terminate.
    svt_block_on_mutex(enc_ctx->total_number_of_recon_frame_mutex);

    if (!pcs->ppcs->is_alt_ref) {
        bool             is_16bit = (scs->static_config.encoder_bit_depth > EB_EIGHT_BIT);
        EbObjectWrapper* output_recon_wrapper_ptr;
        // Get Recon Buffer
        svt_get_empty_object(scs->enc_ctx->recon_output_fifo_ptr, &output_recon_wrapper_ptr);
        EbBufferHeaderType* output_recon_ptr = (EbBufferHeaderType*)output_recon_wrapper_ptr->object_ptr;
        output_recon_ptr->flags              = 0;

        // START READ/WRITE PROTECTED SECTION
        if (enc_ctx->total_number_of_recon_frames == enc_ctx->terminating_picture_number) {
            output_recon_ptr->flags = EB_BUFFERFLAG_EOS;
        }

        enc_ctx->total_number_of_recon_frames++;

        // STOP READ/WRITE PROTECTED SECTION
        output_recon_ptr->n_filled_len = 0;

        // Copy the Reconstructed Picture to the Output Recon Buffer
        {
            uint32_t sample_total_count;
            uint8_t* recon_read_ptr;
            uint8_t* recon_write_ptr;

            EbPictureBufferDesc* recon_ptr;
            EbPictureBufferDesc* intermediate_buffer_ptr = NULL;
            svt_aom_get_recon_pic(pcs, &recon_ptr, is_16bit);

            const uint32_t color_format = recon_ptr->color_format;
            const uint16_t ss_x         = (color_format == EB_YUV444 ? 0 : 1);
            const uint16_t ss_y         = (color_format >= EB_YUV422 ? 0 : 1);
#if CONFIG_ENABLE_FILM_GRAIN
            // FGN: Create a buffer if needed, copy the reconstructed picture and run the film grain synthesis algorithm
            if (scs->seq_header.film_grain_params_present && pcs->ppcs->frm_hdr.film_grain_params.apply_grain) {
                AomFilmGrain* film_grain_ptr;

                uint16_t                    padding = scs->super_block_size + 32;
                EbPictureBufferDescInitData temp_recon_desc_init_data;
                temp_recon_desc_init_data.max_width          = (uint16_t)scs->max_input_luma_width;
                temp_recon_desc_init_data.max_height         = (uint16_t)scs->max_input_luma_height;
                temp_recon_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;

                temp_recon_desc_init_data.left_padding  = padding;
                temp_recon_desc_init_data.right_padding = padding;
                temp_recon_desc_init_data.top_padding   = padding;
                temp_recon_desc_init_data.bot_padding   = padding;
                temp_recon_desc_init_data.split_mode    = false;
                temp_recon_desc_init_data.color_format  = scs->static_config.encoder_color_format;

                if (is_16bit) {
                    temp_recon_desc_init_data.bit_depth = EB_SIXTEEN_BIT;
                } else {
                    temp_recon_desc_init_data.bit_depth = EB_EIGHT_BIT;
                }

                EB_NO_THROW_NEW(
                    intermediate_buffer_ptr, svt_recon_picture_buffer_desc_ctor, (EbPtr)&temp_recon_desc_init_data);

                if (pcs->ppcs->is_ref == true) {
                    film_grain_ptr = &((EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr)->film_grain_params;
                } else {
                    film_grain_ptr = &pcs->ppcs->frm_hdr.film_grain_params;
                }

                if (intermediate_buffer_ptr) {
                    svt_av1_add_film_grain(recon_ptr, intermediate_buffer_ptr, film_grain_ptr);
                    recon_ptr = intermediate_buffer_ptr;
                }
            }
            // End running the film grain
#endif

            // set output recon frame size to original size when enable resize feature
            // easy to display in tool and analysis
            uint16_t recon_w = recon_ptr->width;
            uint16_t recon_h = recon_ptr->height;
            if (scs->static_config.resize_mode != RESIZE_NONE) {
                recon_w = recon_ptr->max_width; //ALIGN_POWER_OF_TWO(recon_ptr->width, 3);
                recon_h = recon_ptr->max_height; //ALIGN_POWER_OF_TWO(recon_ptr->height, 3);
            }
            // Keep the recon at full resolution and show the lower resolution video on the top right part
            // Y Recon Samples
            sample_total_count = ((pcs->scs->max_initial_input_luma_width - scs->max_initial_input_pad_right) *
                                  (pcs->scs->max_initial_input_luma_height - scs->max_initial_input_pad_bottom))
                << is_16bit;
            recon_read_ptr = recon_ptr->buffer_y + (recon_ptr->org_y << is_16bit) * recon_ptr->stride_y +
                (recon_ptr->org_x << is_16bit);
            recon_write_ptr = &(output_recon_ptr->p_buffer[output_recon_ptr->n_filled_len]);
            // Reset the Luma buffer for the case on changing the resolution on the fly
            memset(recon_write_ptr, 0, sample_total_count);
            CHECK_REPORT_ERROR((output_recon_ptr->n_filled_len + sample_total_count <= output_recon_ptr->n_alloc_len),
                               enc_ctx->app_callback_ptr,
                               EB_ENC_ROB_OF_ERROR);

            // Initialize Y recon buffer
            svt_aom_picture_copy_kernel(
                recon_read_ptr,
                recon_ptr->stride_y,
                recon_write_ptr,
                pcs->scs->max_initial_input_luma_width - scs->pad_right, // use the full res stride
                recon_w - scs->pad_right,
                recon_h - scs->pad_bottom,
                1 << is_16bit);

            output_recon_ptr->n_filled_len += sample_total_count;

            // U Recon Samples
            // Keep the recon at full resolution and show the lower resolution video on the top right part
            sample_total_count =
                (((pcs->scs->max_initial_input_luma_width + ss_x - scs->max_initial_input_pad_right) >> ss_x) *
                 ((pcs->scs->max_initial_input_luma_height + ss_y - scs->max_initial_input_pad_bottom) >> ss_y))
                << is_16bit;
            recon_read_ptr = recon_ptr->buffer_cb + ((recon_ptr->org_y << is_16bit) >> ss_y) * recon_ptr->stride_cb +
                ((recon_ptr->org_x << is_16bit) >> ss_x);
            recon_write_ptr = &(output_recon_ptr->p_buffer[output_recon_ptr->n_filled_len]);

            // Reset the Chroma buffer for the case on changing the resolution on the fly
            memset(recon_write_ptr, 0, sample_total_count);

            CHECK_REPORT_ERROR((output_recon_ptr->n_filled_len + sample_total_count <= output_recon_ptr->n_alloc_len),
                               enc_ctx->app_callback_ptr,
                               EB_ENC_ROB_OF_ERROR);

            // Initialize U recon buffer
            svt_aom_picture_copy_kernel(recon_read_ptr,
                                        recon_ptr->stride_cb,
                                        recon_write_ptr,
                                        (pcs->scs->max_initial_input_luma_width + ss_x - scs->pad_right) >> ss_x,
                                        (recon_w + ss_x - scs->pad_right) >> ss_x,
                                        (recon_h + ss_y - scs->pad_bottom) >> ss_y,
                                        1 << is_16bit);
            output_recon_ptr->n_filled_len += sample_total_count;

            // V Recon Samples
            sample_total_count =
                (((pcs->scs->max_initial_input_luma_width + ss_x - scs->max_initial_input_pad_right) >> ss_x) *
                 ((pcs->scs->max_initial_input_luma_height + ss_y - scs->max_initial_input_pad_bottom) >> ss_y))
                << is_16bit;
            recon_read_ptr = recon_ptr->buffer_cr + ((recon_ptr->org_y << is_16bit) >> ss_y) * recon_ptr->stride_cr +
                ((recon_ptr->org_x << is_16bit) >> ss_x);
            recon_write_ptr = &(output_recon_ptr->p_buffer[output_recon_ptr->n_filled_len]);
            // Reset the Chroma buffer for the case on changing the resolution on the fly
            memset(recon_write_ptr, 0, sample_total_count);
            CHECK_REPORT_ERROR((output_recon_ptr->n_filled_len + sample_total_count <= output_recon_ptr->n_alloc_len),
                               enc_ctx->app_callback_ptr,
                               EB_ENC_ROB_OF_ERROR);

            // Initialize V recon buffer
            svt_aom_picture_copy_kernel(recon_read_ptr,
                                        recon_ptr->stride_cr,
                                        recon_write_ptr,
                                        (pcs->scs->max_initial_input_luma_width + ss_x - scs->pad_right) >> ss_x,
                                        (recon_w + ss_x - scs->pad_right) >> ss_x,
                                        (recon_h + ss_y - scs->pad_bottom) >> ss_y,
                                        1 << is_16bit);
            output_recon_ptr->n_filled_len += sample_total_count;
            output_recon_ptr->pts = pcs->picture_number;

            // add metadata of resized frame size to app for rendering
            if (pcs->ppcs->frame_resize_enabled) {
                SvtMetadataFrameSizeT frame_size = {0};
                frame_size.width                 = recon_w;
                frame_size.height                = recon_h;
                frame_size.disp_width            = recon_ptr->width;
                frame_size.disp_height           = recon_ptr->height;
                frame_size.stride                = recon_w;
                frame_size.subsampling_x         = ss_x;
                frame_size.subsampling_y         = ss_y;
                svt_add_metadata(
                    output_recon_ptr, EB_AV1_METADATA_TYPE_FRAME_SIZE, (uint8_t*)&frame_size, sizeof(frame_size));
            }

            if (intermediate_buffer_ptr) {
                EB_DELETE(intermediate_buffer_ptr);
            }
        }

        // Post the Recon object
        svt_post_full_object(output_recon_wrapper_ptr);
    } else {
        // Overlay and altref have 1 recon only, which is from overlay pictures. So the recon of the
        // alt_ref is not sent to the application. However, to hanlde the end of sequence properly,
        // total_number_of_recon_frames is increamented
        enc_ctx->total_number_of_recon_frames++;
    }
    svt_release_mutex(enc_ctx->total_number_of_recon_frame_mutex);
}

//************************************/
// Calculate Frame SSIM
/************************************/

static void svt_aom_ssim_parms_8x8_c(const uint8_t* s, int sp, const uint8_t* r, int rp, uint32_t* sum_s,
                                     uint32_t* sum_r, uint32_t* sum_sq_s, uint32_t* sum_sq_r, uint32_t* sum_sxr) {
    int i, j;
    for (i = 0; i < 8; i++, s += sp, r += rp) {
        for (j = 0; j < 8; j++) {
            *sum_s += s[j];
            *sum_r += r[j];
            *sum_sq_s += s[j] * s[j];
            *sum_sq_r += r[j] * r[j];
            *sum_sxr += s[j] * r[j];
        }
    }
}

static void svt_aom_highbd_ssim_parms_8x8_c(const uint8_t* s, int sp, const uint8_t* sinc, int spinc, const uint16_t* r,
                                            int rp, uint32_t* sum_s, uint32_t* sum_r, uint32_t* sum_sq_s,
                                            uint32_t* sum_sq_r, uint32_t* sum_sxr) {
    int      i, j;
    uint32_t ss;
    for (i = 0; i < 8; i++, s += sp, sinc += spinc, r += rp) {
        for (j = 0; j < 8; j++) {
            ss = (int64_t)(s[j] << 2) + ((sinc[j] >> 6) & 0x3);
            *sum_s += ss;
            *sum_r += r[j];
            *sum_sq_s += ss * ss;
            *sum_sq_r += r[j] * r[j];
            *sum_sxr += ss * r[j];
        }
    }
}

static const int64_t cc1    = 26634; // (64^2*(.01*255)^2
static const int64_t cc2    = 239708; // (64^2*(.03*255)^2
static const int64_t cc1_10 = 428658; // (64^2*(.01*1023)^2
static const int64_t cc2_10 = 3857925; // (64^2*(.03*1023)^2
static const int64_t cc1_12 = 6868593; // (64^2*(.01*4095)^2
static const int64_t cc2_12 = 61817334; // (64^2*(.03*4095)^2

double similarity(uint32_t sum_s, uint32_t sum_r, uint32_t sum_sq_s, uint32_t sum_sq_r, uint32_t sum_sxr, int count,
                  uint32_t bd) {
    double  ssim_n, ssim_d;
    int64_t c1, c2;

    if (bd == 8) {
        // scale the constants by number of pixels
        c1 = (cc1 * count * count) >> 12;
        c2 = (cc2 * count * count) >> 12;
    } else if (bd == 10) {
        c1 = (cc1_10 * count * count) >> 12;
        c2 = (cc2_10 * count * count) >> 12;
    } else if (bd == 12) {
        c1 = (cc1_12 * count * count) >> 12;
        c2 = (cc2_12 * count * count) >> 12;
    } else {
        c1 = c2 = 0;
        assert(0);
    }

    ssim_n = (2.0 * sum_s * sum_r + c1) * (2.0 * count * sum_sxr - 2.0 * sum_s * sum_r + c2);

    ssim_d = ((double)sum_s * sum_s + (double)sum_r * sum_r + c1) *
        ((double)count * sum_sq_s - (double)sum_s * sum_s + (double)count * sum_sq_r - (double)sum_r * sum_r + c2);

    return ssim_n / ssim_d;
}

static double ssim_8x8(const uint8_t* s, int sp, const uint8_t* r, int rp) {
    uint32_t sum_s = 0, sum_r = 0, sum_sq_s = 0, sum_sq_r = 0, sum_sxr = 0;
    svt_aom_ssim_parms_8x8_c(s, sp, r, rp, &sum_s, &sum_r, &sum_sq_s, &sum_sq_r, &sum_sxr);
    return similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, 64, 8);
}

static double highbd_ssim_8x8(const uint8_t* s, int sp, const uint8_t* sinc, int spinc, const uint16_t* r, int rp,
                              uint32_t bd, uint32_t shift) {
    uint32_t sum_s = 0, sum_r = 0, sum_sq_s = 0, sum_sq_r = 0, sum_sxr = 0;
    svt_aom_highbd_ssim_parms_8x8_c(s, sp, sinc, spinc, r, rp, &sum_s, &sum_r, &sum_sq_s, &sum_sq_r, &sum_sxr);
    return similarity(sum_s >> shift,
                      sum_r >> shift,
                      sum_sq_s >> (2 * shift),
                      sum_sq_r >> (2 * shift),
                      sum_sxr >> (2 * shift),
                      64,
                      bd);
}

// We are using a 8x8 moving window with starting location of each 8x8 window
// on the 4x4 pixel grid. Such arrangement allows the windows to overlap
// block boundaries to penalize blocking artifacts.
static double aom_ssim2(const uint8_t* img1, int stride_img1, const uint8_t* img2, int stride_img2, int width,
                        int height) {
    int    i, j;
    int    samples    = 0;
    double ssim_total = 0;

    // region too small to compute meaningful SSIM score
    if (width <= 8 || height <= 8) {
        return NAN;
    }

    // sample point start with each 4x4 location
    for (i = 0; i <= height - 8; i += 4, img1 += stride_img1 * 4, img2 += stride_img2 * 4) {
        for (j = 0; j <= width - 8; j += 4) {
            double v = ssim_8x8(img1 + j, stride_img1, img2 + j, stride_img2);
            ssim_total += v;
            samples++;
        }
    }
    assert(samples > 0);
    ssim_total /= samples;
    return ssim_total;
}

static double aom_highbd_ssim2(const uint8_t* img1, int stride_img1, const uint8_t* img1inc, int stride_img1inc,
                               const uint16_t* img2, int stride_img2, int width, int height, uint32_t bd,
                               uint32_t shift) {
    int    i, j;
    int    samples    = 0;
    double ssim_total = 0;

    // region too small to compute meaningful SSIM score
    if (width <= 8 || height <= 8) {
        return NAN;
    }

    // sample point start with each 4x4 location
    for (i = 0; i <= height - 8;
         i += 4, img1 += stride_img1 * 4, img1inc += stride_img1inc * 4, img2 += stride_img2 * 4) {
        for (j = 0; j <= width - 8; j += 4) {
            double v = highbd_ssim_8x8(
                (img1 + j), stride_img1, (img1inc + j), stride_img1inc, (img2 + j), stride_img2, bd, shift);
            ssim_total += v;
            samples++;
        }
    }
    assert(samples > 0);
    ssim_total /= samples;
    return ssim_total;
}

void free_temporal_filtering_buffer(PictureControlSet* pcs, SequenceControlSet* scs) {
    // save_source_picture_ptr will be allocated only if do_tf is true in svt_av1_init_temporal_filtering().
    if (!pcs->ppcs->do_tf) {
        return;
    }

    EB_FREE_ARRAY(pcs->ppcs->save_source_picture_ptr[0]);
    EB_FREE_ARRAY(pcs->ppcs->save_source_picture_ptr[1]);
    EB_FREE_ARRAY(pcs->ppcs->save_source_picture_ptr[2]);

    bool is_16bit = (scs->static_config.encoder_bit_depth > EB_EIGHT_BIT);
    if (is_16bit) {
        EB_FREE_ARRAY(pcs->ppcs->save_source_picture_bit_inc_ptr[0]);
        EB_FREE_ARRAY(pcs->ppcs->save_source_picture_bit_inc_ptr[1]);
        EB_FREE_ARRAY(pcs->ppcs->save_source_picture_bit_inc_ptr[2]);
    }
}

EbErrorType svt_aom_ssim_calculations(PictureControlSet* pcs, SequenceControlSet* scs, bool free_memory) {
    bool is_16bit = (scs->static_config.encoder_bit_depth > EB_EIGHT_BIT);

    const uint32_t ss_x = scs->subsampling_x;
    const uint32_t ss_y = scs->subsampling_y;

    EbPictureBufferDesc* recon_ptr;
    EbPictureBufferDesc* input_pic = (EbPictureBufferDesc*)pcs->ppcs->enhanced_unscaled_pic;
    svt_aom_get_recon_pic(pcs, &recon_ptr, is_16bit);

    // upscale recon if resized
    EbPictureBufferDesc* upscaled_recon = NULL;
    bool                 is_resized = recon_ptr->width != input_pic->width || recon_ptr->height != input_pic->height;
    if (is_resized) {
        superres_params_type spr_params = {input_pic->width, input_pic->height, 0};
        svt_aom_downscaled_source_buffer_desc_ctor(&upscaled_recon, recon_ptr, spr_params);
        svt_aom_resize_frame(recon_ptr,
                             upscaled_recon,
                             scs->static_config.encoder_bit_depth,
                             av1_num_planes(&scs->seq_header.color_config),
                             ss_x,
                             ss_y,
                             recon_ptr->packed_flag,
                             PICTURE_BUFFER_DESC_FULL_MASK,
                             0); // is_2bcompress
        recon_ptr = upscaled_recon;
    }

    const int32_t input_org_x_c = input_pic->org_x >> ss_x;
    const int32_t input_org_y_c = input_pic->org_y >> ss_y;
    const int32_t recon_org_x_c = recon_ptr->org_x >> ss_x;
    const int32_t recon_org_y_c = recon_ptr->org_y >> ss_y;

    if (!is_16bit) {
        EbByte input_buffer;
        EbByte recon_buffer;
        EbByte buffer_y;
        EbByte buffer_cb;
        EbByte buffer_cr;

        // if current source picture was temporally filtered, use an alternative buffer which stores
        // the original source picture
        if (pcs->ppcs->do_tf == true) {
            assert(pcs->ppcs->save_source_picture_width == input_pic->width &&
                   pcs->ppcs->save_source_picture_height == input_pic->height);
            buffer_y  = pcs->ppcs->save_source_picture_ptr[0];
            buffer_cb = pcs->ppcs->save_source_picture_ptr[1];
            buffer_cr = pcs->ppcs->save_source_picture_ptr[2];
        } else {
            buffer_y  = input_pic->buffer_y;
            buffer_cb = input_pic->buffer_cb;
            buffer_cr = input_pic->buffer_cr;
        }

        recon_buffer         = &recon_ptr->buffer_y[recon_ptr->org_x + recon_ptr->org_y * recon_ptr->stride_y];
        input_buffer         = &buffer_y[input_pic->org_x + input_pic->org_y * input_pic->stride_y];
        pcs->ppcs->luma_ssim = aom_ssim2(input_buffer,
                                         input_pic->stride_y,
                                         recon_buffer,
                                         recon_ptr->stride_y,
                                         scs->max_input_luma_width,
                                         scs->max_input_luma_height);

        recon_buffer       = &recon_ptr->buffer_cb[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cb];
        input_buffer       = &buffer_cb[input_org_x_c + input_org_y_c * input_pic->stride_cb];
        pcs->ppcs->cb_ssim = aom_ssim2(input_buffer,
                                       input_pic->stride_cb,
                                       recon_buffer,
                                       recon_ptr->stride_cb,
                                       scs->chroma_width,
                                       scs->chroma_height);

        recon_buffer       = &recon_ptr->buffer_cr[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cr];
        input_buffer       = &buffer_cr[input_org_x_c + input_org_y_c * input_pic->stride_cr];
        pcs->ppcs->cr_ssim = aom_ssim2(input_buffer,
                                       input_pic->stride_cr,
                                       recon_buffer,
                                       recon_ptr->stride_cr,
                                       scs->chroma_width,
                                       scs->chroma_height);

        if (free_memory && pcs->ppcs->do_tf == true) {
            EB_FREE_ARRAY(buffer_y);
            EB_FREE_ARRAY(buffer_cb);
            EB_FREE_ARRAY(buffer_cr);
        }
    } else {
        EbByte    input_buffer;
        EbByte    input_buffer_bit_inc;
        uint16_t* recon_buffer;

        // if current source picture was temporally filtered, use an alternative buffer which stores
        // the original source picture
        EbByte buffer_y, buffer_bit_inc_y;
        EbByte buffer_cb, buffer_bit_inc_cb;
        EbByte buffer_cr, buffer_bit_inc_cr;

        if (pcs->ppcs->do_tf == true) {
            assert(pcs->ppcs->save_source_picture_width == input_pic->width &&
                   pcs->ppcs->save_source_picture_height == input_pic->height);
            buffer_y          = pcs->ppcs->save_source_picture_ptr[0];
            buffer_bit_inc_y  = pcs->ppcs->save_source_picture_bit_inc_ptr[0];
            buffer_cb         = pcs->ppcs->save_source_picture_ptr[1];
            buffer_bit_inc_cb = pcs->ppcs->save_source_picture_bit_inc_ptr[1];
            buffer_cr         = pcs->ppcs->save_source_picture_ptr[2];
            buffer_bit_inc_cr = pcs->ppcs->save_source_picture_bit_inc_ptr[2];
        } else {
            uint32_t height_y = input_pic->height + input_pic->org_y + input_pic->origin_bot_y;

            buffer_y  = input_pic->buffer_y;
            buffer_cb = input_pic->buffer_cb;
            buffer_cr = input_pic->buffer_cr;

            EB_MALLOC_ARRAY(buffer_bit_inc_y, pcs->ppcs->enhanced_unscaled_pic->luma_size);
            EB_MALLOC_ARRAY(buffer_bit_inc_cb, pcs->ppcs->enhanced_unscaled_pic->chroma_size);
            EB_MALLOC_ARRAY(buffer_bit_inc_cr, pcs->ppcs->enhanced_unscaled_pic->chroma_size);

            svt_c_unpack_compressed_10bit(input_pic->buffer_bit_inc_y,
                                          input_pic->stride_bit_inc_y / 4,
                                          buffer_bit_inc_y,
                                          input_pic->stride_bit_inc_y,
                                          height_y);
            // U
            svt_c_unpack_compressed_10bit(input_pic->buffer_bit_inc_cb,
                                          input_pic->stride_bit_inc_cb / 4,
                                          buffer_bit_inc_cb,
                                          input_pic->stride_bit_inc_cb,
                                          height_y >> ss_y);
            // V
            svt_c_unpack_compressed_10bit(input_pic->buffer_bit_inc_cr,
                                          input_pic->stride_bit_inc_cr / 4,
                                          buffer_bit_inc_cr,
                                          input_pic->stride_bit_inc_cr,
                                          height_y >> ss_y);
        }

        int bd    = 10;
        int shift = 0; // both input and output are 10 bit (bitdepth - input_bd)

        recon_buffer = &((uint16_t*)recon_ptr->buffer_y)[recon_ptr->org_x + recon_ptr->org_y * recon_ptr->stride_y];
        input_buffer = &buffer_y[input_pic->org_x + input_pic->org_y * input_pic->stride_y];
        input_buffer_bit_inc = &buffer_bit_inc_y[input_pic->org_x + input_pic->org_y * input_pic->stride_bit_inc_y];
        pcs->ppcs->luma_ssim = aom_highbd_ssim2(input_buffer,
                                                input_pic->stride_y,
                                                input_buffer_bit_inc,
                                                input_pic->stride_bit_inc_y,
                                                recon_buffer,
                                                recon_ptr->stride_y,
                                                scs->max_input_luma_width,
                                                scs->max_input_luma_height,
                                                bd,
                                                shift);

        recon_buffer         = &((uint16_t*)recon_ptr->buffer_cb)[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cb];
        input_buffer         = &buffer_cb[input_org_x_c + input_org_y_c * input_pic->stride_cb];
        input_buffer_bit_inc = &buffer_bit_inc_cb[input_org_x_c + input_org_y_c * input_pic->stride_bit_inc_cb];
        pcs->ppcs->cb_ssim   = aom_highbd_ssim2(input_buffer,
                                              input_pic->stride_cb,
                                              input_buffer_bit_inc,
                                              input_pic->stride_bit_inc_cb,
                                              recon_buffer,
                                              recon_ptr->stride_cb,
                                              scs->chroma_width,
                                              scs->chroma_height,
                                              bd,
                                              shift);

        recon_buffer         = &((uint16_t*)recon_ptr->buffer_cr)[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cr];
        input_buffer         = &buffer_cr[input_org_x_c + input_org_y_c * input_pic->stride_cr];
        input_buffer_bit_inc = &buffer_bit_inc_cr[input_org_x_c + input_org_y_c * input_pic->stride_bit_inc_cr];
        pcs->ppcs->cr_ssim   = aom_highbd_ssim2(input_buffer,
                                              input_pic->stride_cr,
                                              input_buffer_bit_inc,
                                              input_pic->stride_bit_inc_cr,
                                              recon_buffer,
                                              recon_ptr->stride_cr,
                                              scs->chroma_width,
                                              scs->chroma_height,
                                              bd,
                                              shift);

        if (free_memory && pcs->ppcs->do_tf == true) {
            EB_FREE_ARRAY(buffer_y);
            EB_FREE_ARRAY(buffer_cb);
            EB_FREE_ARRAY(buffer_cr);
            EB_FREE_ARRAY(buffer_bit_inc_y);
            EB_FREE_ARRAY(buffer_bit_inc_cb);
            EB_FREE_ARRAY(buffer_bit_inc_cr);
        }
        if (pcs->ppcs->do_tf == false) {
            EB_FREE_ARRAY(buffer_bit_inc_y);
            EB_FREE_ARRAY(buffer_bit_inc_cb);
            EB_FREE_ARRAY(buffer_bit_inc_cr);
        }
    }
    EB_DELETE(upscaled_recon);
    return EB_ErrorNone;
}

static int64_t get_sse_10bit(const uint8_t* a_hi, int32_t a_hi_stride, const uint8_t* a_lo, int32_t a_lo_stride,
                             const uint16_t* b, int32_t b_stride, int32_t width, int32_t height) {
    int64_t sse = 0;

    for (int j = 0; j < height; ++j) {
        for (int i = 0; i < width; ++i) {
            sse += SQR(((a_hi[i] << 2) | (a_lo[i] >> 6)) - b[i]);
        }

        a_hi += a_hi_stride;
        a_lo += a_lo_stride;
        b += b_stride;
    }

    return sse;
}

EbErrorType psnr_calculations(PictureControlSet* pcs, SequenceControlSet* scs, bool free_memory) {
    bool is_16bit = (scs->static_config.encoder_bit_depth > EB_EIGHT_BIT);

    const uint32_t ss_x = scs->subsampling_x;
    const uint32_t ss_y = scs->subsampling_y;

    EbPictureBufferDesc* recon_ptr;
    EbPictureBufferDesc* input_pic = (EbPictureBufferDesc*)pcs->ppcs->enhanced_unscaled_pic;
    svt_aom_get_recon_pic(pcs, &recon_ptr, is_16bit);

    // upscale recon if resized
    EbPictureBufferDesc* upscaled_recon = NULL;
    bool                 is_resized = recon_ptr->width != input_pic->width || recon_ptr->height != input_pic->height;
    if (is_resized) {
        superres_params_type spr_params = {input_pic->width, input_pic->height, 0};
        svt_aom_downscaled_source_buffer_desc_ctor(&upscaled_recon, recon_ptr, spr_params);
        svt_aom_resize_frame(recon_ptr,
                             upscaled_recon,
                             scs->static_config.encoder_bit_depth,
                             av1_num_planes(&scs->seq_header.color_config),
                             ss_x,
                             ss_y,
                             recon_ptr->packed_flag,
                             PICTURE_BUFFER_DESC_FULL_MASK,
                             0); // is_2bcompress
        recon_ptr = upscaled_recon;
    }

    const int32_t pic_w = input_pic->width - scs->max_input_pad_right;
    const int32_t pic_h = input_pic->height - scs->max_input_pad_bottom;

    const int32_t input_org_x_c = input_pic->org_x >> ss_x;
    const int32_t input_org_y_c = input_pic->org_y >> ss_y;
    const int32_t recon_org_x_c = recon_ptr->org_x >> ss_x;
    const int32_t recon_org_y_c = recon_ptr->org_y >> ss_y;

    if (!is_16bit) {
        EbByte input_buffer;
        EbByte recon_buffer;
        EbByte buffer_y;
        EbByte buffer_cb;
        EbByte buffer_cr;

        // if current source picture was temporally filtered, use an alternative buffer which stores
        // the original source picture
        if (pcs->ppcs->do_tf == true) {
            assert(pcs->ppcs->save_source_picture_width == input_pic->width &&
                   pcs->ppcs->save_source_picture_height == input_pic->height);
            buffer_y  = pcs->ppcs->save_source_picture_ptr[0];
            buffer_cb = pcs->ppcs->save_source_picture_ptr[1];
            buffer_cr = pcs->ppcs->save_source_picture_ptr[2];
        } else {
            buffer_y  = input_pic->buffer_y;
            buffer_cb = input_pic->buffer_cb;
            buffer_cr = input_pic->buffer_cr;
        }

        recon_buffer        = &recon_ptr->buffer_y[recon_ptr->org_x + recon_ptr->org_y * recon_ptr->stride_y];
        input_buffer        = &buffer_y[input_pic->org_x + input_pic->org_y * input_pic->stride_y];
        pcs->ppcs->luma_sse = svt_aom_get_sse(
            input_buffer, input_pic->stride_y, recon_buffer, recon_ptr->stride_y, pic_w, pic_h);

        recon_buffer      = &recon_ptr->buffer_cb[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cb];
        input_buffer      = &buffer_cb[input_org_x_c + input_org_y_c * input_pic->stride_cb];
        pcs->ppcs->cb_sse = svt_aom_get_sse(
            input_buffer, input_pic->stride_cb, recon_buffer, recon_ptr->stride_cb, pic_w >> ss_x, pic_h >> ss_y);

        recon_buffer      = &recon_ptr->buffer_cr[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cr];
        input_buffer      = &buffer_cr[input_org_x_c + input_org_y_c * input_pic->stride_cr];
        pcs->ppcs->cr_sse = svt_aom_get_sse(
            input_buffer, input_pic->stride_cr, recon_buffer, recon_ptr->stride_cr, pic_w >> ss_x, pic_h >> ss_y);

        if (free_memory && pcs->ppcs->do_tf == true) {
            EB_FREE_ARRAY(buffer_y);
            EB_FREE_ARRAY(buffer_cb);
            EB_FREE_ARRAY(buffer_cr);
        }
    } else {
        EbByte    input_buffer;
        EbByte    input_buffer_bit_inc;
        uint16_t* recon_buffer;

        // if current source picture was temporally filtered, use an alternative buffer which stores
        // the original source picture
        EbByte buffer_y, buffer_bit_inc_y;
        EbByte buffer_cb, buffer_bit_inc_cb;
        EbByte buffer_cr, buffer_bit_inc_cr;

        if (pcs->ppcs->do_tf == true) {
            assert(pcs->ppcs->save_source_picture_width == input_pic->width &&
                   pcs->ppcs->save_source_picture_height == input_pic->height);
            buffer_y          = pcs->ppcs->save_source_picture_ptr[0];
            buffer_bit_inc_y  = pcs->ppcs->save_source_picture_bit_inc_ptr[0];
            buffer_cb         = pcs->ppcs->save_source_picture_ptr[1];
            buffer_bit_inc_cb = pcs->ppcs->save_source_picture_bit_inc_ptr[1];
            buffer_cr         = pcs->ppcs->save_source_picture_ptr[2];
            buffer_bit_inc_cr = pcs->ppcs->save_source_picture_bit_inc_ptr[2];
        } else {
            uint32_t height_y = input_pic->height + input_pic->org_y + input_pic->origin_bot_y;

            buffer_y  = input_pic->buffer_y;
            buffer_cb = input_pic->buffer_cb;
            buffer_cr = input_pic->buffer_cr;

            EB_MALLOC_ARRAY(buffer_bit_inc_y, pcs->ppcs->enhanced_unscaled_pic->luma_size);
            EB_MALLOC_ARRAY(buffer_bit_inc_cb, pcs->ppcs->enhanced_unscaled_pic->chroma_size);
            EB_MALLOC_ARRAY(buffer_bit_inc_cr, pcs->ppcs->enhanced_unscaled_pic->chroma_size);

            svt_c_unpack_compressed_10bit(input_pic->buffer_bit_inc_y,
                                          input_pic->stride_bit_inc_y / 4,
                                          buffer_bit_inc_y,
                                          input_pic->stride_bit_inc_y,
                                          height_y);
            // U
            svt_c_unpack_compressed_10bit(input_pic->buffer_bit_inc_cb,
                                          input_pic->stride_bit_inc_cb / 4,
                                          buffer_bit_inc_cb,
                                          input_pic->stride_bit_inc_cb,
                                          height_y >> ss_y);
            // V
            svt_c_unpack_compressed_10bit(input_pic->buffer_bit_inc_cr,
                                          input_pic->stride_bit_inc_cr / 4,
                                          buffer_bit_inc_cr,
                                          input_pic->stride_bit_inc_cr,
                                          height_y >> ss_y);
        }

        recon_buffer = &((uint16_t*)recon_ptr->buffer_y)[recon_ptr->org_x + recon_ptr->org_y * recon_ptr->stride_y];
        input_buffer = &buffer_y[input_pic->org_x + input_pic->org_y * input_pic->stride_y];
        input_buffer_bit_inc = &buffer_bit_inc_y[input_pic->org_x + input_pic->org_y * input_pic->stride_bit_inc_y];

        pcs->ppcs->luma_sse = get_sse_10bit(input_buffer,
                                            input_pic->stride_y,
                                            input_buffer_bit_inc,
                                            input_pic->stride_bit_inc_y,
                                            recon_buffer,
                                            recon_ptr->stride_y,
                                            pic_w,
                                            pic_h);

        recon_buffer         = &((uint16_t*)recon_ptr->buffer_cb)[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cb];
        input_buffer         = &buffer_cb[input_org_x_c + input_org_y_c * input_pic->stride_cb];
        input_buffer_bit_inc = &buffer_bit_inc_cb[input_org_x_c + input_org_y_c * input_pic->stride_bit_inc_cb];

        pcs->ppcs->cb_sse = get_sse_10bit(input_buffer,
                                          input_pic->stride_cb,
                                          input_buffer_bit_inc,
                                          input_pic->stride_bit_inc_cb,
                                          recon_buffer,
                                          recon_ptr->stride_cb,
                                          pic_w >> ss_x,
                                          pic_h >> ss_y);

        recon_buffer         = &((uint16_t*)recon_ptr->buffer_cr)[recon_org_x_c + recon_org_y_c * recon_ptr->stride_cr];
        input_buffer         = &buffer_cr[input_org_x_c + input_org_y_c * input_pic->stride_cr];
        input_buffer_bit_inc = &buffer_bit_inc_cr[input_org_x_c + input_org_y_c * input_pic->stride_bit_inc_cr];

        pcs->ppcs->cr_sse = get_sse_10bit(input_buffer,
                                          input_pic->stride_cr,
                                          input_buffer_bit_inc,
                                          input_pic->stride_bit_inc_cr,
                                          recon_buffer,
                                          recon_ptr->stride_cr,
                                          pic_w >> ss_x,
                                          pic_h >> ss_y);

        if (free_memory && pcs->ppcs->do_tf == true) {
            EB_FREE_ARRAY(buffer_y);
            EB_FREE_ARRAY(buffer_cb);
            EB_FREE_ARRAY(buffer_cr);
            EB_FREE_ARRAY(buffer_bit_inc_y);
            EB_FREE_ARRAY(buffer_bit_inc_cb);
            EB_FREE_ARRAY(buffer_bit_inc_cr);
        }
        if (pcs->ppcs->do_tf == false) {
            EB_FREE_ARRAY(buffer_bit_inc_y);
            EB_FREE_ARRAY(buffer_bit_inc_cb);
            EB_FREE_ARRAY(buffer_bit_inc_cr);
        }
    }
    EB_DELETE(upscaled_recon);
    return EB_ErrorNone;
}

void pad_ref_and_set_flags(PictureControlSet* pcs, SequenceControlSet* scs) {
    EbReferenceObject* ref_object = (EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr;

    EbPictureBufferDesc* ref_pic_ptr;
    EbPictureBufferDesc* ref_pic_16bit_ptr;

    {
        svt_aom_get_recon_pic(pcs, &ref_pic_ptr, 0);
        svt_aom_get_recon_pic(pcs, &ref_pic_16bit_ptr, 1);
    }
    const bool     is_16bit     = (scs->static_config.encoder_bit_depth > EB_EIGHT_BIT);
    const uint32_t color_format = ref_pic_ptr->color_format;
    const uint16_t ss_x         = (color_format == EB_YUV444 ? 0 : 1);
    const uint16_t ss_y         = (color_format >= EB_YUV422 ? 0 : 1);

    if (!is_16bit) {
        svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(scs, ref_pic_ptr);
        // Y samples
        svt_aom_generate_padding(ref_pic_ptr->buffer_y,
                                 ref_pic_ptr->stride_y,
                                 ref_pic_ptr->width,
                                 ref_pic_ptr->height,
                                 ref_pic_ptr->org_x,
                                 ref_pic_ptr->org_y);

        // Cb samples
        svt_aom_generate_padding(ref_pic_ptr->buffer_cb,
                                 ref_pic_ptr->stride_cb,
                                 (ref_pic_ptr->width + ss_x) >> ss_x,
                                 (ref_pic_ptr->height + ss_y) >> ss_y,
                                 (ref_pic_ptr->org_x + ss_x) >> ss_x,
                                 (ref_pic_ptr->org_y + ss_y) >> ss_y);

        // Cr samples
        svt_aom_generate_padding(ref_pic_ptr->buffer_cr,
                                 ref_pic_ptr->stride_cr,
                                 (ref_pic_ptr->width + ss_x) >> ss_x,
                                 (ref_pic_ptr->height + ss_y) >> ss_y,
                                 (ref_pic_ptr->org_x + ss_x) >> ss_x,
                                 (ref_pic_ptr->org_y + ss_y) >> ss_y);
    }

    //We need this for MCP
    if (is_16bit) {
        // Non visible Reference samples should be overwritten by the last visible line of pixels
        svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions_16bit(scs, ref_pic_16bit_ptr);

        // Y samples
        svt_aom_generate_padding16_bit((uint16_t*)ref_pic_16bit_ptr->buffer_y,
                                       ref_pic_16bit_ptr->stride_y,
                                       ref_pic_16bit_ptr->width,
                                       ref_pic_16bit_ptr->height,
                                       ref_pic_16bit_ptr->org_x,
                                       ref_pic_16bit_ptr->org_y);

        // Cb samples
        svt_aom_generate_padding16_bit((uint16_t*)ref_pic_16bit_ptr->buffer_cb,
                                       ref_pic_16bit_ptr->stride_cb,
                                       (ref_pic_16bit_ptr->width + ss_x) >> ss_x,
                                       (ref_pic_16bit_ptr->height + ss_y) >> ss_y,
                                       (ref_pic_16bit_ptr->org_x + ss_x) >> ss_x,
                                       (ref_pic_16bit_ptr->org_y + ss_y) >> ss_y);

        // Cr samples
        svt_aom_generate_padding16_bit((uint16_t*)ref_pic_16bit_ptr->buffer_cr,
                                       ref_pic_16bit_ptr->stride_cr,
                                       (ref_pic_16bit_ptr->width + ss_x) >> ss_x,
                                       (ref_pic_16bit_ptr->height + ss_y) >> ss_y,
                                       (ref_pic_16bit_ptr->org_x + ss_x) >> ss_x,
                                       (ref_pic_16bit_ptr->org_y + ss_y) >> ss_y);

        // Hsan: unpack ref samples (to be used @ MD)
        svt_aom_un_pack2d((uint16_t*)ref_pic_16bit_ptr->buffer_y,
                          ref_pic_16bit_ptr->stride_y,
                          ref_pic_ptr->buffer_y,
                          ref_pic_ptr->stride_y,
                          ref_pic_ptr->buffer_bit_inc_y,
                          ref_pic_ptr->stride_bit_inc_y,
                          ref_pic_16bit_ptr->width + (ref_pic_ptr->org_x << 1),
                          ref_pic_16bit_ptr->height + (ref_pic_ptr->org_y << 1));
        svt_aom_un_pack2d((uint16_t*)ref_pic_16bit_ptr->buffer_cb,
                          ref_pic_16bit_ptr->stride_cb,
                          ref_pic_ptr->buffer_cb,
                          ref_pic_ptr->stride_cb,
                          ref_pic_ptr->buffer_bit_inc_cb,
                          ref_pic_ptr->stride_bit_inc_cb,
                          (ref_pic_16bit_ptr->width + ss_x + (ref_pic_ptr->org_x << 1)) >> ss_x,
                          (ref_pic_16bit_ptr->height + ss_y + (ref_pic_ptr->org_y << 1)) >> ss_y);
        svt_aom_un_pack2d((uint16_t*)ref_pic_16bit_ptr->buffer_cr,
                          ref_pic_16bit_ptr->stride_cr,
                          ref_pic_ptr->buffer_cr,
                          ref_pic_ptr->stride_cr,
                          ref_pic_ptr->buffer_bit_inc_cr,
                          ref_pic_ptr->stride_bit_inc_cr,
                          (ref_pic_16bit_ptr->width + ss_x + (ref_pic_ptr->org_x << 1)) >> ss_x,
                          (ref_pic_16bit_ptr->height + ss_y + (ref_pic_ptr->org_y << 1)) >> ss_y);
    }
    if ((scs->is_16bit_pipeline) && (!is_16bit)) {
        // Y samples
        svt_aom_generate_padding16_bit((uint16_t*)ref_pic_16bit_ptr->buffer_y,
                                       ref_pic_16bit_ptr->stride_y,
                                       ref_pic_16bit_ptr->width - scs->max_input_pad_right,
                                       ref_pic_16bit_ptr->height - scs->max_input_pad_bottom,
                                       ref_pic_16bit_ptr->org_x,
                                       ref_pic_16bit_ptr->org_y);

        // Cb samples
        svt_aom_generate_padding16_bit((uint16_t*)ref_pic_16bit_ptr->buffer_cb,
                                       ref_pic_16bit_ptr->stride_cb,
                                       (ref_pic_16bit_ptr->width + ss_x - scs->max_input_pad_right) >> ss_x,
                                       (ref_pic_16bit_ptr->height + ss_y - scs->max_input_pad_bottom) >> ss_y,
                                       (ref_pic_16bit_ptr->org_x + ss_x) >> ss_x,
                                       (ref_pic_16bit_ptr->org_y + ss_y) >> ss_y);

        // Cr samples
        svt_aom_generate_padding16_bit((uint16_t*)ref_pic_16bit_ptr->buffer_cr,
                                       ref_pic_16bit_ptr->stride_cr,
                                       (ref_pic_16bit_ptr->width + ss_x - scs->max_input_pad_right) >> ss_x,
                                       (ref_pic_16bit_ptr->height + ss_y - scs->max_input_pad_bottom) >> ss_y,
                                       (ref_pic_16bit_ptr->org_x + ss_x) >> ss_x,
                                       (ref_pic_16bit_ptr->org_y + ss_y) >> ss_y);

        // Hsan: unpack ref samples (to be used @ MD)

        //Y
        uint16_t* buf_16bit = (uint16_t*)(ref_pic_16bit_ptr->buffer_y);
        uint8_t*  buf_8bit  = ref_pic_ptr->buffer_y;
        svt_convert_16bit_to_8bit(buf_16bit,
                                  ref_pic_16bit_ptr->stride_y,
                                  buf_8bit,
                                  ref_pic_ptr->stride_y,
                                  ref_pic_16bit_ptr->width + (ref_pic_ptr->org_x << 1),
                                  ref_pic_16bit_ptr->height + (ref_pic_ptr->org_y << 1));

        //CB
        buf_16bit = (uint16_t*)(ref_pic_16bit_ptr->buffer_cb);
        buf_8bit  = ref_pic_ptr->buffer_cb;
        svt_convert_16bit_to_8bit(buf_16bit,
                                  ref_pic_16bit_ptr->stride_cb,
                                  buf_8bit,
                                  ref_pic_ptr->stride_cb,
                                  (ref_pic_16bit_ptr->width + ss_x + (ref_pic_ptr->org_x << 1)) >> ss_x,
                                  (ref_pic_16bit_ptr->height + ss_y + (ref_pic_ptr->org_y << 1)) >> ss_y);

        //CR
        buf_16bit = (uint16_t*)(ref_pic_16bit_ptr->buffer_cr);
        buf_8bit  = ref_pic_ptr->buffer_cr;
        svt_convert_16bit_to_8bit(buf_16bit,
                                  ref_pic_16bit_ptr->stride_cr,
                                  buf_8bit,
                                  ref_pic_ptr->stride_cr,
                                  (ref_pic_16bit_ptr->width + ss_x + (ref_pic_ptr->org_x << 1)) >> ss_x,
                                  (ref_pic_16bit_ptr->height + ss_y + (ref_pic_ptr->org_y << 1)) >> ss_y);
    }
    // set up the ref POC
    ref_object->ref_poc = pcs->ppcs->picture_number;

    // set up the base_q_idx
    ref_object->base_q_idx = pcs->ppcs->frm_hdr.quantization_params.base_q_idx;

    // set up the Slice Type
    ref_object->slice_type = pcs->ppcs->slice_type;
    ref_object->r0         = pcs->ppcs->r0;
}

/*
 * Prepare the input picture for EncDec processing, including any necessary
 * padding, compressing, packing, or bit depth conversion.
 */
static void prepare_input_picture(SequenceControlSet* scs, PictureControlSet* pcs, EncDecContext* ctx,
                                  EbPictureBufferDesc* input_pic, uint32_t sb_org_x, uint32_t sb_org_y) {
    bool     is_16bit  = ctx->is_16bit;
    uint32_t sb_width  = MIN(scs->sb_size, pcs->ppcs->aligned_width - sb_org_x);
    uint32_t sb_height = MIN(scs->sb_size, pcs->ppcs->aligned_height - sb_org_y);

    if (is_16bit && scs->static_config.encoder_bit_depth > EB_EIGHT_BIT) {
        //SB128_TODO change 10bit SB creation

        const uint32_t input_luma_offset = ((sb_org_y + input_pic->org_y) * input_pic->stride_y) +
            (sb_org_x + input_pic->org_x);
        const uint32_t input_cb_offset = (((sb_org_y + input_pic->org_y) >> 1) * input_pic->stride_cb) +
            ((sb_org_x + input_pic->org_x) >> 1);
        const uint32_t input_cr_offset = (((sb_org_y + input_pic->org_y) >> 1) * input_pic->stride_cr) +
            ((sb_org_x + input_pic->org_x) >> 1);

        //sb_width is n*8 so the 2bit-decompression kernel works properly
        uint32_t comp_stride_y           = input_pic->stride_y / 4;
        uint32_t comp_luma_buffer_offset = comp_stride_y * input_pic->org_y + input_pic->org_x / 4;
        comp_luma_buffer_offset += sb_org_x / 4 + sb_org_y * comp_stride_y;

        svt_aom_compressed_pack_sb(input_pic->buffer_y + input_luma_offset,
                                   input_pic->stride_y,
                                   input_pic->buffer_bit_inc_y + comp_luma_buffer_offset,
                                   comp_stride_y,
                                   (uint16_t*)ctx->input_sample16bit_buffer->buffer_y,
                                   ctx->input_sample16bit_buffer->stride_y,
                                   sb_width,
                                   sb_height);

        uint32_t comp_stride_uv            = input_pic->stride_cb / 4;
        uint32_t comp_chroma_buffer_offset = comp_stride_uv * (input_pic->org_y / 2) + input_pic->org_x / 2 / 4;
        comp_chroma_buffer_offset += sb_org_x / 4 / 2 + sb_org_y / 2 * comp_stride_uv;

        svt_aom_compressed_pack_sb(input_pic->buffer_cb + input_cb_offset,
                                   input_pic->stride_cb,
                                   input_pic->buffer_bit_inc_cb + comp_chroma_buffer_offset,
                                   comp_stride_uv,
                                   (uint16_t*)ctx->input_sample16bit_buffer->buffer_cb,
                                   ctx->input_sample16bit_buffer->stride_cb,
                                   sb_width / 2,
                                   sb_height / 2);
        svt_aom_compressed_pack_sb(input_pic->buffer_cr + input_cr_offset,
                                   input_pic->stride_cr,
                                   input_pic->buffer_bit_inc_cr + comp_chroma_buffer_offset,
                                   comp_stride_uv,
                                   (uint16_t*)ctx->input_sample16bit_buffer->buffer_cr,
                                   ctx->input_sample16bit_buffer->stride_cr,
                                   sb_width / 2,
                                   sb_height / 2);

        // PAD the packed source in incomplete sb up to max SB size
        svt_aom_pad_input_picture_16bit((uint16_t*)ctx->input_sample16bit_buffer->buffer_y,
                                        ctx->input_sample16bit_buffer->stride_y,
                                        sb_width,
                                        sb_height,
                                        scs->sb_size - sb_width,
                                        scs->sb_size - sb_height);

        // Safe to divide by 2 (scs->sb_size - sb_width) >> 1), with no risk of off-of-one issues
        // from chroma subsampling as picture is already 8px aligned
        svt_aom_pad_input_picture_16bit((uint16_t*)ctx->input_sample16bit_buffer->buffer_cb,
                                        ctx->input_sample16bit_buffer->stride_cb,
                                        sb_width >> 1,
                                        sb_height >> 1,
                                        (scs->sb_size - sb_width) >> 1,
                                        (scs->sb_size - sb_height) >> 1);

        svt_aom_pad_input_picture_16bit((uint16_t*)ctx->input_sample16bit_buffer->buffer_cr,
                                        ctx->input_sample16bit_buffer->stride_cr,
                                        sb_width >> 1,
                                        sb_height >> 1,
                                        (scs->sb_size - sb_width) >> 1,
                                        (scs->sb_size - sb_height) >> 1);

        if (ctx->md_ctx->hbd_md == 0) {
            svt_aom_store16bit_input_src(
                ctx->input_sample16bit_buffer, pcs, sb_org_x, sb_org_y, scs->sb_size, scs->sb_size);
        }
    }

    if (is_16bit && scs->static_config.encoder_bit_depth == EB_EIGHT_BIT) {
        const uint32_t input_luma_offset = ((sb_org_y + input_pic->org_y) * input_pic->stride_y) +
            (sb_org_x + input_pic->org_x);
        const uint32_t input_cb_offset = (((sb_org_y + input_pic->org_y) >> 1) * input_pic->stride_cb) +
            ((sb_org_x + input_pic->org_x) >> 1);
        const uint32_t input_cr_offset = (((sb_org_y + input_pic->org_y) >> 1) * input_pic->stride_cr) +
            ((sb_org_x + input_pic->org_x) >> 1);

        sb_width  = ((sb_width < MIN_SB_SIZE) || ((sb_width > MIN_SB_SIZE) && (sb_width < MAX_SB_SIZE)))
             ? MIN(scs->sb_size, (pcs->ppcs->aligned_width + scs->right_padding) - sb_org_x)
             : sb_width;
        sb_height = ((sb_height < MIN_SB_SIZE) || ((sb_height > MIN_SB_SIZE) && (sb_height < MAX_SB_SIZE)))
            ? MIN(scs->sb_size, (pcs->ppcs->aligned_height + scs->bot_padding) - sb_org_y)
            : sb_height;

        // PACK Y
        uint16_t* buf_16bit = (uint16_t*)ctx->input_sample16bit_buffer->buffer_y;
        uint8_t*  buf_8bit  = input_pic->buffer_y + input_luma_offset;
        svt_convert_8bit_to_16bit(
            buf_8bit, input_pic->stride_y, buf_16bit, ctx->input_sample16bit_buffer->stride_y, sb_width, sb_height);

        // PACK CB
        buf_16bit = (uint16_t*)ctx->input_sample16bit_buffer->buffer_cb;
        buf_8bit  = input_pic->buffer_cb + input_cb_offset;
        svt_convert_8bit_to_16bit(buf_8bit,
                                  input_pic->stride_cb,
                                  buf_16bit,
                                  ctx->input_sample16bit_buffer->stride_cb,
                                  sb_width >> 1,
                                  sb_height >> 1);

        // PACK CR
        buf_16bit = (uint16_t*)ctx->input_sample16bit_buffer->buffer_cr;
        buf_8bit  = input_pic->buffer_cr + input_cr_offset;
        svt_convert_8bit_to_16bit(buf_8bit,
                                  input_pic->stride_cr,
                                  buf_16bit,
                                  ctx->input_sample16bit_buffer->stride_cr,
                                  sb_width >> 1,
                                  sb_height >> 1);
    }
}

static void copy_neighbour_arrays_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t src_idx,
                                            uint32_t dst_idx, uint32_t sb_org_x, uint32_t sb_org_y) {
    const uint16_t tile_idx = ctx->tile_index;

    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[src_idx][tile_idx],
                           pcs->md_luma_recon_na[dst_idx][tile_idx],
                           sb_org_x, // blk org is always the top left of the SB
                           sb_org_y,
                           64, // block is always the SB, which is 64x64 for LPD0
                           64,
                           NEIGHBOR_ARRAY_UNIT_FULL_MASK);
}

void svt_aom_copy_neighbour_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t src_idx, uint32_t dst_idx,
                                   const BlockSize bsize, const int mi_row, const int mi_col);

/* Update shapes and tot_shapes with the shapes to be tested at the current d1 depth, based on block
characteristics and settings.
*/
static void set_blocks_to_test(PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds, const int mi_row,
                               const int mi_col, Part shapes[9], uint8_t* tot_shapes) {
    const int      sq_size  = block_size_wide[mds->bsize];
    const int      hbs      = mi_size_wide[mds->bsize] >> 1;
    const int      has_rows = mi_row + hbs < pcs->ppcs->av1_cm->mi_rows;
    const int      has_cols = mi_col + hbs < pcs->ppcs->av1_cm->mi_cols;
    const uint16_t min_nsq  = ctx->pd_pass == PD_PASS_1 && ctx->lpd1_ctrls.pd1_level != REGULAR_PD1 ? 8 : 4;

    // If block has no valid partitions (i.e. must be SPLIT) exit immediately.
    // For an incomplete block if SQ shape is not allowed, H or V may still be allowed, but only
    // if the NSQ geom allows for it.
    if ((!has_cols && !has_rows) ||
        ((!has_cols || !has_rows) &&
         (!ctx->nsq_geom_ctrls.enabled || sq_size <= MAX(min_nsq, ctx->nsq_geom_ctrls.min_nsq_block_size)))) {
        *tot_shapes = 0;
        return;
    }

    const bool inj_hv_incomp = (!has_cols || !has_rows);
    uint8_t    shapes_idx    = 0;
    const Part max_part      = (!ctx->nsq_geom_ctrls.enabled || (sq_size <= ctx->nsq_geom_ctrls.min_nsq_block_size) ||
                           sq_size == 4 || (ctx->md_disallow_nsq_search && !inj_hv_incomp))
             ? PART_N
             : (sq_size == 8 || inj_hv_incomp) ? PART_V
                                               : PART_S - 1;
    for (Part part = PART_N; part <= max_part; part++) {
        if (inj_hv_incomp) {
            if ((has_cols && part != PART_H) || (has_rows && part != PART_V)) {
                continue;
            }
        }
        if ((part == PART_H4 || part == PART_V4) && sq_size == 128) {
            continue;
        }
        if (!ctx->nsq_geom_ctrls.allow_HVA_HVB &&
            (part == PART_HA || part == PART_HB || part == PART_VA || part == PART_VB)) {
            continue;
        }
        if (!ctx->nsq_geom_ctrls.allow_HV4 && (part == PART_H4 || part == PART_V4)) {
            continue;
        }
        shapes[shapes_idx++] = part;
    }
    *tot_shapes = shapes_idx;
}

// Initialize the MD blocks to be tested
static void init_md_scan(PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds, const int mi_row,
                         const int mi_col, const int min_sq_size, const int max_sq_size,
                         const bool use_predetermined_depths) {
    const int sq_size = block_size_wide[mds->bsize];
    if (mi_col >= pcs->ppcs->av1_cm->mi_cols || mi_row >= pcs->ppcs->av1_cm->mi_rows) {
        mds->split_flag = false;
        mds->tot_shapes = 0;
        return;
    }

    // SQ/NSQ block(s) filter based on the SQ size
    const bool test_depth = (sq_size < min_sq_size) || (sq_size > max_sq_size) ? 0
        : use_predetermined_depths                                             ? mds->tot_shapes
                                                                               : 1;

    if (!use_predetermined_depths) {
        mds->split_flag = (sq_size > min_sq_size);
    }
    if (test_depth) {
        set_blocks_to_test(pcs, ctx, mds, mi_row, mi_col, mds->shapes, &mds->tot_shapes);
    } else {
        mds->tot_shapes = 0;
    }

    if (mds->split_flag) {
        const int mi_step = mi_size_wide[mds->bsize] / 2;
        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            const int x_idx = (i & 1) * mi_step;
            const int y_idx = (i >> 1) * mi_step;
            init_md_scan(pcs,
                         ctx,
                         mds->split[i],
                         mi_row + y_idx,
                         mi_col + x_idx,
                         min_sq_size,
                         max_sq_size,
                         use_predetermined_depths);
        }
    }
}

static void set_blocks_to_be_tested(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                    MdScan* mds, const bool use_predetermined_depths) {
    memset(ctx->tested_blk, false, sizeof(*(ctx->tested_blk)) * ctx->blocks_to_alloc);
    int min_sq_size = (ctx->depth_removal_ctrls.enabled && ctx->depth_removal_ctrls.disallow_below_64x64) ? 64
        : (ctx->depth_removal_ctrls.enabled && ctx->depth_removal_ctrls.disallow_below_32x32)             ? 32
        : (ctx->disallow_8x8 || (ctx->depth_removal_ctrls.enabled && ctx->depth_removal_ctrls.disallow_below_16x16))
        ? 16
        : ctx->disallow_4x4 ? 8
                            : 4;
    int max_sq_size = ctx->max_block_size;
    if (pcs->mimic_only_tx_4x4) {
        max_sq_size = MIN(max_sq_size, 8);
    } else if (scs->static_config.max_tx_size == 32) {
        max_sq_size = MIN(max_sq_size, 32);
    } else if (pcs->slice_type == I_SLICE) {
        max_sq_size = MIN(max_sq_size, 64);
    }
    // Safety check: Restrict min sq size so mode decision can always find at least one valid partition scheme
    min_sq_size = MIN(min_sq_size, scs->static_config.max_tx_size);
    assert(min_sq_size <= max_sq_size);

    init_md_scan(pcs,
                 ctx,
                 mds,
                 ctx->sb_origin_y >> 2,
                 ctx->sb_origin_x >> 2,
                 min_sq_size,
                 max_sq_size,
                 use_predetermined_depths);
}

static void set_child_to_be_tested(PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds, int e_depth,
                                   const int mi_row, const int mi_col) {
    const int sq_size = block_size_wide[mds->bsize];
    if (sq_size <= 4 || // 4x4 blocks have no children
        (sq_size == 8 && ctx->disallow_4x4) || (sq_size == 16 && ctx->disallow_8x8)) {
        return;
    }
    mds->split_flag   = true;
    const int mi_step = mi_size_wide[mds->bsize] / 2;
    for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
        const int x_idx           = (i & 1) * mi_step;
        const int y_idx           = (i >> 1) * mi_step;
        mds->split[i]->is_child   = true;
        mds->split[i]->split_flag = false;
        // Set tot_shapes to 1 because the actual number of shapes must be set later in
        // set_blocks_to_be_tested once the proper NSQ settings are applied.
        mds->split[i]->tot_shapes = 1;
        if (e_depth > 1) {
            set_child_to_be_tested(pcs, ctx, mds->split[i], e_depth - 1, mi_row + y_idx, mi_col + x_idx);
        }
    }
}

static void update_pred_th_offset(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree, int* s_depth,
                                  int* e_depth, int64_t* s_th_offset, int64_t* e_th_offset) {
    const int bwidth  = block_size_wide[pc_tree->bsize];
    const int bheight = block_size_high[pc_tree->bsize];
    assert(bwidth == bheight);
    const int sq_size = bwidth;

    if (ctx->depth_refinement_ctrls.cost_band_based_modulation) {
        uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];

        // cost-band-based modulation
        uint64_t max_cost = RDCOST(full_lambda, 16, ctx->depth_refinement_ctrls.max_cost_multiplier * bwidth * bheight);

        // For incomplete blocks, H/V partitions may be allowed, while square is not. In those cases, the selected depth
        // may not have a valid SQ cost, so we need to check that the SQ block is available before using the cost
        if (pc_tree->tested_blk[PART_N][0] && pc_tree->block_data[PART_N][0]->cost <= max_cost) {
            uint64_t band_size = max_cost / ctx->depth_refinement_ctrls.max_band_cnt;
            uint64_t band_idx  = pc_tree->block_data[PART_N][0]->cost / band_size;
            if (ctx->depth_refinement_ctrls.decrement_per_band[band_idx] == MAX_SIGNED_VALUE) {
                *s_depth = 0;
                *e_depth = 0;
            } else {
                *s_th_offset = -ctx->depth_refinement_ctrls.decrement_per_band[band_idx];
                *e_th_offset = -ctx->depth_refinement_ctrls.decrement_per_band[band_idx];
            }
        }
    }

    if (*s_depth) {
        const uint32_t lower_depth_split_cost_th = ctx->depth_refinement_ctrls.lower_depth_split_cost_th;
        // Skip testing NSQ shapes at parent depth if the rate cost of splitting is very low
        if (lower_depth_split_cost_th && pc_tree->parent->tested_blk[PART_N][0]) {
            const uint32_t full_lambda = ctx->hbd_md ? ctx->full_sb_lambda_md[EB_10_BIT_MD]
                                                     : ctx->full_sb_lambda_md[EB_8_BIT_MD];
            const uint64_t split_rate  = svt_aom_partition_rate_cost(pcs->ppcs,
                                                                    pc_tree->parent->bsize,
                                                                    pc_tree->parent->mi_row,
                                                                    pc_tree->parent->mi_col,
                                                                    ctx->md_rate_est_ctx,
                                                                    PARTITION_SPLIT,
                                                                    0,
                                                                    0);
            const uint64_t split_cost  = RDCOST(full_lambda, split_rate, 0);
            if (split_cost * 10000 < pc_tree->parent->block_data[PART_N][0]->cost * lower_depth_split_cost_th) {
                *s_depth = 0;
            }
        }
    }

    uint32_t split_cost_th = ctx->depth_refinement_ctrls.split_rate_th;
    // Skip testing child depth if the rate cost of splitting is high
    if (split_cost_th && pc_tree->tested_blk[PART_N][0]) {
        if (ctx->lpd0_ctrls.pd0_level > REGULAR_PD0) {
            // If LPD0 was used, use a safer threshold
            split_cost_th += 20;
        }
        const uint32_t full_lambda = ctx->hbd_md ? ctx->full_sb_lambda_md[EB_10_BIT_MD]
                                                 : ctx->full_sb_lambda_md[EB_8_BIT_MD];
        const uint64_t split_rate  = svt_aom_partition_rate_cost(pcs->ppcs,
                                                                pc_tree->bsize,
                                                                pc_tree->mi_row,
                                                                pc_tree->mi_col,
                                                                ctx->md_rate_est_ctx,
                                                                PARTITION_SPLIT,
                                                                0, // partition ctxs not updated in PD0
                                                                0);
        const uint64_t split_cost  = RDCOST(full_lambda, split_rate, 0);

        if (split_cost * 1000 > pc_tree->block_data[PART_N][0]->cost * split_cost_th) {
            *e_depth = 0;
        }
    }

    // Use info from ref. frames (if available)
    if (ctx->depth_refinement_ctrls.use_ref_info) {
        const bool is_ref_l0_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0);
        const bool is_ref_l1_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0);

        if (pcs->slice_type != I_SLICE && is_ref_l0_avail) {
            EbReferenceObject* ref_obj_l0 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;

            uint8_t sb_min_sq_size = ref_obj_l0->sb_min_sq_size[ctx->sb_index];
            uint8_t sb_max_sq_size = ref_obj_l0->sb_max_sq_size[ctx->sb_index];

            if (pcs->slice_type == B_SLICE && is_ref_l1_avail && pcs->ppcs->ref_list1_count_try) {
                EbReferenceObject* ref_obj_l1 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
                sb_min_sq_size                = MIN(sb_min_sq_size, ref_obj_l1->sb_min_sq_size[ctx->sb_index]);
                sb_max_sq_size                = MAX(sb_max_sq_size, ref_obj_l1->sb_max_sq_size[ctx->sb_index]);
            }

            if ((sq_size == 128 && pcs->scs->super_block_size == 128) ||
                (sq_size == 64 && pcs->scs->super_block_size == 64)) {
                if (sq_size == sb_min_sq_size && sq_size == sb_max_sq_size) {
                    *s_depth = 0;
                    *e_depth = 0;
                }
            }
        }
    }
}

static void is_parent_to_current_deviation_small(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree,
                                                 int64_t th_offset, int* s_depth) {
    if (pc_tree->parent->tested_blk[PART_N][0]) {
        int64_t s1_parent_to_current_th = (int64_t)ctx->depth_refinement_ctrls.s1_parent_to_current_th;
        int64_t s2_parent_to_current_th = (int64_t)ctx->depth_refinement_ctrls.s2_parent_to_current_th;

        if (ctx->depth_refinement_ctrls.q_weight) {
            uint32_t q_weight, q_weight_denom;
            svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.depths_qp_based_th_scaling,
                                                    &q_weight,
                                                    &q_weight_denom,
                                                    pcs->scs->static_config.qp);
            s1_parent_to_current_th = s1_parent_to_current_th == (uint8_t)~0
                ? MIN_SIGNED_VALUE
                : DIVIDE_AND_ROUND(s1_parent_to_current_th * q_weight, q_weight_denom);
            s2_parent_to_current_th = s2_parent_to_current_th == (uint8_t)~0
                ? MIN_SIGNED_VALUE
                : DIVIDE_AND_ROUND(s2_parent_to_current_th * q_weight, q_weight_denom);
        }

        s1_parent_to_current_th = s1_parent_to_current_th == MIN_SIGNED_VALUE
            ? MIN_SIGNED_VALUE
            : ctx->depth_refinement_ctrls.s1_parent_to_current_th + th_offset;
        s2_parent_to_current_th = s2_parent_to_current_th == MIN_SIGNED_VALUE
            ? MIN_SIGNED_VALUE
            : ctx->depth_refinement_ctrls.s2_parent_to_current_th + th_offset;

        const int      bwidth      = block_size_wide[pc_tree->bsize];
        const int      bheight     = block_size_high[pc_tree->bsize];
        const uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];

        uint64_t max_cost = ctx->depth_refinement_ctrls.parent_max_cost_th_mult
            ? RDCOST(full_lambda,
                     18000 * ctx->depth_refinement_ctrls.parent_max_cost_th_mult,
                     60 * ctx->depth_refinement_ctrls.parent_max_cost_th_mult * bwidth * bheight * 4)
            : 0;

        int64_t parent_to_current_deviation = (int64_t)(((int64_t)MAX(pc_tree->parent->block_data[PART_N][0]->cost, 1) -
                                                         (int64_t)MAX((pc_tree->block_data[PART_N][0]->cost * 4), 1)) *
                                                        100) /
            (int64_t)MAX((pc_tree->block_data[PART_N][0]->cost * 4), 1);

        if (parent_to_current_deviation >= s1_parent_to_current_th &&
            pc_tree->parent->block_data[PART_N][0]->cost >= max_cost) {
            *s_depth = 0;
        } else if (parent_to_current_deviation >= s2_parent_to_current_th) {
            *s_depth = -1;
        } else {
            *s_depth = MAX(*s_depth, -2);
        }
    } else {
        if (ctx->depth_refinement_ctrls.pd0_unavail_mode_depth == 0) {
            *s_depth = 0;
        } else if (ctx->depth_refinement_ctrls.pd0_unavail_mode_depth == 1) {
            *s_depth = MAX(*s_depth, -1);
        }
    }
}

static void is_child_to_current_deviation_small(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree,
                                                int64_t th_offset, int* e_depth) {
    uint64_t child_cost = 0;
    uint8_t  child_cnt  = 0;
    if (pc_tree->split[0]->tested_blk[PART_N][0]) {
        child_cost += pc_tree->split[0]->block_data[PART_N][0]->cost;
        child_cnt++;
    }
    if (pc_tree->split[1]->tested_blk[PART_N][0]) {
        child_cost += pc_tree->split[1]->block_data[PART_N][0]->cost;
        child_cnt++;
    }
    if (pc_tree->split[2]->tested_blk[PART_N][0]) {
        child_cost += pc_tree->split[2]->block_data[PART_N][0]->cost;
        child_cnt++;
    }
    if (pc_tree->split[3]->tested_blk[PART_N][0]) {
        child_cost += pc_tree->split[3]->block_data[PART_N][0]->cost;
        child_cnt++;
    }

    if (child_cnt) {
        int64_t e1_sub_to_current_th = (int64_t)ctx->depth_refinement_ctrls.e1_sub_to_current_th;
        int64_t e2_sub_to_current_th = (int64_t)ctx->depth_refinement_ctrls.e2_sub_to_current_th;

        if (ctx->depth_refinement_ctrls.q_weight) {
            uint32_t q_weight, q_weight_denom;
            svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.depths_qp_based_th_scaling,
                                                    &q_weight,
                                                    &q_weight_denom,
                                                    pcs->scs->static_config.qp);
            e1_sub_to_current_th = e1_sub_to_current_th == (uint8_t)~0
                ? MIN_SIGNED_VALUE
                : DIVIDE_AND_ROUND(e1_sub_to_current_th * q_weight, q_weight_denom);
            e2_sub_to_current_th = e2_sub_to_current_th == (uint8_t)~0
                ? MIN_SIGNED_VALUE
                : DIVIDE_AND_ROUND(e2_sub_to_current_th * q_weight, q_weight_denom);
        }

        e1_sub_to_current_th = e1_sub_to_current_th == MIN_SIGNED_VALUE ? MIN_SIGNED_VALUE
                                                                        : e1_sub_to_current_th + th_offset;

        e2_sub_to_current_th = e2_sub_to_current_th == MIN_SIGNED_VALUE ? MIN_SIGNED_VALUE
                                                                        : e2_sub_to_current_th + th_offset;

        int64_t child_to_current_deviation;
        child_cost                      = (child_cost / child_cnt) * 4;
        const uint32_t full_lambda      = ctx->hbd_md ? ctx->full_sb_lambda_md[EB_10_BIT_MD]
                                                      : ctx->full_sb_lambda_md[EB_8_BIT_MD];
        const uint64_t child_split_rate = svt_aom_partition_rate_cost(pcs->ppcs,
                                                                      pc_tree->bsize,
                                                                      pc_tree->mi_row,
                                                                      pc_tree->mi_col,
                                                                      ctx->md_rate_est_ctx,
                                                                      PARTITION_SPLIT,
                                                                      0, // partition ctxs not updated in pd0
                                                                      0);
        child_cost += RDCOST(full_lambda, child_split_rate, 0);
        child_to_current_deviation =
            (int64_t)(((int64_t)MAX(child_cost, 1) - (int64_t)MAX(pc_tree->block_data[PART_N][0]->cost, 1)) * 100) /
            (int64_t)(MAX(pc_tree->block_data[PART_N][0]->cost, 1));

        if (child_to_current_deviation >= e1_sub_to_current_th) {
            *e_depth = 0;
        } else if (child_to_current_deviation >= e2_sub_to_current_th) {
            *e_depth = 1;
        } else {
            *e_depth = MIN(*e_depth, 2);
        }
    } else {
        if (ctx->depth_refinement_ctrls.pd0_unavail_mode_depth == 0) {
            *e_depth = 0;
        } else if (ctx->depth_refinement_ctrls.pd0_unavail_mode_depth == 1) {
            *e_depth = MIN(*e_depth, 1);
        }
    }
}

static void set_start_end_depth(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree,
                                const int max_pd0_size, const int min_pd0_size, int* s_depth_ret, int* e_depth_ret) {
    const int sq_size = block_size_wide[pc_tree->bsize];

    int s_depth = ctx->depth_refinement_ctrls.mode == PD0_DEPTH_PRED_PART_ONLY ? 0 : -2;
    int e_depth = ctx->depth_refinement_ctrls.mode == PD0_DEPTH_PRED_PART_ONLY ? 0 : 2;
    if (s_depth == 0 && e_depth == 0) {
        *s_depth_ret = 0;
        *e_depth_ret = 0;
        return;
    }

    // 4x4 blocks have no children
    if (sq_size == 4) {
        e_depth = 0;
    }
    // Check that the start and end depth are in allowed range, given other features
    // which restrict allowable depths
    if (ctx->disallow_8x8) {
        e_depth = (sq_size <= 16) ? 0
            : (sq_size == 32)     ? MIN(1, e_depth)
            : (sq_size == 64)     ? MIN(2, e_depth)
            : (sq_size == 128)    ? MIN(3, e_depth)
                                  : e_depth;
    } else if (ctx->disallow_4x4) {
        e_depth = (sq_size == 8) ? 0 : (sq_size == 16) ? MIN(1, e_depth) : (sq_size == 32) ? MIN(2, e_depth) : e_depth;
    }
    if (ctx->depth_removal_ctrls.enabled) {
        if (ctx->depth_removal_ctrls.disallow_below_64x64) {
            e_depth = (sq_size <= 64) ? 0 : (sq_size == 128) ? MIN(1, e_depth) : e_depth;
        } else if (ctx->depth_removal_ctrls.disallow_below_32x32) {
            e_depth = (sq_size <= 32) ? 0
                : (sq_size == 64)     ? MIN(1, e_depth)
                : (sq_size == 128)    ? MIN(2, e_depth)
                                      : e_depth;
        } else if (ctx->depth_removal_ctrls.disallow_below_16x16) {
            e_depth = (sq_size <= 16) ? 0
                : (sq_size == 32)     ? MIN(1, e_depth)
                : (sq_size == 64)     ? MIN(2, e_depth)
                : (sq_size == 128)    ? MIN(3, e_depth)
                                      : e_depth;
        }
    }
    int32_t max_sq_size = ctx->max_block_size;
    if (pcs->scs->static_config.max_tx_size == 32) {
        max_sq_size = MIN(max_sq_size, 32);
    }

    if (sq_size == max_sq_size) {
        s_depth = 0;
    } else if (s_depth == -2 && sq_size << 1 == max_sq_size) {
        s_depth = -1;
    }
    uint8_t add_parent_depth = 1;
    uint8_t add_sub_depth    = 1;
    if (ctx->depth_refinement_ctrls.mode == PD0_DEPTH_ADAPTIVE && (s_depth != 0 || e_depth != 0)) {
        add_parent_depth = 0;
        add_sub_depth    = 0;

        if (ctx->depth_refinement_ctrls.limit_max_min_to_pd0 &&
            (max_pd0_size / min_pd0_size) > ctx->depth_refinement_ctrls.limit_max_min_to_pd0) {
            // If PD0 selected multiple depths, don't test depths above the largest or below the smallest block sizes
            if (sq_size == max_pd0_size) {
                s_depth = 0;
            }
            if (sq_size == min_pd0_size) {
                e_depth = 0;
            }

            if (s_depth == -2 && sq_size << 1 == max_pd0_size) {
                s_depth = -1;
            }

            if (e_depth == 2 && sq_size >> 1 == min_pd0_size) {
                e_depth = 1;
            }
        }

        if (ctx->depth_refinement_ctrls.coeff_lvl_modulation) {
            if (pcs->slice_type != I_SLICE && pcs->coeff_lvl != LOW_LVL && pcs->coeff_lvl != VLOW_LVL) {
                s_depth = MAX(s_depth, -1);
                e_depth = MIN(e_depth, 1);
            }
        }

        int64_t s_th_offset = 0;
        int64_t e_th_offset = 0;

        update_pred_th_offset(pcs, ctx, pc_tree, &s_depth, &e_depth, &s_th_offset, &e_th_offset);
        if (s_depth &&
            // Check tested_blk b/c use block's cost inside
            pc_tree->tested_blk[PART_N][0] && sq_size < ((pcs->scs->seq_header.sb_size == BLOCK_128X128) ? 128 : 64)) {
            is_parent_to_current_deviation_small(pcs, ctx, pc_tree, s_th_offset, &s_depth);
            if (s_depth) {
                add_parent_depth = 1;
            }
        }

        if (e_depth &&
            // Check tested_blk b/c use block's cost inside
            pc_tree->tested_blk[PART_N][0] && sq_size > 4) {
            is_child_to_current_deviation_small(pcs, ctx, pc_tree, e_th_offset, &e_depth);
            if (e_depth) {
                add_sub_depth = 1;
            }
        }
    }

    *s_depth_ret = add_parent_depth ? s_depth : 0;
    *e_depth_ret = add_sub_depth ? e_depth : 0;
}

// pc_tree contains the chosen prediction structure from the first/previous PD pass.
// The blocks/depths to be tested in the next PD pass will be written to/updated in mds.
static int refine_depth(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree, MdScan* mds,
                        const int mi_row, const int mi_col, const int max_pd0_size, const int min_pd0_size) {
    // add check that blocks are in bounds
    if (mi_col >= pcs->ppcs->av1_cm->mi_cols || mi_row >= pcs->ppcs->av1_cm->mi_rows) {
        mds->split_flag = false;
        mds->is_child   = false;
        mds->tot_shapes = 0;
        return 0;
    }

    int s_depth = 0;
    if (pc_tree->partition != PARTITION_SPLIT) {
        // Add current pred depth to be tested. tot_shapes = 1 signals to test this depth;
        // the blocks to be tested will be updated in set_blocks_to_be_tested after
        // proper PD1 settings (esp. for NSQ) have been updated.
        mds->split_flag = false;
        mds->is_child   = false;
        mds->tot_shapes = 1;

        int e_depth = 0;
        set_start_end_depth(pcs, ctx, pc_tree, max_pd0_size, min_pd0_size, &s_depth, &e_depth);

        if (e_depth || s_depth) {
            ctx->pred_depth_only = false;
        }

        if (e_depth) {
            set_child_to_be_tested(pcs, ctx, mds, e_depth, mi_row, mi_col);
        }
    } else {
        // Set flags to signal this depth shouldn't be tested (unless later updated via depth refinement)
        mds->split_flag = true;
        mds->tot_shapes = 0;
        mds->is_child   = false;

        const int mi_step = mi_size_wide[mds->bsize] / 2;
        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            const int x_idx        = (i & 1) * mi_step;
            const int y_idx        = (i >> 1) * mi_step;
            int       s_depth_temp = refine_depth(
                pcs, ctx, pc_tree->split[i], mds->split[i], mi_row + y_idx, mi_col + x_idx, max_pd0_size, min_pd0_size);
            s_depth = MIN(s_depth, s_depth_temp);
        }

        const int sq_size        = block_size_wide[mds->bsize];
        uint8_t   is_blk_allowed = pcs->slice_type != I_SLICE ? 1 : (sq_size < 128) ? 1 : 0;
        if (s_depth && is_blk_allowed) {
            // Add current pred depth to be tested. tot_shapes = 1 signals to test this depth;
            // the blocks to be tested will be updated in set_blocks_to_be_tested after
            // proper PD1 settings (esp. for NSQ) have been updated.
            mds->tot_shapes = 1;
            s_depth++;
        }
    }

    return s_depth;
}

static void get_max_min_pd0_depths(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree, const int mi_row,
                                   const int mi_col, int* max_pd0_size_out, int* min_pd0_size_out) {
    // check that blocks are in bounds
    if (mi_col >= pcs->ppcs->av1_cm->mi_cols || mi_row >= pcs->ppcs->av1_cm->mi_rows) {
        return;
    }

    if (pc_tree->partition != PARTITION_SPLIT) {
        const int sq_size = block_size_wide[pc_tree->bsize];
        if (sq_size > *max_pd0_size_out) {
            *max_pd0_size_out = sq_size;
        }
        if (sq_size < *min_pd0_size_out) {
            *min_pd0_size_out = sq_size;
        }
    } else {
        const int mi_step = mi_size_wide[pc_tree->bsize] / 2;
        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            const int x_idx = (i & 1) * mi_step;
            const int y_idx = (i >> 1) * mi_step;
            get_max_min_pd0_depths(
                pcs, ctx, pc_tree->split[i], mi_row + y_idx, mi_col + x_idx, max_pd0_size_out, min_pd0_size_out);
        }
    }
}

static void perform_pred_depth_refinement(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree,
                                          MdScan* mds, const int mi_row, const int mi_col) {
    // Get max/min PD0 selected block sizes
    int max_pd0_size = 0;
    int min_pd0_size = 255;
    if (ctx->depth_refinement_ctrls.limit_max_min_to_pd0) {
        get_max_min_pd0_depths(pcs, ctx, pc_tree, mi_row, mi_col, &max_pd0_size, &min_pd0_size);
    }

    ctx->pred_depth_only = true;

    refine_depth(pcs, ctx, pc_tree, mds, mi_row, mi_col, max_pd0_size, min_pd0_size);
}

void mdc_init_qp_update(PictureControlSet* pcs);
void svt_aom_init_enc_dec_segement(PictureParentControlSet* ppcs);

static void recode_loop_decision_maker(PictureControlSet* pcs, SequenceControlSet* scs, bool* do_recode) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    EncodeContext* const     enc_ctx = ppcs->scs->enc_ctx;
    RATE_CONTROL* const      rc      = &(enc_ctx->rc);
    bool                     loop    = false;
    FrameHeader*             frm_hdr = &ppcs->frm_hdr;
    int32_t                  q       = frm_hdr->quantization_params.base_q_idx;
    if (ppcs->loop_count == 0) {
        ppcs->q_low  = ppcs->bottom_index;
        ppcs->q_high = ppcs->top_index;
    }

    // Update q and decide whether to do a recode loop
    recode_loop_update_q(ppcs,
                         &loop,
                         &q,
                         &ppcs->q_low,
                         &ppcs->q_high,
                         ppcs->top_index,
                         ppcs->bottom_index,
                         &ppcs->undershoot_seen,
                         &ppcs->overshoot_seen,
                         &ppcs->low_cr_seen,
                         ppcs->loop_count);

    // Special case for overlay frame.
    if (loop && ppcs->is_overlay && ppcs->projected_frame_size < rc->max_frame_bandwidth) {
        loop = false;
    }
    *do_recode = loop;

    if (*do_recode) {
        ppcs->loop_count++;

        frm_hdr->quantization_params.base_q_idx = (uint8_t)CLIP3(
            (int32_t)quantizer_to_qindex[scs->static_config.min_qp_allowed],
            (int32_t)quantizer_to_qindex[scs->static_config.max_qp_allowed],
            q);

        ppcs->picture_qp = (uint8_t)CLIP3((int32_t)scs->static_config.min_qp_allowed,
                                          (int32_t)scs->static_config.max_qp_allowed,
                                          (frm_hdr->quantization_params.base_q_idx + 2) >> 2);

        // set initial SB base_q_idx values
        pcs->ppcs->frm_hdr.delta_q_params.delta_q_present = 0;
        for (int sb_addr = 0; sb_addr < pcs->sb_total_count; ++sb_addr) {
            SuperBlock* sb_ptr = pcs->sb_ptr_array[sb_addr];
            sb_ptr->qindex     = frm_hdr->quantization_params.base_q_idx;
        }

        // adjust SB qindex based on variance
        if (scs->static_config.enable_variance_boost) {
            svt_av1_variance_adjust_qp(pcs);
        }

        // 2pass QPM with tpl_la
        if (scs->static_config.aq_mode == 2 && ppcs->tpl_ctrls.enable && ppcs->r0 != 0) {
            svt_aom_sb_qp_derivation_tpl_la(pcs);
        }

        if (pcs->ppcs->frm_hdr.delta_q_params.delta_q_present && pcs->ppcs->frm_hdr.delta_q_params.delta_q_res != 1) {
            // adjust delta q res and normalize superblock delta q values to reduce signaling overhead
            svt_av1_normalize_sb_delta_q(pcs);
        }
    } else {
        ppcs->loop_count = 0;
    }
}

/* for debug/documentation purposes: list all features assumed off for light pd1*/
static void exaustive_light_pd1_features(ModeDecisionContext* md_ctx, PictureParentControlSet* ppcs,
                                         uint8_t use_light_pd1, uint8_t debug_lpd1_features) {
    if (debug_lpd1_features) {
        uint8_t light_pd1;

        // Use light-PD1 path if the assumed features are off
        if (md_ctx->obmc_ctrls.enabled == 0 && md_ctx->md_allow_intrabc == 0 && md_ctx->hbd_md == 0 &&
            md_ctx->ifs_ctrls.level == IFS_OFF && ppcs->frm_hdr.allow_warped_motion == 0 &&
            md_ctx->inter_intra_comp_ctrls.enabled == 0 && md_ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx == 0 &&
            md_ctx->spatial_sse_ctrls.level == SSSE_OFF && md_ctx->md_sq_me_ctrls.enabled == 0 &&
            md_ctx->md_pme_ctrls.enabled == 0 && md_ctx->txt_ctrls.enabled == 0 && md_ctx->unipred3x3_injection == 0 &&
            md_ctx->bipred3x3_ctrls.enabled == 0 && md_ctx->inter_comp_ctrls.tot_comp_types == 1 &&
            md_ctx->obmc_ctrls.enabled == 0 && md_ctx->filter_intra_ctrls.enabled == 0 &&
            md_ctx->new_nearest_near_comb_injection == 0 && md_ctx->md_palette_level == 0 &&
            ppcs->gm_ctrls.enabled == 0 &&
            // If TXS enabled at picture level, there are necessary context updates that must be added to LPD1
            ppcs->frm_hdr.tx_mode != TX_MODE_SELECT && md_ctx->txs_ctrls.enabled == 0 && md_ctx->pred_depth_only &&
            md_ctx->md_disallow_nsq_search == true && md_ctx->disallow_4x4 == true &&
            ppcs->scs->super_block_size == 64 && ppcs->ref_list0_count_try == 1 && ppcs->ref_list1_count_try == 1 &&
            md_ctx->cfl_ctrls.enabled == 0 && md_ctx->uv_ctrls.uv_mode == CHROMA_MODE_1) {
            light_pd1 = 1;
        } else {
            light_pd1 = 0;
        }

        svt_aom_assert_err(light_pd1 == use_light_pd1, "Warning: light PD1 feature assumption is broken \n");
    }
}

/* Light-PD1 classifier used when cost/coeff info is available.  If PD0 is skipped, or the trasnsform is
not performed, a separate detector (lpd1_detector_skip_pd0) is used. */
static void lpd1_detector_post_pd0(PictureControlSet* pcs, ModeDecisionContext* md_ctx, const PC_TREE* const pc_tree) {
    for (int pd1_lvl = LPD1_LEVELS - 1; pd1_lvl > REGULAR_PD1; pd1_lvl--) {
        if (pd1_lvl <= (md_ctx->pd1_lvl_refinement - 1)) {
            break;
        }
        if (md_ctx->lpd1_ctrls.pd1_level == pd1_lvl) {
            if (md_ctx->lpd1_ctrls.use_lpd1_detector[pd1_lvl]) {
                // Use info from ref frames (if available)
                if (md_ctx->lpd1_ctrls.use_ref_info[pd1_lvl] && pcs->slice_type != I_SLICE) {
                    // Get list 0 refs' info
                    uint8_t l0_was_intra = 0;
                    uint8_t l0_refs      = 0;
                    // the frame size of reference pics are different if enable reference scaling.
                    // sb info can not be reused because super blocks are mismatched, so we set
                    // the reference pic unavailable to avoid using wrong info
                    const bool is_ref_l0_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0);
                    if (pcs->ppcs->ref_list0_count_try && is_ref_l0_avail) {
                        EbReferenceObject* ref_obj_l0 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
                        // flat ipp should not use hierarchical concept
                        if (ref_obj_l0->tmp_layer_idx <= pcs->temporal_layer_index || pcs->scs->use_flat_ipp) {
                            l0_was_intra += ref_obj_l0->sb_intra[md_ctx->sb_index];
                            l0_refs++;
                        }
                    }

                    // Get list 1 refs' info
                    uint8_t    l1_was_intra    = 0;
                    uint8_t    l1_refs         = 0;
                    const bool is_ref_l1_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0);
                    if (pcs->ppcs->ref_list1_count_try && is_ref_l1_avail) {
                        EbReferenceObject* ref_obj_l1 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
                        // flat ipp should not use hierarchical concept
                        if (ref_obj_l1->tmp_layer_idx <= pcs->temporal_layer_index || pcs->scs->use_flat_ipp) {
                            l1_was_intra += ref_obj_l1->sb_intra[md_ctx->sb_index];
                            l1_refs++;
                        }
                    }

                    if ((l0_refs || l1_refs) && (!l0_refs || l0_was_intra) && (!l1_refs || l1_was_intra)) {
                        md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        continue;
                    } else if ((l0_refs && l0_was_intra) || (l1_refs && l1_was_intra)) {
                        md_ctx->lpd1_ctrls.cost_th_dist[pd1_lvl] >>= 2;
                        md_ctx->lpd1_ctrls.cost_th_rate[pd1_lvl] >>= 2;
                        md_ctx->lpd1_ctrls.me_8x8_cost_variance_th[pd1_lvl] >>= 1;
                        md_ctx->lpd1_ctrls.nz_coeff_th[pd1_lvl] >>= 1;
                    }
                }

                /* Use the cost and coeffs of the 64x64 block to avoid looping over all tested blocks to find
                the selected partitioning. */
                const uint64_t pd0_cost = pc_tree->rdc.rd_cost;
                // If block was not tested in PD0, won't have coeff info, so set to max and base detection on cost only (which is set
                // even if 64x64 block is not tested)
                const uint32_t nz_coeffs = pc_tree->tested_blk[PART_N][0] ? pc_tree->block_data[PART_N][0]->cnt_nz_coeff
                                                                          : (uint32_t)~0;

                const uint32_t lambda = md_ctx->full_sb_lambda_md[EB_8_BIT_MD]; // light-PD1 assumes 8-bit MD
                const uint32_t rate   = md_ctx->lpd1_ctrls.cost_th_rate[pd1_lvl];
                const uint32_t dist   = md_ctx->lpd1_ctrls.cost_th_dist[pd1_lvl];
                /* dist << 14 is equivalent to 64 * 64 * 4 * dist (64 * 64 so the distortion is the per-pixel SSD) and 4 because
                the distortion of the 64x64 block is shifted by 2 (same as multiplying by 4) in perform_tx_light_pd0. */
                const uint64_t low_th      = RDCOST(lambda, rate, (uint64_t)dist << 14);
                const uint16_t nz_coeff_th = md_ctx->lpd1_ctrls.nz_coeff_th[pd1_lvl];
                // If the PD0 cost is very high and the number of non-zero coeffs is high, the block is difficult, so should use regular PD1
                if (pd0_cost > low_th && nz_coeffs >= nz_coeff_th) {
                    md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                }

                // If the best PD0 mode was INTER, check the MV length
                if (pc_tree->tested_blk[PART_N][0] && is_inter_mode(pc_tree->block_data[PART_N][0]->block_mi.mode) &&
                    md_ctx->lpd1_ctrls.max_mv_length[pd1_lvl] != (uint16_t)~0) {
                    BlkStruct*     blk_ptr       = pc_tree->block_data[PART_N][0];
                    const uint16_t max_mv_length = md_ctx->lpd1_ctrls.max_mv_length[pd1_lvl];

                    // unipred MVs always stored in idx0
                    if (blk_ptr->block_mi.mv[0].x > max_mv_length || blk_ptr->block_mi.mv[0].y > max_mv_length) {
                        md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                    }
                    if (has_second_ref(&blk_ptr->block_mi)) {
                        if (blk_ptr->block_mi.mv[1].x > max_mv_length || blk_ptr->block_mi.mv[1].y > max_mv_length) {
                            md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        }
                    }
                }

                if (pcs->slice_type != I_SLICE) {
                    /* me_8x8_cost_variance_th is shifted by 5 then mulitplied by 73 minus pic_qp.  Therefore, the TH must be less than
                        (((uint32_t)~0) >> 2) to avoid overflow issues from the multiplication. */
                    if (md_ctx->lpd1_ctrls.me_8x8_cost_variance_th[pd1_lvl] < (((uint32_t)~0) >> 2) &&
                        pcs->ppcs->me_8x8_cost_variance[md_ctx->sb_index] >
                            (md_ctx->lpd1_ctrls.me_8x8_cost_variance_th[pd1_lvl] >> 5) * (73 - pcs->ppcs->picture_qp)) {
                        md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                    }
                }
            }
        }
    }
}

/* Light-PD1 classifier used when cost/coeff info is unavailable.  If PD0 is skipped, or the trasnsform is
not performed, this detector is used (else lpd1_detector_post_pd0() is used). */
static void lpd1_detector_skip_pd0(PictureControlSet* pcs, ModeDecisionContext* md_ctx, uint32_t pic_width_in_sb) {
    const uint16_t left_sb_index = md_ctx->sb_index - 1;
    const uint16_t top_sb_index  = md_ctx->sb_index - (uint16_t)pic_width_in_sb;

    for (int pd1_lvl = LPD1_LEVELS - 1; pd1_lvl > REGULAR_PD1; pd1_lvl--) {
        if (pd1_lvl <= (md_ctx->pd1_lvl_refinement - 1)) {
            break;
        }
        if (md_ctx->lpd1_ctrls.pd1_level == pd1_lvl) {
            if (md_ctx->lpd1_ctrls.use_lpd1_detector[pd1_lvl]) {
                // Use info from ref. frames (if available)
                if (md_ctx->lpd1_ctrls.use_ref_info[pd1_lvl] && pcs->slice_type != I_SLICE) {
                    // Keep a complexity score for the SB, based on available information.
                    // If the score is high, then reduce the lpd1_level to be used
                    int16_t score = 0;
                    uint8_t refs  = 0;

                    // Get list 0 refs' info
                    // the frame size of reference pics are different if enable reference scaling.
                    // sb info can not be reused because super blocks are mismatched, so we set
                    // the reference pic unavailable to avoid using wrong info
                    const bool is_ref_l0_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0);
                    if (pcs->ppcs->ref_list0_count_try && is_ref_l0_avail) {
                        EbReferenceObject* ref_obj_l0 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
                        // flat ipp should not use hierarchical concept
                        if (ref_obj_l0->tmp_layer_idx <= pcs->temporal_layer_index || pcs->scs->use_flat_ipp) {
                            if (ref_obj_l0->slice_type != I_SLICE) {
                                if (ref_obj_l0->sb_intra[md_ctx->sb_index]) {
                                    score += 5;
                                }
                                if (!ref_obj_l0->sb_skip[md_ctx->sb_index]) {
                                    score += 5;
                                }
                                if (pcs->ppcs->me_64x64_distortion[md_ctx->sb_index] >
                                    (ref_obj_l0->sb_me_64x64_dist[md_ctx->sb_index] * 3)) {
                                    score += 5;
                                }
                                if (pcs->ppcs->me_8x8_cost_variance[md_ctx->sb_index] >
                                    (ref_obj_l0->sb_me_8x8_cost_var[md_ctx->sb_index] * 3)) {
                                    score += 5;
                                }
                            } else {
                                score += 10;
                            }

                            refs++;
                        }
                    }

                    // Get list 1 refs' info
                    const bool is_ref_l1_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0);
                    if (pcs->ppcs->ref_list1_count_try && is_ref_l1_avail) {
                        EbReferenceObject* ref_obj_l1 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
                        // flat ipp should not use hierarchical concept
                        if (ref_obj_l1->tmp_layer_idx <= pcs->temporal_layer_index || pcs->scs->use_flat_ipp) {
                            if (ref_obj_l1->slice_type != I_SLICE) {
                                if (ref_obj_l1->sb_intra[md_ctx->sb_index]) {
                                    score += 5;
                                }
                                if (!ref_obj_l1->sb_skip[md_ctx->sb_index]) {
                                    score += 5;
                                }
                                if (pcs->ppcs->me_64x64_distortion[md_ctx->sb_index] >
                                    (ref_obj_l1->sb_me_64x64_dist[md_ctx->sb_index] * 3)) {
                                    score += 5;
                                }
                                if (pcs->ppcs->me_8x8_cost_variance[md_ctx->sb_index] >
                                    (ref_obj_l1->sb_me_8x8_cost_var[md_ctx->sb_index] * 3)) {
                                    score += 5;
                                }
                            } else {
                                score += 10;
                            }

                            refs++;
                        }
                    }

                    if (refs && score >= 10 * refs) {
                        md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        continue;
                    }
                }

                // I_SLICE doesn't have ME info
                if (pcs->slice_type != I_SLICE) {
                    // If the SB origin of one dimension is zero, then this SB is the first block in a row/column, so won't have neighbours
                    if (md_ctx->sb_origin_x == 0 || md_ctx->sb_origin_y == 0) {
                        if (pcs->ppcs->me_64x64_distortion[md_ctx->sb_index] >
                            md_ctx->lpd1_ctrls.skip_pd0_edge_dist_th[pd1_lvl]) {
                            md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        }

                        /* me_8x8_cost_variance_th is shifted by 5 then mulitplied by 73 minus pic_qp.  Therefore, the TH must be less than
                            (((uint32_t)~0) >> 2) to avoid overflow issues from the multiplication. */
                        if (md_ctx->lpd1_ctrls.me_8x8_cost_variance_th[pd1_lvl] < (((uint32_t)~0) >> 2) &&
                            pcs->ppcs->me_8x8_cost_variance[md_ctx->sb_index] >
                                (md_ctx->lpd1_ctrls.me_8x8_cost_variance_th[pd1_lvl] >> 5) *
                                    (73 - pcs->ppcs->picture_qp)) {
                            md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        }
                    } else {
                        if (md_ctx->lpd1_ctrls.skip_pd0_me_shift[pd1_lvl] != (uint16_t)~0 &&
                            pcs->ppcs->me_64x64_distortion[md_ctx->sb_index] >
                                ((pcs->ppcs->me_64x64_distortion[left_sb_index] +
                                  pcs->ppcs->me_64x64_distortion[top_sb_index])
                                 << md_ctx->lpd1_ctrls.skip_pd0_me_shift[pd1_lvl])) {
                            md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        } else if (md_ctx->lpd1_ctrls.skip_pd0_me_shift[pd1_lvl] != (uint16_t)~0 &&
                                   pcs->ppcs->me_8x8_cost_variance[md_ctx->sb_index] >
                                       ((pcs->ppcs->me_8x8_cost_variance[left_sb_index] +
                                         pcs->ppcs->me_8x8_cost_variance[top_sb_index])
                                        << md_ctx->lpd1_ctrls.skip_pd0_me_shift[pd1_lvl])) {
                            md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                        } else if (md_ctx->lpd1_ctrls.use_ref_info[pd1_lvl]) {
                            // Use info from neighbouring SBs
                            if (pcs->sb_intra[left_sb_index] && pcs->sb_intra[top_sb_index]) {
                                md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                            } else if (!pcs->sb_skip[left_sb_index] && !pcs->sb_skip[top_sb_index] &&
                                       (pcs->sb_intra[left_sb_index] || pcs->sb_intra[top_sb_index])) {
                                md_ctx->lpd1_ctrls.pd1_level = pd1_lvl - 1;
                            }
                        }
                    }
                }
            }
        }
    }
}
#if FTR_VLPD0 // classifier
static void lpd0_detector_allintra(PictureControlSet* pcs, ModeDecisionContext* md_ctx) {
    if (md_ctx->lpd0_ctrls.pd0_level != VERY_LIGHT_PD0) {
        return;
    }

    uint16_t* sb_var = pcs->ppcs->variance[md_ctx->sb_index];

    // Variance accumulation
    int32_t var64 = sb_var[ME_TIER_ZERO_PU_64x64];

    int32_t var32 = 0;
    for (int i = ME_TIER_ZERO_PU_32x32_0; i <= ME_TIER_ZERO_PU_32x32_3; ++i) {
        var32 += sb_var[i];
    }

    int32_t var16 = 0;
    for (int i = ME_TIER_ZERO_PU_16x16_0; i <= ME_TIER_ZERO_PU_16x16_15; ++i) {
        var16 += sb_var[i];
    }

    // Normalize per block
    var32 >>= 2; // 4 x 32x32
    var16 >>= 4; // 16 x 16x16

    // Normalize per pixel
    const int32_t scale_32 = (64 * 64) / (32 * 32); // 4
    const int32_t scale_16 = (64 * 64) / (16 * 16); // 16

    int32_t norm_v64 = var64;
    int32_t norm_v32 = var32 * scale_32;
    int32_t norm_v16 = var16 * scale_16;

    // QP-scaled thresholds
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.lpd0_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->scs->static_config.qp);

    // Threshold for detecting lack of a dominant depth
    int32_t delta_var_th = 7500;

    delta_var_th = DIVIDE_AND_ROUND(delta_var_th * q_weight, q_weight_denom);

    if (ABS(norm_v32 - norm_v64) < delta_var_th && ABS(norm_v16 - norm_v32) < delta_var_th) {
        md_ctx->lpd0_ctrls.pd0_level--;
    }
}
#endif
/* Light-PD0 classifier. */
static void lpd0_detector(PictureControlSet* pcs, ModeDecisionContext* md_ctx, uint32_t pic_width_in_sb) {
    Lpd0Ctrls* lpd0_ctrls = &md_ctx->lpd0_ctrls;

    for (int pd0_lvl = LPD0_LEVELS - 1; pd0_lvl > REGULAR_PD0; pd0_lvl--) {
        if (lpd0_ctrls->pd0_level == pd0_lvl) {
            // VERY_LIGHT_PD0 is not supported for I_SLICE or when transition_present because VERY_LIGHT_PD0
            // only supports INTER compensation
            if ((pcs->slice_type == I_SLICE || pcs->ppcs->transition_present == 1) && pd0_lvl == VERY_LIGHT_PD0) {
                lpd0_ctrls->pd0_level = pd0_lvl - 1;
                continue;
            }

            if (lpd0_ctrls->use_lpd0_detector[pd0_lvl]) {
                if (lpd0_ctrls->use_ref_info[pd0_lvl] && pcs->slice_type != I_SLICE) {
                    // Get list 0 refs' info
                    uint8_t l0_was_intra = 0;
                    uint8_t l0_refs      = 0;
                    // the frame size of reference pics are different if enable reference scaling.
                    // sb info can not be reused because super blocks are mismatched, so we set
                    // the reference pic unavailable to avoid using wrong info
                    const bool is_ref_l0_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0);
                    if (pcs->ppcs->ref_list0_count_try && is_ref_l0_avail) {
                        EbReferenceObject* ref_obj_l0 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
                        if (ref_obj_l0->tmp_layer_idx <= pcs->temporal_layer_index || pcs->scs->use_flat_ipp) {
                            l0_was_intra += ref_obj_l0->sb_intra[md_ctx->sb_index];
                            l0_refs++;
                        }
                    }

                    // Get list 1 refs' info
                    uint8_t    l1_was_intra    = 0;
                    uint8_t    l1_refs         = 0;
                    const bool is_ref_l1_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0);
                    if (pcs->ppcs->ref_list1_count_try && is_ref_l1_avail) {
                        EbReferenceObject* ref_obj_l1 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
                        if (ref_obj_l1->tmp_layer_idx <= pcs->temporal_layer_index || pcs->scs->use_flat_ipp) {
                            l1_was_intra += ref_obj_l1->sb_intra[md_ctx->sb_index];
                            l1_refs++;
                        }
                    }

                    // use_ref_info level 1 (safest)
                    if (lpd0_ctrls->use_ref_info[pd0_lvl] == 1) {
                        if ((l0_refs && l0_was_intra) || (l1_refs && l1_was_intra)) {
                            lpd0_ctrls->pd0_level = pd0_lvl - 1;
                            continue;
                        }
                    }
                    // use_ref_info level 2
                    else if (lpd0_ctrls->use_ref_info[pd0_lvl] == 2) {
                        if ((l0_refs || l1_refs) && (!l0_refs || l0_was_intra) && (!l1_refs || l1_was_intra)) {
                            lpd0_ctrls->pd0_level = pd0_lvl - 1;
                            continue;
                        }
                    }
                    // use_ref_info level 3 (most aggressive)
                    else {
                        if ((l0_refs || l1_refs) && (!l0_refs || l0_was_intra) && (!l1_refs || l1_was_intra) &&
                            pcs->ref_intra_percentage > MAX(1, 50 - (pcs->ppcs->picture_qp >> 1))) {
                            lpd0_ctrls->pd0_level = pd0_lvl - 1;
                            continue;
                        }
                    }
                }

                // I_SLICE doesn't have ME info
                if (pcs->slice_type != I_SLICE) {
                    PictureParentControlSet* ppcs                 = pcs->ppcs;
                    const uint16_t           sb_index             = md_ctx->sb_index;
                    const uint32_t           me_8x8_cost_variance = ppcs->me_8x8_cost_variance[sb_index];
                    const uint32_t           me_64x64_distortion  = ppcs->me_64x64_distortion[sb_index];
                    /* me_8x8_cost_variance_th is shifted by 5 then mulitplied by the pic QP (max 63).  Therefore, the TH must be less than
                       (((uint32_t)~0) >> 1) to avoid overflow issues from the multiplication. */
                    if (lpd0_ctrls->me_8x8_cost_variance_th[pd0_lvl] < (((uint32_t)~0) >> 1) &&
                        me_8x8_cost_variance > (lpd0_ctrls->me_8x8_cost_variance_th[pd0_lvl] >> 5) * ppcs->picture_qp) {
                        lpd0_ctrls->pd0_level = pd0_lvl - 1;
                        continue;
                    }
                    // If the SB origin of one dimension is zero, then this SB is the first block in a row/column, so won't have neighbours
                    const uint16_t left_sb_index = sb_index - 1;
                    const uint16_t top_sb_index  = sb_index - (uint16_t)pic_width_in_sb;
                    if (md_ctx->sb_origin_x == 0 || md_ctx->sb_origin_y == 0) {
                        if (me_64x64_distortion > lpd0_ctrls->edge_dist_th[pd0_lvl]) {
                            lpd0_ctrls->pd0_level = pd0_lvl - 1;
                        }
                    } else {
                        if (lpd0_ctrls->neigh_me_dist_shift[pd0_lvl] != (uint16_t)~0 &&
                            me_64x64_distortion >
                                ((ppcs->me_64x64_distortion[left_sb_index] + ppcs->me_64x64_distortion[top_sb_index])
                                 << lpd0_ctrls->neigh_me_dist_shift[pd0_lvl])) {
                            lpd0_ctrls->pd0_level = pd0_lvl - 1;
                        } else if (lpd0_ctrls->neigh_me_dist_shift[pd0_lvl] != (uint16_t)~0 &&
                                   me_8x8_cost_variance > ((ppcs->me_8x8_cost_variance[left_sb_index] +
                                                            ppcs->me_8x8_cost_variance[top_sb_index])
                                                           << lpd0_ctrls->neigh_me_dist_shift[pd0_lvl])) {
                            lpd0_ctrls->pd0_level = pd0_lvl - 1;
                        } else if (lpd0_ctrls->use_ref_info[pd0_lvl]) {
                            // Use info from neighbouring SBs
                            if (pcs->sb_intra[left_sb_index] && pcs->sb_intra[top_sb_index]) {
                                lpd0_ctrls->pd0_level = pd0_lvl - 1;
                            } else if (!pcs->sb_skip[left_sb_index] && !pcs->sb_skip[top_sb_index] &&
                                       (pcs->sb_intra[left_sb_index] || pcs->sb_intra[top_sb_index])) {
                                lpd0_ctrls->pd0_level = pd0_lvl - 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert(IMPLIES(pcs->slice_type == I_SLICE, lpd0_ctrls->pd0_level != VERY_LIGHT_PD0));
}

static EbErrorType rtime_alloc_palette_search_buffers(ModeDecisionContext* ctx) {
    if (!ctx->palette_buffer) {
        EB_MALLOC_OBJECT(ctx->palette_buffer);
    }

    if (!ctx->palette_cand_array) {
        EB_MALLOC_ARRAY(ctx->palette_cand_array, MAX_PAL_CAND);
        for (int cd = 0; cd < MAX_PAL_CAND; cd++) {
            EB_MALLOC_ARRAY(ctx->palette_cand_array[cd].color_idx_map, MAX_PALETTE_SQUARE);
        }
    }

    if (!ctx->palette_size_array_0) {
        EB_MALLOC_ARRAY(ctx->palette_size_array_0, MAX_PAL_CAND);
    }

    return EB_ErrorNone;
}

#define AVG_CDF_WEIGHT_LEFT 3
#define AVG_CDF_WEIGHT_TOP 1

static NOINLINE void avg_cdf_symbol(AomCdfProb* cdf_ptr_left, AomCdfProb* cdf_ptr_tr, int num_cdfs, int cdf_stride,
                                    int nsymbs, int wt_left, int wt_tr) {
    for (int i = 0; i < num_cdfs; i++) {
        for (int j = 0; j <= nsymbs; j++) {
            cdf_ptr_left[i * cdf_stride + j] = (AomCdfProb)(((int)cdf_ptr_left[i * cdf_stride + j] * wt_left +
                                                             (int)cdf_ptr_tr[i * cdf_stride + j] * wt_tr +
                                                             ((wt_left + wt_tr) / 2)) /
                                                            (wt_left + wt_tr));
            assert(cdf_ptr_left[i * cdf_stride + j] < CDF_PROB_TOP);
        }
    }
}

#define AVERAGE_CDF(cname_left, cname_tr, nsymbs) AVG_CDF_STRIDE(cname_left, cname_tr, nsymbs, CDF_SIZE(nsymbs))

#define AVG_CDF_STRIDE(cname_left, cname_tr, nsymbs, cdf_stride)                                \
    do {                                                                                        \
        AomCdfProb* cdf_ptr_left = (AomCdfProb*)cname_left;                                     \
        AomCdfProb* cdf_ptr_tr   = (AomCdfProb*)cname_tr;                                       \
        int         array_size   = (int)sizeof(cname_left) / sizeof(AomCdfProb);                \
        int         num_cdfs     = array_size / cdf_stride;                                     \
        avg_cdf_symbol(cdf_ptr_left, cdf_ptr_tr, num_cdfs, cdf_stride, nsymbs, wt_left, wt_tr); \
    } while (0)

static NOINLINE void avg_nmv(NmvContext* nmv_left, NmvContext* nmv_tr, int wt_left, int wt_tr) {
    AVERAGE_CDF(nmv_left->joints_cdf, nmv_tr->joints_cdf, 4);
    for (int i = 0; i < 2; i++) {
        AVERAGE_CDF(nmv_left->comps[i].classes_cdf, nmv_tr->comps[i].classes_cdf, MV_CLASSES);
        AVERAGE_CDF(nmv_left->comps[i].class0_fp_cdf, nmv_tr->comps[i].class0_fp_cdf, MV_FP_SIZE);
        AVERAGE_CDF(nmv_left->comps[i].fp_cdf, nmv_tr->comps[i].fp_cdf, MV_FP_SIZE);
        AVERAGE_CDF(nmv_left->comps[i].sign_cdf, nmv_tr->comps[i].sign_cdf, 2);
        AVERAGE_CDF(nmv_left->comps[i].class0_hp_cdf, nmv_tr->comps[i].class0_hp_cdf, 2);
        AVERAGE_CDF(nmv_left->comps[i].hp_cdf, nmv_tr->comps[i].hp_cdf, 2);
        AVERAGE_CDF(nmv_left->comps[i].class0_cdf, nmv_tr->comps[i].class0_cdf, CLASS0_SIZE);
        AVERAGE_CDF(nmv_left->comps[i].bits_cdf, nmv_tr->comps[i].bits_cdf, 2);
    }
}

// Since the top and left SBs are completed, we can average the top SB's CDFs and
// the left SB's CDFs and use it for current SB's encoding to
// improve the performance. This function facilitates the averaging
// of CDF.
static NOINLINE void avg_cdf_symbols(FRAME_CONTEXT* ctx_left, FRAME_CONTEXT* ctx_tr, int wt_left, int wt_tr) {
    AVERAGE_CDF(ctx_left->txb_skip_cdf, ctx_tr->txb_skip_cdf, 2);
    AVERAGE_CDF(ctx_left->eob_extra_cdf, ctx_tr->eob_extra_cdf, 2);
    AVERAGE_CDF(ctx_left->dc_sign_cdf, ctx_tr->dc_sign_cdf, 2);
    AVERAGE_CDF(ctx_left->eob_flag_cdf16, ctx_tr->eob_flag_cdf16, 5);
    AVERAGE_CDF(ctx_left->eob_flag_cdf32, ctx_tr->eob_flag_cdf32, 6);
    AVERAGE_CDF(ctx_left->eob_flag_cdf64, ctx_tr->eob_flag_cdf64, 7);
    AVERAGE_CDF(ctx_left->eob_flag_cdf128, ctx_tr->eob_flag_cdf128, 8);
    AVERAGE_CDF(ctx_left->eob_flag_cdf256, ctx_tr->eob_flag_cdf256, 9);
    AVERAGE_CDF(ctx_left->eob_flag_cdf512, ctx_tr->eob_flag_cdf512, 10);
    AVERAGE_CDF(ctx_left->eob_flag_cdf1024, ctx_tr->eob_flag_cdf1024, 11);
    AVERAGE_CDF(ctx_left->coeff_base_eob_cdf, ctx_tr->coeff_base_eob_cdf, 3);
    AVERAGE_CDF(ctx_left->coeff_base_cdf, ctx_tr->coeff_base_cdf, 4);
    AVERAGE_CDF(ctx_left->coeff_br_cdf, ctx_tr->coeff_br_cdf, BR_CDF_SIZE);
    AVERAGE_CDF(ctx_left->newmv_cdf, ctx_tr->newmv_cdf, 2);
    AVERAGE_CDF(ctx_left->zeromv_cdf, ctx_tr->zeromv_cdf, 2);
    AVERAGE_CDF(ctx_left->refmv_cdf, ctx_tr->refmv_cdf, 2);
    AVERAGE_CDF(ctx_left->drl_cdf, ctx_tr->drl_cdf, 2);
    AVERAGE_CDF(ctx_left->inter_compound_mode_cdf, ctx_tr->inter_compound_mode_cdf, INTER_COMPOUND_MODES);
    AVERAGE_CDF(ctx_left->compound_type_cdf, ctx_tr->compound_type_cdf, MASKED_COMPOUND_TYPES);
    AVERAGE_CDF(ctx_left->wedge_idx_cdf, ctx_tr->wedge_idx_cdf, 16);
    AVERAGE_CDF(ctx_left->interintra_cdf, ctx_tr->interintra_cdf, 2);
    AVERAGE_CDF(ctx_left->wedge_interintra_cdf, ctx_tr->wedge_interintra_cdf, 2);
    AVERAGE_CDF(ctx_left->interintra_mode_cdf, ctx_tr->interintra_mode_cdf, INTERINTRA_MODES);
    AVERAGE_CDF(ctx_left->motion_mode_cdf, ctx_tr->motion_mode_cdf, MOTION_MODES);
    AVERAGE_CDF(ctx_left->obmc_cdf, ctx_tr->obmc_cdf, 2);
    AVERAGE_CDF(ctx_left->palette_y_size_cdf, ctx_tr->palette_y_size_cdf, PALETTE_SIZES);
    AVERAGE_CDF(ctx_left->palette_uv_size_cdf, ctx_tr->palette_uv_size_cdf, PALETTE_SIZES);
    for (int j = 0; j < PALETTE_SIZES; j++) {
        int nsymbs = j + PALETTE_MIN_SIZE;
        AVG_CDF_STRIDE(ctx_left->palette_y_color_index_cdf[j],
                       ctx_tr->palette_y_color_index_cdf[j],
                       nsymbs,
                       CDF_SIZE(PALETTE_COLORS));
        AVG_CDF_STRIDE(ctx_left->palette_uv_color_index_cdf[j],
                       ctx_tr->palette_uv_color_index_cdf[j],
                       nsymbs,
                       CDF_SIZE(PALETTE_COLORS));
    }
    AVERAGE_CDF(ctx_left->palette_y_mode_cdf, ctx_tr->palette_y_mode_cdf, 2);
    AVERAGE_CDF(ctx_left->palette_uv_mode_cdf, ctx_tr->palette_uv_mode_cdf, 2);
    AVERAGE_CDF(ctx_left->comp_inter_cdf, ctx_tr->comp_inter_cdf, 2);
    AVERAGE_CDF(ctx_left->single_ref_cdf, ctx_tr->single_ref_cdf, 2);
    AVERAGE_CDF(ctx_left->comp_ref_type_cdf, ctx_tr->comp_ref_type_cdf, 2);
    AVERAGE_CDF(ctx_left->uni_comp_ref_cdf, ctx_tr->uni_comp_ref_cdf, 2);
    AVERAGE_CDF(ctx_left->comp_ref_cdf, ctx_tr->comp_ref_cdf, 2);
    AVERAGE_CDF(ctx_left->comp_bwdref_cdf, ctx_tr->comp_bwdref_cdf, 2);
    AVERAGE_CDF(ctx_left->txfm_partition_cdf, ctx_tr->txfm_partition_cdf, 2);
    AVERAGE_CDF(ctx_left->compound_index_cdf, ctx_tr->compound_index_cdf, 2);
    AVERAGE_CDF(ctx_left->comp_group_idx_cdf, ctx_tr->comp_group_idx_cdf, 2);
    AVERAGE_CDF(ctx_left->skip_mode_cdfs, ctx_tr->skip_mode_cdfs, 2);
    AVERAGE_CDF(ctx_left->skip_cdfs, ctx_tr->skip_cdfs, 2);
    AVERAGE_CDF(ctx_left->intra_inter_cdf, ctx_tr->intra_inter_cdf, 2);
    avg_nmv(&ctx_left->nmvc, &ctx_tr->nmvc, wt_left, wt_tr);
    avg_nmv(&ctx_left->ndvc, &ctx_tr->ndvc, wt_left, wt_tr);
    AVERAGE_CDF(ctx_left->intrabc_cdf, ctx_tr->intrabc_cdf, 2);
    AVERAGE_CDF(ctx_left->seg.tree_cdf, ctx_tr->seg.tree_cdf, MAX_SEGMENTS);
    AVERAGE_CDF(ctx_left->seg.pred_cdf, ctx_tr->seg.pred_cdf, 2);
    AVERAGE_CDF(ctx_left->seg.spatial_pred_seg_cdf, ctx_tr->seg.spatial_pred_seg_cdf, MAX_SEGMENTS);
    AVERAGE_CDF(ctx_left->filter_intra_cdfs, ctx_tr->filter_intra_cdfs, 2);
    AVERAGE_CDF(ctx_left->filter_intra_mode_cdf, ctx_tr->filter_intra_mode_cdf, FILTER_INTRA_MODES);
    AVERAGE_CDF(ctx_left->switchable_restore_cdf, ctx_tr->switchable_restore_cdf, RESTORE_SWITCHABLE_TYPES);
    AVERAGE_CDF(ctx_left->wiener_restore_cdf, ctx_tr->wiener_restore_cdf, 2);
    AVERAGE_CDF(ctx_left->sgrproj_restore_cdf, ctx_tr->sgrproj_restore_cdf, 2);
    AVERAGE_CDF(ctx_left->y_mode_cdf, ctx_tr->y_mode_cdf, INTRA_MODES);
    AVG_CDF_STRIDE(ctx_left->uv_mode_cdf[0], ctx_tr->uv_mode_cdf[0], UV_INTRA_MODES - 1, CDF_SIZE(UV_INTRA_MODES));
    AVERAGE_CDF(ctx_left->uv_mode_cdf[1], ctx_tr->uv_mode_cdf[1], UV_INTRA_MODES);
    for (int i = 0; i < PARTITION_CONTEXTS; i++) {
        if (i < 4) {
            AVG_CDF_STRIDE(ctx_left->partition_cdf[i], ctx_tr->partition_cdf[i], 4, CDF_SIZE(10));
        } else if (i < 16) {
            AVERAGE_CDF(ctx_left->partition_cdf[i], ctx_tr->partition_cdf[i], 10);
        } else {
            AVG_CDF_STRIDE(ctx_left->partition_cdf[i], ctx_tr->partition_cdf[i], 8, CDF_SIZE(10));
        }
    }
    AVERAGE_CDF(ctx_left->switchable_interp_cdf, ctx_tr->switchable_interp_cdf, SWITCHABLE_FILTERS);
    AVERAGE_CDF(ctx_left->kf_y_cdf, ctx_tr->kf_y_cdf, INTRA_MODES);
    AVERAGE_CDF(ctx_left->angle_delta_cdf, ctx_tr->angle_delta_cdf, 2 * MAX_ANGLE_DELTA + 1);
    AVG_CDF_STRIDE(ctx_left->tx_size_cdf[0], ctx_tr->tx_size_cdf[0], MAX_TX_DEPTH, CDF_SIZE(MAX_TX_DEPTH + 1));
    AVERAGE_CDF(ctx_left->tx_size_cdf[1], ctx_tr->tx_size_cdf[1], MAX_TX_DEPTH + 1);
    AVERAGE_CDF(ctx_left->tx_size_cdf[2], ctx_tr->tx_size_cdf[2], MAX_TX_DEPTH + 1);
    AVERAGE_CDF(ctx_left->tx_size_cdf[3], ctx_tr->tx_size_cdf[3], MAX_TX_DEPTH + 1);
    AVERAGE_CDF(ctx_left->delta_q_cdf, ctx_tr->delta_q_cdf, DELTA_Q_PROBS + 1);
    AVERAGE_CDF(ctx_left->delta_lf_cdf, ctx_tr->delta_lf_cdf, DELTA_LF_PROBS + 1);
    for (int i = 0; i < FRAME_LF_COUNT; i++) {
        AVERAGE_CDF(ctx_left->delta_lf_multi_cdf[i], ctx_tr->delta_lf_multi_cdf[i], DELTA_LF_PROBS + 1);
    }
    AVG_CDF_STRIDE(ctx_left->intra_ext_tx_cdf[1], ctx_tr->intra_ext_tx_cdf[1], 7, CDF_SIZE(TX_TYPES));
    AVG_CDF_STRIDE(ctx_left->intra_ext_tx_cdf[2], ctx_tr->intra_ext_tx_cdf[2], 5, CDF_SIZE(TX_TYPES));
    AVG_CDF_STRIDE(ctx_left->inter_ext_tx_cdf[1], ctx_tr->inter_ext_tx_cdf[1], 16, CDF_SIZE(TX_TYPES));
    AVG_CDF_STRIDE(ctx_left->inter_ext_tx_cdf[2], ctx_tr->inter_ext_tx_cdf[2], 12, CDF_SIZE(TX_TYPES));
    AVG_CDF_STRIDE(ctx_left->inter_ext_tx_cdf[3], ctx_tr->inter_ext_tx_cdf[3], 2, CDF_SIZE(TX_TYPES));
    AVERAGE_CDF(ctx_left->cfl_sign_cdf, ctx_tr->cfl_sign_cdf, CFL_JOINT_SIGNS);
    AVERAGE_CDF(ctx_left->cfl_alpha_cdf, ctx_tr->cfl_alpha_cdf, CFL_ALPHABET_SIZE);
}

/* EncDec (Encode Decode) Kernel */
/*********************************************************************************
 *
 * @brief
 *  The EncDec process contains both the mode decision and the encode pass engines
 *  of the encoder. The mode decision encapsulates multiple partitioning decision (PD) stages
 *  and multiple mode decision (MD) stages. At the end of the last mode decision stage,
 *  the winning partition and modes combinations per block get reconstructed in the encode pass
 *  operation which is part of the common section between the encoder and the decoder
 *  Common encoder and decoder tasks such as Intra Prediction, Motion Compensated Prediction,
 *  Transform, Quantization are performed in this process.
 *
 * @par Description:
 *  The EncDec process operates on an SB basis.
 *  The EncDec process takes as input the Motion Vector XY pairs candidates
 *  and corresponding distortion estimates from the Motion Estimation process,
 *  and the picture-level QP from the Rate Control process. All inputs are passed
 *  through the picture structures: PictureControlSet and SequenceControlSet.
 *  local structures of type EncDecContext and ModeDecisionContext contain all parameters
 *  and results corresponding to the SuperBlock being processed.
 *  each of the context structures is local to on thread and thus there's no risk of
 *  affecting (changing) other SBs data in the process.
 *
 * @param[in] Vector
 *  Motion Vector XY pairs from Motion Estimation process
 *
 * @param[in] Distortion Estimates
 *  Distortion estimates from Motion Estimation process
 *
 * @param[in] Picture QP
 *  Picture Quantization Parameter from Rate Control process
 *
 * @param[out] Blocks
 *  The encode pass takes the selected partitioning and coding modes as input from mode decision for
 *each superblock and produces quantized transfrom coefficients for the residuals and the
 *appropriate syntax elements to be sent to the entropy coding engine
 *
 ********************************************************************************/
void* svt_aom_mode_decision_kernel(void* input_ptr) {
    // Context & SCS & PCS
    EbThreadContext* thread_ctx = (EbThreadContext*)input_ptr;
    EncDecContext*   ed_ctx     = (EncDecContext*)thread_ctx->priv;

    // Input
    EbObjectWrapper* enc_dec_tasks_wrapper;

    // Output
    EbObjectWrapper* enc_dec_results_wrapper;
    EncDecResults*   enc_dec_results;
    // SB Loop variables
    SuperBlock* sb_ptr;
    uint16_t    sb_index;
    uint32_t    x_sb_index;
    uint32_t    y_sb_index;
    uint32_t    sb_origin_x;
    uint32_t    sb_origin_y;

    // Segments
    uint16_t        segment_index;
    uint32_t        x_sb_start_index;
    uint32_t        y_sb_start_index;
    uint32_t        sb_start_index;
    uint32_t        sb_segment_count;
    uint32_t        sb_segment_index;
    uint32_t        segment_row_index;
    uint32_t        segment_band_index;
    uint32_t        segment_band_size;
    EncDecSegments* segments_ptr;

    segment_index = 0;

    for (;;) {
        // Get Mode Decision Results
        EB_GET_FULL_OBJECT(ed_ctx->mode_decision_input_fifo_ptr, &enc_dec_tasks_wrapper);

        EncDecTasks*             enc_dec_tasks = (EncDecTasks*)enc_dec_tasks_wrapper->object_ptr;
        PictureControlSet*       pcs           = (PictureControlSet*)enc_dec_tasks->pcs_wrapper->object_ptr;
        SequenceControlSet*      scs           = pcs->scs;
        ModeDecisionContext*     md_ctx        = ed_ctx->md_ctx;
        PictureParentControlSet* ppcs          = pcs->ppcs;
        md_ctx->encoder_bit_depth              = (uint8_t)scs->static_config.encoder_bit_depth;
        md_ctx->corrupted_mv_check             = (pcs->ppcs->aligned_width >= (1 << (MV_IN_USE_BITS - 3))) ||
            (pcs->ppcs->aligned_height >= (1 << (MV_IN_USE_BITS - 3)));
        ed_ctx->tile_group_index = enc_dec_tasks->tile_group_index;
        ed_ctx->coded_sb_count   = 0;
        segments_ptr             = pcs->enc_dec_segment_ctrl[ed_ctx->tile_group_index];
        // SB Constants
        uint8_t  sb_size                = (uint8_t)scs->sb_size;
        uint8_t  sb_size_log2           = (uint8_t)svt_log2f(sb_size);
        uint32_t pic_width_in_sb        = (pcs->ppcs->aligned_width + sb_size - 1) >> sb_size_log2;
        uint16_t tile_group_width_in_sb = pcs->ppcs->tile_group_info[ed_ctx->tile_group_index].tile_group_width_in_sb;
        ed_ctx->tot_intra_coded_area    = 0;
        ed_ctx->tot_skip_coded_area     = 0;
        ed_ctx->tot_hp_coded_area       = 0;
        ed_ctx->tot_cnt_zero_mv         = 0;
        // Bypass encdec for the first pass
        if (svt_aom_is_pic_skipped(pcs->ppcs)) {
            svt_release_object(pcs->ppcs->me_data_wrapper);
            pcs->ppcs->me_data_wrapper = (EbObjectWrapper*)NULL;
            pcs->ppcs->pa_me_data      = NULL;
            // Get Empty EncDec Results
            svt_get_empty_object(ed_ctx->enc_dec_output_fifo_ptr, &enc_dec_results_wrapper);
            enc_dec_results              = (EncDecResults*)enc_dec_results_wrapper->object_ptr;
            enc_dec_results->pcs_wrapper = enc_dec_tasks->pcs_wrapper;

            // Post EncDec Results
            svt_post_full_object(enc_dec_results_wrapper);
        } else {
            if (enc_dec_tasks->input_type == ENCDEC_TASKS_SUPERRES_INPUT) {
                // do as dorecode do
                pcs->enc_dec_coded_sb_count = 0;
                // re-init mode decision configuration for qp update for re-encode frame
                mdc_init_qp_update(pcs);
                // init segment for re-encode frame
                svt_aom_init_enc_dec_segement(pcs->ppcs);

                // post tile based encdec task
                EbObjectWrapper* enc_dec_re_encode_tasks_wrapper;
                uint16_t         tg_count = pcs->ppcs->tile_group_cols * pcs->ppcs->tile_group_rows;
                for (uint16_t tile_group_idx = 0; tile_group_idx < tg_count; tile_group_idx++) {
                    svt_get_empty_object(ed_ctx->enc_dec_feedback_fifo_ptr, &enc_dec_re_encode_tasks_wrapper);

                    EncDecTasks* enc_dec_re_encode_tasks_ptr = (EncDecTasks*)
                                                                   enc_dec_re_encode_tasks_wrapper->object_ptr;
                    enc_dec_re_encode_tasks_ptr->pcs_wrapper      = enc_dec_tasks->pcs_wrapper;
                    enc_dec_re_encode_tasks_ptr->input_type       = ENCDEC_TASKS_MDC_INPUT;
                    enc_dec_re_encode_tasks_ptr->tile_group_index = tile_group_idx;

                    // Post the Full Results Object
                    svt_post_full_object(enc_dec_re_encode_tasks_wrapper);
                }

                svt_release_object(enc_dec_tasks_wrapper);
                continue;
            }

            if (pcs->cdf_ctrl.enabled) {
                if (!pcs->cdf_ctrl.update_mv) {
                    copy_mv_rate(pcs, ed_ctx->md_ctx->rate_est_table);
                }
                if (!pcs->cdf_ctrl.update_se) {
                    svt_aom_estimate_syntax_rate(ed_ctx->md_ctx->rate_est_table,
                                                 pcs->slice_type == I_SLICE ? true : false,
                                                 scs->seq_header.filter_intra_level,
                                                 pcs->ppcs->frm_hdr.allow_screen_content_tools,
                                                 pcs->ppcs->enable_restoration,
                                                 pcs->ppcs->frm_hdr.allow_intrabc,
                                                 &pcs->md_frame_context);
                }
                if (!pcs->cdf_ctrl.update_coef) {
                    svt_aom_estimate_coefficients_rate(ed_ctx->md_ctx->rate_est_table, &pcs->md_frame_context);
                }
            }
            // Segment-loop
            while (assign_enc_dec_segments(
                       segments_ptr, &segment_index, enc_dec_tasks, ed_ctx->enc_dec_feedback_fifo_ptr) == true) {
                x_sb_start_index = segments_ptr->x_start_array[segment_index];
                y_sb_start_index = segments_ptr->y_start_array[segment_index];
                sb_start_index   = y_sb_start_index * tile_group_width_in_sb + x_sb_start_index;
                sb_segment_count = segments_ptr->valid_sb_count_array[segment_index];

                segment_row_index  = segment_index / segments_ptr->segment_band_count;
                segment_band_index = segment_index - segment_row_index * segments_ptr->segment_band_count;
                segment_band_size  = (segments_ptr->sb_band_count * (segment_band_index + 1) +
                                     segments_ptr->segment_band_count - 1) /
                    segments_ptr->segment_band_count;

                // Reset Coding Loop State
                svt_aom_reset_mode_decision(scs, ed_ctx->md_ctx, pcs, ed_ctx->tile_group_index, segment_index);

                // Reset EncDec Coding State
                reset_enc_dec( // HT done
                    ed_ctx,
                    pcs,
                    scs,
                    segment_index);

                for (y_sb_index = y_sb_start_index, sb_segment_index = sb_start_index;
                     sb_segment_index < sb_start_index + sb_segment_count;
                     ++y_sb_index) {
                    for (x_sb_index = x_sb_start_index;
                         x_sb_index < tile_group_width_in_sb && (x_sb_index + y_sb_index < segment_band_size) &&
                         sb_segment_index < sb_start_index + sb_segment_count;
                         ++x_sb_index, ++sb_segment_index) {
                        uint16_t tile_group_y_sb_start =
                            pcs->ppcs->tile_group_info[ed_ctx->tile_group_index].tile_group_sb_start_y;
                        uint16_t tile_group_x_sb_start =
                            pcs->ppcs->tile_group_info[ed_ctx->tile_group_index].tile_group_sb_start_x;
                        sb_index = ed_ctx->md_ctx->sb_index = (uint16_t)((y_sb_index + tile_group_y_sb_start) *
                                                                             pic_width_in_sb +
                                                                         x_sb_index + tile_group_x_sb_start);
                        sb_ptr = ed_ctx->md_ctx->sb_ptr = pcs->sb_ptr_array[sb_index];
                        sb_origin_x                     = (x_sb_index + tile_group_x_sb_start) << sb_size_log2;
                        sb_origin_y                     = (y_sb_index + tile_group_y_sb_start) << sb_size_log2;
                        ed_ctx->tile_index              = sb_ptr->tile_info.tile_rs_index;
                        ed_ctx->md_ctx->tile_index      = sb_ptr->tile_info.tile_rs_index;
                        ed_ctx->md_ctx->sb_origin_x     = sb_origin_x;
                        ed_ctx->md_ctx->sb_origin_y     = sb_origin_y;
                        ed_ctx->sb_index                = sb_index;
                        if (pcs->cdf_ctrl.enabled) {
                            if (scs->pic_based_rate_est && scs->enc_dec_segment_row_count_array == 1 &&
                                scs->enc_dec_segment_col_count_array == 1) {
                                if (sb_index == 0) {
                                    pcs->ec_ctx_array[sb_index] = pcs->md_frame_context;
                                } else {
                                    pcs->ec_ctx_array[sb_index] = pcs->ec_ctx_array[sb_index - 1];
                                }
                            } else {
                                // Use the latest available CDF for the current SB
                                // Use the weighted average of left (3x) and top right (1x) if available.
                                int8_t top_right_available = ((int32_t)(sb_origin_y >> MI_SIZE_LOG2) >
                                                              sb_ptr->tile_info.mi_row_start) &&
                                    ((int32_t)((sb_origin_x + (1 << sb_size_log2)) >> MI_SIZE_LOG2) <
                                     sb_ptr->tile_info.mi_col_end);

                                int8_t left_available = ((int32_t)(sb_origin_x >> MI_SIZE_LOG2) >
                                                         sb_ptr->tile_info.mi_col_start);

                                if (!left_available && !top_right_available) {
                                    pcs->ec_ctx_array[sb_index] = pcs->md_frame_context;
                                } else if (!left_available) {
                                    pcs->ec_ctx_array[sb_index] = pcs->ec_ctx_array[sb_index - pic_width_in_sb + 1];
                                } else if (!top_right_available) {
                                    pcs->ec_ctx_array[sb_index] = pcs->ec_ctx_array[sb_index - 1];
                                } else {
                                    pcs->ec_ctx_array[sb_index] = pcs->ec_ctx_array[sb_index - 1];
                                    avg_cdf_symbols(&pcs->ec_ctx_array[sb_index],
                                                    &pcs->ec_ctx_array[sb_index - pic_width_in_sb + 1],
                                                    AVG_CDF_WEIGHT_LEFT,
                                                    AVG_CDF_WEIGHT_TOP);
                                }
                            }
                            // Initial Rate Estimation of the syntax elements
                            if (pcs->cdf_ctrl.update_se) {
                                svt_aom_estimate_syntax_rate(ed_ctx->md_ctx->rate_est_table,
                                                             pcs->slice_type == I_SLICE,
                                                             scs->seq_header.filter_intra_level,
                                                             pcs->ppcs->frm_hdr.allow_screen_content_tools,
                                                             pcs->ppcs->enable_restoration,
                                                             pcs->ppcs->frm_hdr.allow_intrabc,
                                                             &pcs->ec_ctx_array[sb_index]);
                            }
                            // Initial Rate Estimation of the Motion vectors
                            if (pcs->cdf_ctrl.update_mv) {
                                svt_aom_estimate_mv_rate(
                                    pcs, ed_ctx->md_ctx->rate_est_table, &pcs->ec_ctx_array[sb_index]);
                            }

                            if (pcs->cdf_ctrl.update_coef) {
                                svt_aom_estimate_coefficients_rate(ed_ctx->md_ctx->rate_est_table,
                                                                   &pcs->ec_ctx_array[sb_index]);
                            }
                            ed_ctx->md_ctx->md_rate_est_ctx = ed_ctx->md_ctx->rate_est_table;
                        }

                        // Configure the SB
                        svt_aom_mode_decision_configure_sb(
                            ed_ctx->md_ctx,
                            pcs,
                            sb_ptr->qindex,
                            svt_aom_get_me_qindex(pcs, sb_ptr, scs->seq_header.sb_size == BLOCK_128X128));
                        // signals set once per SB (i.e. not per PD)
                        svt_aom_sig_deriv_enc_dec_common(scs, pcs, ed_ctx->md_ctx);

                        if (pcs->ppcs->palette_level) {
                            rtime_alloc_palette_search_buffers(md_ctx);
                            // Status of palette info alloc
                            for (int i = 0; i < scs->max_block_cnt; ++i) {
                                ed_ctx->md_ctx->md_blk_arr_nsq[i].palette_mem = 0;
                            }
                        }

                        // Initialize is_subres_safe
                        ed_ctx->md_ctx->is_subres_safe = (uint8_t)~0;
                        // Signal initialized here; if needed, will be set in md_encode_block before MDS3
                        md_ctx->need_hbd_comp_mds3 = 0;
                        bool skip_pd_pass_0        = (ed_ctx->md_ctx->depth_removal_ctrls.disallow_below_64x64 &&
                                               (scs->super_block_size == 64 || ed_ctx->md_ctx->max_block_size == 64)) ||
                            (ed_ctx->md_ctx->depth_removal_ctrls.disallow_below_32x32 &&
                             ed_ctx->md_ctx->max_block_size == 32);
#if FTR_VLPD0 // classifier
                        if (scs->allintra) {
                            lpd0_detector_allintra(pcs, md_ctx);
                        } else {
#endif

                            // If LPD0 is used, a more conservative level can be set for complex SBs
                            const bool use_lpd0_classifier = !scs->static_config.rtc || pcs->ppcs->sc_class1 ||
                                pcs->enc_mode <= ENC_M9;
                            if (use_lpd0_classifier && md_ctx->lpd0_ctrls.pd0_level > REGULAR_PD0) {
                                lpd0_detector(pcs, md_ctx, pic_width_in_sb);
                            }
#if FTR_VLPD0
                        }
#endif
                        // PD0 is only skipped if there is a single depth to test
                        if (skip_pd_pass_0) {
                            md_ctx->pred_depth_only = 1;
                        }

                        // Multi-Pass PD
                        if (!skip_pd_pass_0 && pcs->ppcs->multi_pass_pd_level == MULTI_PASS_PD_ON) {
                            // [PD_PASS_0]
                            // Input : mdc_blk_ptr built @ mdc process (up to 4421)
                            // Output: md_blk_arr_nsq reduced set of block(s)
                            ed_ctx->md_ctx->pd_pass = PD_PASS_0;
                            // PD0 doesn't have a fixed partition structure, as the main purpose of PD0
                            // is to determine a prediction for the final prediction structure
                            md_ctx->fixed_partition = false;
                            // skip_intra much be true for non-I_SLICE pictures to use light_pd0 path
                            if (md_ctx->lpd0_ctrls.pd0_level > REGULAR_PD0) {
                                // [PD_PASS_0] Signal(s) derivation
                                svt_aom_sig_deriv_enc_dec_light_pd0(scs, pcs, ed_ctx->md_ctx);
                                // Save a clean copy of the neighbor arrays
                                if (!ed_ctx->md_ctx->skip_intra) {
                                    copy_neighbour_arrays_light_pd0(pcs,
                                                                    ed_ctx->md_ctx,
                                                                    MD_NEIGHBOR_ARRAY_INDEX,
                                                                    MULTI_STAGE_PD_NEIGHBOR_ARRAY_INDEX,
                                                                    sb_origin_x,
                                                                    sb_origin_y);
                                }

                                set_blocks_to_be_tested(scs, pcs, md_ctx, md_ctx->mds, 0);
                                svt_aom_init_sb_data(scs, pcs, md_ctx);
                                svt_aom_pick_partition_lpd0(scs,
                                                            pcs,
                                                            ed_ctx->md_ctx,
                                                            md_ctx->mds,
                                                            md_ctx->pc_tree,
                                                            md_ctx->sb_origin_y >> 2,
                                                            md_ctx->sb_origin_x >> 2);
                                // Re-build mdc_blk_ptr for the 2nd PD Pass [PD_PASS_1]
                                // Reset neighbor information to current SB @ position (0,0)
                                if (!ed_ctx->md_ctx->skip_intra) {
                                    copy_neighbour_arrays_light_pd0(pcs,
                                                                    ed_ctx->md_ctx,
                                                                    MULTI_STAGE_PD_NEIGHBOR_ARRAY_INDEX,
                                                                    MD_NEIGHBOR_ARRAY_INDEX,
                                                                    sb_origin_x,
                                                                    sb_origin_y);
                                }
                            } else {
                                // [PD_PASS_0] Signal(s) derivation
#if TUNE_STILL_IMAGE
                                if (scs->allintra) {
                                    svt_aom_sig_deriv_enc_dec_allintra(pcs, ed_ctx->md_ctx);
                                } else if (scs->static_config.rtc) {
                                    svt_aom_sig_deriv_enc_dec_rtc(pcs, ed_ctx->md_ctx);
                                } else {
                                    svt_aom_sig_deriv_enc_dec_default(pcs, ed_ctx->md_ctx);
                                }

#else
                                svt_aom_sig_deriv_enc_dec(scs, pcs, ed_ctx->md_ctx);
#endif

                                // Save a clean copy of the neighbor arrays
                                svt_aom_copy_neighbour_arrays(pcs,
                                                              ed_ctx->md_ctx,
                                                              MD_NEIGHBOR_ARRAY_INDEX,
                                                              MULTI_STAGE_PD_NEIGHBOR_ARRAY_INDEX,
                                                              scs->seq_header.sb_size,
                                                              sb_origin_y >> MI_SIZE_LOG2,
                                                              sb_origin_x >> MI_SIZE_LOG2);

                                set_blocks_to_be_tested(scs, pcs, md_ctx, md_ctx->mds, 0);
                                // PD0 MD Tool(s) : ME_MV(s) as INTER candidate(s), DC as INTRA candidate, luma only, Frequency domain SSE,
                                // no fast rate (no MVP table generation), MDS0 then MDS3, reduced NIC(s), 1 ref per list,..
                                svt_aom_init_sb_data(scs, pcs, md_ctx);
                                svt_aom_pick_partition(scs,
                                                       pcs,
                                                       ed_ctx->md_ctx,
                                                       md_ctx->mds,
                                                       md_ctx->pc_tree,
                                                       md_ctx->sb_origin_y >> 2,
                                                       md_ctx->sb_origin_x >> 2);
                                // Re-build mdc_blk_ptr for the 2nd PD Pass [PD_PASS_1]
                                // Reset neighbor information to current SB @ position (0,0)
                                svt_aom_copy_neighbour_arrays(pcs,
                                                              ed_ctx->md_ctx,
                                                              MULTI_STAGE_PD_NEIGHBOR_ARRAY_INDEX,
                                                              MD_NEIGHBOR_ARRAY_INDEX,
                                                              scs->seq_header.sb_size,
                                                              sb_origin_y >> MI_SIZE_LOG2,
                                                              sb_origin_x >> MI_SIZE_LOG2);
                            }
                            // This classifier is used for only pd0_level 0 and pd0_level 1
                            // where the cnt_nz_coeff is derived @ PD0
                            if (md_ctx->lpd0_ctrls.pd0_level < VERY_LIGHT_PD0) {
                                lpd1_detector_post_pd0(pcs, md_ctx, md_ctx->pc_tree);
                            }
                            // Force pred depth only for modes where that is not the default
                            if (md_ctx->lpd1_ctrls.pd1_level > REGULAR_PD1) {
                                ed_ctx->md_ctx->depth_refinement_ctrls.mode = PD0_DEPTH_PRED_PART_ONLY;
                                md_ctx->pred_depth_only                     = 1;
                            }
                            // Perform Pred_0 depth refinement - add depth(s) to be considered in the next stage(s)
                            perform_pred_depth_refinement(pcs,
                                                          ed_ctx->md_ctx,
                                                          md_ctx->pc_tree,
                                                          md_ctx->mds,
                                                          md_ctx->sb_origin_y >> 2,
                                                          md_ctx->sb_origin_x >> 2);
                        }
                        // [PD_PASS_1] Signal(s) derivation
                        ed_ctx->md_ctx->pd_pass = PD_PASS_1;
                        // This classifier is used for the case PD0 is bypassed and for pd0_level 2
                        // where the cnt_nz_coeff is not derived @ PD0
                        if (skip_pd_pass_0 || md_ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0) {
                            lpd1_detector_skip_pd0(pcs, md_ctx, pic_width_in_sb);
                        }

                        // Can only use light-PD1 under the following conditions
                        if (!(md_ctx->hbd_md == 0 && md_ctx->pred_depth_only && md_ctx->disallow_4x4 == true &&
                              scs->super_block_size == 64)) {
                            md_ctx->lpd1_ctrls.pd1_level = REGULAR_PD1;
                        }
                        exaustive_light_pd1_features(md_ctx, ppcs, md_ctx->lpd1_ctrls.pd1_level > REGULAR_PD1, 0);
                        if (md_ctx->lpd1_ctrls.pd1_level > REGULAR_PD1) {
                            svt_aom_sig_deriv_enc_dec_light_pd1(pcs, ed_ctx->md_ctx);
#if TUNE_STILL_IMAGE
                        } else if (scs->allintra) {
                            svt_aom_sig_deriv_enc_dec_allintra(pcs, ed_ctx->md_ctx);
                        } else if (scs->static_config.rtc) {
                            svt_aom_sig_deriv_enc_dec_rtc(pcs, ed_ctx->md_ctx);
                        } else {
                            svt_aom_sig_deriv_enc_dec_default(pcs, ed_ctx->md_ctx);
                        }
#else
                        } else {
                            svt_aom_sig_deriv_enc_dec(scs, pcs, ed_ctx->md_ctx);
                        }
#endif
                        // If there is only one depth and no NSQ search at PD1, then the partition structure
                        // is fixed.
                        md_ctx->fixed_partition = md_ctx->pred_depth_only && md_ctx->md_disallow_nsq_search;

                        set_blocks_to_be_tested(
                            scs,
                            pcs,
                            md_ctx,
                            md_ctx->mds,
                            !(skip_pd_pass_0 || pcs->ppcs->multi_pass_pd_level == MULTI_PASS_PD_OFF));
                        // [PD_PASS_1] Mode Decision - Obtain the final partitioning decision using more accurate info
                        // than previous stages.  Reduce the total number of partitions to 1.
                        // Input : mdc_blk_ptr built @ PD0 refinement
                        // Output: md_blk_arr_nsq reduced set of block(s)

                        // PD1 MD Tool(s): default MD Tool(s)
                        svt_aom_init_sb_data(scs, pcs, md_ctx);
                        if (md_ctx->lpd1_ctrls.pd1_level > REGULAR_PD1) {
                            svt_aom_pick_partition_lpd1(scs,
                                                        pcs,
                                                        ed_ctx->md_ctx,
                                                        md_ctx->mds,
                                                        md_ctx->pc_tree,
                                                        md_ctx->sb_origin_y >> 2,
                                                        md_ctx->sb_origin_x >> 2);
                        } else {
                            svt_aom_pick_partition(scs,
                                                   pcs,
                                                   ed_ctx->md_ctx,
                                                   md_ctx->mds,
                                                   md_ctx->pc_tree,
                                                   md_ctx->sb_origin_y >> 2,
                                                   md_ctx->sb_origin_x >> 2);
                        }
                        //  Encode Pass
                        if (!ed_ctx->md_ctx->bypass_encdec) {
                            ed_ctx->coded_area_sb    = 0;
                            ed_ctx->coded_area_sb_uv = 0;
                            ed_ctx->input_samples    = pcs->ppcs->enhanced_pic;
                            prepare_input_picture(scs, pcs, ed_ctx, pcs->ppcs->enhanced_pic, sb_origin_x, sb_origin_y);
                        }
                        if (sb_index == 0) {
                            pcs->ppcs->pcs_total_rate = 0;
                        }
                        ed_ctx->coded_area_sb_update    = 0;
                        ed_ctx->coded_area_sb_uv_update = 0;
                        if (!scs->allintra) {
                            pcs->sb_intra[sb_index]       = 0;
                            pcs->sb_skip[sb_index]        = 1;
                            pcs->sb_64x64_mvp[sb_index]   = 0;
                            pcs->sb_min_sq_size[sb_index] = 128;
                            pcs->sb_max_sq_size[sb_index] = 0;
                        }
                        sb_ptr->final_blk_cnt = 0;
                        svt_aom_encode_sb(scs,
                                          pcs,
                                          ed_ctx,
                                          sb_ptr,
                                          md_ctx->pc_tree,
                                          sb_ptr->ptree,
                                          md_ctx->sb_origin_y >> 2,
                                          md_ctx->sb_origin_x >> 2);
                        // free MD palette info buffer
                        if (pcs->ppcs->palette_level) {
                            const uint16_t max_block_cnt = scs->max_block_cnt;
                            uint32_t       blk_index     = 0;
                            while (blk_index < max_block_cnt) {
                                if (md_ctx->md_blk_arr_nsq[blk_index].palette_mem) {
                                    EB_FREE_ARRAY(md_ctx->md_blk_arr_nsq[blk_index].palette_info->color_idx_map);
                                    EB_FREE_ARRAY(md_ctx->md_blk_arr_nsq[blk_index].palette_info);
                                    md_ctx->md_blk_arr_nsq[blk_index].palette_mem = 0;
                                }
                                blk_index++;
                            }
                        }

                        // When DLF filters are derived without a frame-level search, we can apply the filters here
                        // to take advantage of the MD multi-threading.
                        // TODO: Add segments to DLF so this can be moved to that process (where is belongs) without
                        // losing the multi-threaded performance.
                        const uint16_t tg_count   = pcs->ppcs->tile_group_cols * pcs->ppcs->tile_group_rows;
                        const bool     enable_dlf = pcs->ppcs->dlf_ctrls.enabled && pcs->ppcs->dlf_ctrls.sb_based_dlf;
                        if (enable_dlf && tg_count == 1) {
                            //Generate the loop filter parameters
                            if (sb_index == 0) {
                                svt_av1_loop_filter_init(pcs);
                                svt_av1_pick_filter_level(
                                    (EbPictureBufferDesc*)pcs->ppcs->enhanced_pic, pcs, LPF_PICK_FROM_Q);
                                svt_av1_loop_filter_frame_init(&pcs->ppcs->frm_hdr, &pcs->ppcs->lf_info, 0, 3);
                            }

                            // Apply the loop filter
                            //Jing: Don't work for tile_parallel since the SB of bottom tile comes early than the bottom SB of top tile

#if OPT_DLF
#if OPT_Q_CDEF
                            if ((pcs->ppcs->cdef_search_ctrls.enabled &&
                                 !pcs->ppcs->cdef_search_ctrls.use_qp_strength &&
                                 !pcs->ppcs->cdef_search_ctrls.use_reference_cdef_fs) ||
                                pcs->ppcs->enable_restoration ||
#else
                            if (pcs->ppcs->cdef_search_ctrls.enabled || pcs->ppcs->enable_restoration ||
#endif
                                pcs->ppcs->is_ref || scs->static_config.recon_enabled) {
#endif
                                if (pcs->ppcs->frm_hdr.loop_filter_params.filter_level[0] ||
                                    pcs->ppcs->frm_hdr.loop_filter_params.filter_level[1]) {
                                    EbPictureBufferDesc* recon_buffer;
                                    svt_aom_get_recon_pic(pcs, &recon_buffer, ed_ctx->is_16bit);
                                    uint32_t sb_width = MIN(scs->sb_size, pcs->ppcs->aligned_width - sb_origin_x);
                                    uint8_t  last_col = ((sb_origin_x + sb_width) == pcs->ppcs->aligned_width) ? 1 : 0;
                                    svt_aom_loop_filter_sb(
                                        recon_buffer, pcs, sb_origin_y >> 2, sb_origin_x >> 2, 0, 3, last_col);
                                }
#if OPT_DLF
                            }
#endif
                        }

                        ed_ctx->coded_sb_count++;
                    }
                    x_sb_start_index = (x_sb_start_index > 0) ? x_sb_start_index - 1 : 0;
                }
            }

            svt_block_on_mutex(pcs->intra_mutex);
            pcs->intra_coded_area += (uint32_t)ed_ctx->tot_intra_coded_area;
            pcs->skip_coded_area += (uint32_t)ed_ctx->tot_skip_coded_area;
            pcs->hp_coded_area += (uint32_t)ed_ctx->tot_hp_coded_area;
            pcs->avg_cnt_zeromv += (uint32_t)ed_ctx->tot_cnt_zero_mv;
            // Accumulate block selection
            pcs->enc_dec_coded_sb_count += (uint32_t)ed_ctx->coded_sb_count;
            bool last_sb_flag = (pcs->sb_total_count == pcs->enc_dec_coded_sb_count);
            svt_release_mutex(pcs->intra_mutex);

            if (last_sb_flag) {
                bool do_recode = false;
                if ((scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR ||
                     scs->static_config.max_bit_rate != 0) &&
                    scs->enc_ctx->recode_loop != DISALLOW_RECODE) {
                    recode_loop_decision_maker(pcs, scs, &do_recode);
                }

                if (do_recode) {
                    // Deallocate the palette data
                    for (sb_index = 0; sb_index < pcs->enc_dec_coded_sb_count; ++sb_index) {
                        sb_ptr = pcs->sb_ptr_array[sb_index];
                        for (uint16_t blk_cnt = 0; blk_cnt < sb_ptr->final_blk_cnt; blk_cnt++) {
                            EcBlkStruct* final_blk_arr = &(sb_ptr->final_blk_arr[blk_cnt]);
                            if (final_blk_arr->palette_info != NULL) {
                                assert(final_blk_arr->palette_info->color_idx_map != NULL && "free palette:Null");
                                EB_FREE(final_blk_arr->palette_info->color_idx_map);
                                final_blk_arr->palette_info->color_idx_map = NULL;
                                EB_FREE(final_blk_arr->palette_info);
                            }
                        }
                    }
                    pcs->enc_dec_coded_sb_count = 0;
                    // re-init mode decision configuration for qp update for re-encode frame
                    mdc_init_qp_update(pcs);
                    // init segment for re-encode frame
                    svt_aom_init_enc_dec_segement(pcs->ppcs);
                    EbObjectWrapper* enc_dec_re_encode_tasks_wrapper;
                    uint16_t         tg_count = pcs->ppcs->tile_group_cols * pcs->ppcs->tile_group_rows;
                    for (uint16_t tile_group_idx = 0; tile_group_idx < tg_count; tile_group_idx++) {
                        svt_get_empty_object(ed_ctx->enc_dec_feedback_fifo_ptr, &enc_dec_re_encode_tasks_wrapper);

                        EncDecTasks* enc_dec_re_encode_tasks_ptr = (EncDecTasks*)
                                                                       enc_dec_re_encode_tasks_wrapper->object_ptr;
                        enc_dec_re_encode_tasks_ptr->pcs_wrapper      = enc_dec_tasks->pcs_wrapper;
                        enc_dec_re_encode_tasks_ptr->input_type       = ENCDEC_TASKS_MDC_INPUT;
                        enc_dec_re_encode_tasks_ptr->tile_group_index = tile_group_idx;

                        // Post the Full Results Object
                        svt_post_full_object(enc_dec_re_encode_tasks_wrapper);
                    }

                } else {
                    EB_FREE_ARRAY(pcs->ec_ctx_array);
                    // Copy film grain data from parent picture set to the reference object for
                    // further reference
                    if (scs->seq_header.film_grain_params_present) {
                        if (pcs->ppcs->is_ref == true && pcs->ppcs->ref_pic_wrapper) {
                            ((EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr)->film_grain_params =
                                pcs->ppcs->frm_hdr.film_grain_params;
                        }
                    }
                    // Force each frame to update their data so future frames can use it,
                    // even if the current frame did not use it.  This enables REF frames to
                    // have the feature off, while NREF frames can have it on.  Used for
                    // multi-threading.
                    if (pcs->ppcs->is_ref == true && pcs->ppcs->ref_pic_wrapper) {
                        for (int frame = LAST_FRAME; frame <= ALTREF_FRAME; ++frame) {
                            ((EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr)->global_motion[frame] =
                                pcs->ppcs->global_motion[frame];
                        }
                    }
                    svt_memcpy(pcs->ppcs->av1x->sgrproj_restore_cost,
                               pcs->md_rate_est_ctx->sgrproj_restore_fac_bits,
                               2 * sizeof(int32_t));
                    svt_memcpy(pcs->ppcs->av1x->switchable_restore_cost,
                               pcs->md_rate_est_ctx->switchable_restore_fac_bits,
                               3 * sizeof(int32_t));
                    svt_memcpy(pcs->ppcs->av1x->wiener_restore_cost,
                               pcs->md_rate_est_ctx->wiener_restore_fac_bits,
                               2 * sizeof(int32_t));
                    pcs->ppcs->av1x->rdmult =
                        ed_ctx->pic_full_lambda[(ed_ctx->bit_depth == EB_TEN_BIT) ? EB_10_BIT_MD : EB_8_BIT_MD];
                    if (pcs->ppcs->superres_total_recode_loop == 0) {
                        svt_release_object(pcs->ppcs->me_data_wrapper);
                        pcs->ppcs->me_data_wrapper = (EbObjectWrapper*)NULL;
                        pcs->ppcs->pa_me_data      = NULL;
                    }
                    // Get Empty EncDec Results
                    svt_get_empty_object(ed_ctx->enc_dec_output_fifo_ptr, &enc_dec_results_wrapper);
                    enc_dec_results              = (EncDecResults*)enc_dec_results_wrapper->object_ptr;
                    enc_dec_results->pcs_wrapper = enc_dec_tasks->pcs_wrapper;

                    // Post EncDec Results
                    svt_post_full_object(enc_dec_results_wrapper);
                }
            }
        }
        // Release Mode Decision Results
        svt_release_object(enc_dec_tasks_wrapper);
    }
    return NULL;
}
