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

#include <stdlib.h>

#include "enc_handle.h"
#include "utility.h"
#include "pcs.h"
#include "pd_results.h"
#include "me_process.h"
#include "me_results.h"
#include "reference_object.h"
#include "motion_estimation.h"
#include "lambda_rate_tables.h"
#include "compute_sad.h"
#ifdef ARCH_X86_64
#include <emmintrin.h>
#endif
#include "temporal_filtering.h"
#include "global_me.h"

#include "resize.h"
#include "pic_demux_results.h"
#include "rc_tasks.h"
#include "firstpass.h"
#include "initial_rc_process.h"
#include "enc_mode_config.h"

/* --32x32-
|00||01|
|02||03|
--------*/
/* ------16x16-----
|00||01||04||05|
|02||03||06||07|
|08||09||12||13|
|10||11||14||15|
----------------*/
/* ------8x8----------------------------
|00||01||04||05|     |16||17||20||21|
|02||03||06||07|     |18||19||22||23|
|08||09||12||13|     |24||25||28||29|
|10||11||14||15|     |26||27||30||31|

|32||33||36||37|     |48||49||52||53|
|34||35||38||39|     |50||51||54||55|
|40||41||44||45|     |56||57||60||61|
|42||43||46||47|     |58||59||62||63|
-------------------------------------*/

void dg_detector_hme_level0(PictureParentControlSet* ppcs, uint32_t seg_idx);

static void motion_estimation_context_dctor(EbPtr p) {
    EbThreadContext*           thread_ctx = (EbThreadContext*)p;
    MotionEstimationContext_t* obj        = (MotionEstimationContext_t*)thread_ctx->priv;
    EB_DELETE(obj->me_ctx);
    EB_FREE_ARRAY(obj);
}

/************************************************
 * Motion Analysis Context Constructor
 ************************************************/
EbErrorType svt_aom_motion_estimation_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                   int index) {
    MotionEstimationContext_t* me_context_ptr;

    EB_CALLOC_ARRAY(me_context_ptr, 1);
    thread_ctx->priv                                        = me_context_ptr;
    thread_ctx->dctor                                       = motion_estimation_context_dctor;
    me_context_ptr->picture_decision_results_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->picture_decision_results_resource_ptr, index);
    me_context_ptr->motion_estimation_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->motion_estimation_results_resource_ptr, index);
    EB_NEW(me_context_ptr->me_ctx, svt_aom_me_context_ctor);
    return EB_ErrorNone;
}

/************************************************
 * Motion Analysis Kernel
 * The Motion Analysis performs  Motion Estimation
 * This process has access to the current input picture as well as
 * the input pictures, which the current picture references according
 * to the prediction structure pattern.  The Motion Analysis process is multithreaded,
 * so pictures can be processed out of order as long as all inputs are available.
 ************************************************/
void* svt_aom_motion_estimation_kernel(void* input_ptr) {
    EbThreadContext*           thread_ctx     = (EbThreadContext*)input_ptr;
    MotionEstimationContext_t* me_context_ptr = (MotionEstimationContext_t*)thread_ctx->priv;
    EbObjectWrapper*           in_results_wrapper_ptr;
    EbObjectWrapper*           out_results_wrapper;
    for (;;) {
        // Get Input Full Object
        EB_GET_FULL_OBJECT(me_context_ptr->picture_decision_results_input_fifo_ptr, &in_results_wrapper_ptr);
        PictureDecisionResults*  in_results_ptr = (PictureDecisionResults*)in_results_wrapper_ptr->object_ptr;
        PictureParentControlSet* pcs            = (PictureParentControlSet*)in_results_ptr->pcs_wrapper->object_ptr;
        SequenceControlSet*      scs            = pcs->scs;
        if (in_results_ptr->task_type == TASK_TFME) {
            me_context_ptr->me_ctx->me_type = ME_MCTF;
        } else if (in_results_ptr->task_type == TASK_PAME || in_results_ptr->task_type == TASK_SUPERRES_RE_ME) {
            me_context_ptr->me_ctx->me_type = ME_OPEN_LOOP;
        } else if (in_results_ptr->task_type == TASK_DG_DETECTOR_HME) {
            me_context_ptr->me_ctx->me_type = ME_DG_DETECTOR;
        }

        // ME Kernel Signal(s) derivation
        if ((in_results_ptr->task_type == TASK_PAME) || (in_results_ptr->task_type == TASK_SUPERRES_RE_ME)) {
            svt_aom_sig_deriv_me(scs, pcs, me_context_ptr->me_ctx);
        }

        else if (in_results_ptr->task_type == TASK_TFME) {
            svt_aom_sig_deriv_me_tf(pcs, me_context_ptr->me_ctx);
        }

        if ((in_results_ptr->task_type == TASK_PAME) || (in_results_ptr->task_type == TASK_SUPERRES_RE_ME)) {
            EbPictureBufferDesc* sixteenth_picture_ptr;
            EbPictureBufferDesc* quarter_picture_ptr;
            EbPictureBufferDesc* input_padded_pic;
            EbPictureBufferDesc* input_pic;
            EbPaReferenceObject* pa_ref_obj_;

            //assert((int)pcs->pa_ref_pic_wrapper->live_count > 0);
            pa_ref_obj_ = (EbPaReferenceObject*)pcs->pa_ref_pic_wrapper->object_ptr;
            // Set 1/4 and 1/16 ME input buffer(s); filtered or decimated
            quarter_picture_ptr   = pa_ref_obj_->quarter_downsampled_picture_ptr;
            sixteenth_picture_ptr = pa_ref_obj_->sixteenth_downsampled_picture_ptr;
            input_padded_pic      = pa_ref_obj_->input_padded_pic;

            input_pic = pcs->enhanced_pic;

            // Segments
            uint32_t segment_index         = in_results_ptr->segment_index;
            uint32_t pic_width_in_b64      = (pcs->aligned_width + scs->b64_size - 1) / scs->b64_size;
            uint32_t picture_height_in_b64 = (pcs->aligned_height + scs->b64_size - 1) / scs->b64_size;
            uint32_t y_segment_index;
            uint32_t x_segment_index;

            SEGMENT_CONVERT_IDX_TO_XY(segment_index, x_segment_index, y_segment_index, pcs->me_segments_column_count);
            uint32_t x_b64_start_index = SEGMENT_START_IDX(
                x_segment_index, pic_width_in_b64, pcs->me_segments_column_count);
            uint32_t x_b64_end_index = SEGMENT_END_IDX(
                x_segment_index, pic_width_in_b64, pcs->me_segments_column_count);
            uint32_t y_b64_start_index = SEGMENT_START_IDX(
                y_segment_index, picture_height_in_b64, pcs->me_segments_row_count);
            uint32_t y_b64_end_index = SEGMENT_END_IDX(
                y_segment_index, picture_height_in_b64, pcs->me_segments_row_count);

            bool skip_me = false;
            if (svt_aom_is_pic_skipped(pcs)) {
                skip_me = true;
            }
            // skip me for the first pass. ME is already performed
            if (!skip_me) {
                if (pcs->slice_type != I_SLICE) {
                    // Use scaled source references if resolution of the reference is different that of the input
                    svt_aom_use_scaled_source_refs_if_needed(
                        pcs, input_pic, pa_ref_obj_, &input_padded_pic, &quarter_picture_ptr, &sixteenth_picture_ptr);

                    // 64x64 Block Loop
                    for (uint32_t y_b64_index = y_b64_start_index; y_b64_index < y_b64_end_index; ++y_b64_index) {
                        for (uint32_t x_b64_index = x_b64_start_index; x_b64_index < x_b64_end_index; ++x_b64_index) {
                            uint32_t b64_index = (uint16_t)(x_b64_index + y_b64_index * pic_width_in_b64);

                            uint32_t b64_origin_x = x_b64_index * scs->b64_size;
                            uint32_t b64_origin_y = y_b64_index * scs->b64_size;

                            // Load the 64x64 Block from the input to the intermediate block buffer
                            uint32_t buffer_index = (input_pic->org_y + b64_origin_y) * input_pic->stride_y +
                                input_pic->org_x + b64_origin_x;
#ifdef ARCH_X86_64
                            uint8_t* src_ptr    = &input_padded_pic->buffer_y[buffer_index];
                            uint32_t b64_height = (pcs->aligned_height - b64_origin_y) < BLOCK_SIZE_64
                                ? pcs->aligned_height - b64_origin_y
                                : BLOCK_SIZE_64;
                            //_MM_HINT_T0     //_MM_HINT_T1    //_MM_HINT_T2//_MM_HINT_NTA
                            for (uint32_t i = 0; i < b64_height; i++) {
                                char const* p = (char const*)(src_ptr + i * input_padded_pic->stride_y);
                                _mm_prefetch(p, _MM_HINT_T2);
                            }
#endif
                            me_context_ptr->me_ctx->b64_src_ptr    = &input_padded_pic->buffer_y[buffer_index];
                            me_context_ptr->me_ctx->b64_src_stride = input_padded_pic->stride_y;

                            // Load the 1/4 decimated SB from the 1/4 decimated input to the 1/4 intermediate SB buffer
                            if (me_context_ptr->me_ctx->enable_hme_level1_flag) {
                                buffer_index = (quarter_picture_ptr->org_y + (b64_origin_y >> 1)) *
                                        quarter_picture_ptr->stride_y +
                                    quarter_picture_ptr->org_x + (b64_origin_x >> 1);

                                me_context_ptr->me_ctx->quarter_b64_buffer =
                                    &quarter_picture_ptr->buffer_y[buffer_index];
                                me_context_ptr->me_ctx->quarter_b64_buffer_stride = quarter_picture_ptr->stride_y;
                            }

                            // Load the 1/16 decimated SB from the 1/16 decimated input to the 1/16 intermediate SB buffer
                            if (me_context_ptr->me_ctx->enable_hme_level0_flag) {
                                buffer_index = (sixteenth_picture_ptr->org_y + (b64_origin_y >> 2)) *
                                        sixteenth_picture_ptr->stride_y +
                                    sixteenth_picture_ptr->org_x + (b64_origin_x >> 2);

                                me_context_ptr->me_ctx->sixteenth_b64_buffer =
                                    &sixteenth_picture_ptr->buffer_y[buffer_index];
                                me_context_ptr->me_ctx->sixteenth_b64_buffer_stride = sixteenth_picture_ptr->stride_y;
                            }

                            me_context_ptr->me_ctx->me_type = ME_OPEN_LOOP;

                            if ((in_results_ptr->task_type == TASK_PAME) ||
                                (in_results_ptr->task_type == TASK_SUPERRES_RE_ME)) {
                                me_context_ptr->me_ctx->num_of_list_to_search = MAX_NUM_OF_REF_PIC_LIST;

                                me_context_ptr->me_ctx->num_of_ref_pic_to_search[0] = pcs->ref_list0_count_try;
                                me_context_ptr->me_ctx->num_of_ref_pic_to_search[1] = pcs->ref_list1_count_try;
                                me_context_ptr->me_ctx->temporal_layer_index        = pcs->temporal_layer_index;
                                me_context_ptr->me_ctx->is_ref                      = pcs->is_ref;

                                if (pcs->frame_superres_enabled || pcs->frame_resize_enabled) {
                                    for (int i = 0; i < me_context_ptr->me_ctx->num_of_list_to_search; i++) {
                                        for (int j = 0; j < me_context_ptr->me_ctx->num_of_ref_pic_to_search[i]; j++) {
                                            //assert((int)pcs->ref_pa_pic_ptr_array[i][j]->live_count > 0);
                                            uint8_t sr_denom_idx     = svt_aom_get_denom_idx(pcs->superres_denom);
                                            uint8_t resize_denom_idx = svt_aom_get_denom_idx(pcs->resize_denom);
                                            EbPaReferenceObject* ref_object =
                                                (EbPaReferenceObject*)pcs->ref_pa_pic_ptr_array[i][j]->object_ptr;
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].picture_ptr =
                                                ref_object->downscaled_input_padded_picture_ptr[sr_denom_idx]
                                                                                               [resize_denom_idx];
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].quarter_picture_ptr =
                                                ref_object
                                                    ->downscaled_quarter_downsampled_picture_ptr[sr_denom_idx]
                                                                                                [resize_denom_idx];
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].sixteenth_picture_ptr =
                                                ref_object
                                                    ->downscaled_sixteenth_downsampled_picture_ptr[sr_denom_idx]
                                                                                                  [resize_denom_idx];
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].picture_number =
                                                ref_object->picture_number;
                                        }
                                    }
                                } else {
                                    for (int i = 0; i < me_context_ptr->me_ctx->num_of_list_to_search; i++) {
                                        for (int j = 0; j < me_context_ptr->me_ctx->num_of_ref_pic_to_search[i]; j++) {
                                            //assert((int)pcs->ref_pa_pic_ptr_array[i][j]->live_count > 0);
                                            EbPaReferenceObject* ref_object =
                                                (EbPaReferenceObject*)pcs->ref_pa_pic_ptr_array[i][j]->object_ptr;
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].picture_ptr =
                                                ref_object->input_padded_pic;
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].quarter_picture_ptr =
                                                ref_object->quarter_downsampled_picture_ptr;
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].sixteenth_picture_ptr =
                                                ref_object->sixteenth_downsampled_picture_ptr;
                                            me_context_ptr->me_ctx->me_ds_ref_array[i][j].picture_number =
                                                ref_object->picture_number;
                                        }
                                    }
                                }
                            }

                            svt_aom_motion_estimation_b64(
                                pcs, b64_index, b64_origin_x, b64_origin_y, me_context_ptr->me_ctx, input_pic);

                            if ((in_results_ptr->task_type == TASK_PAME) ||
                                (in_results_ptr->task_type == TASK_SUPERRES_RE_ME)) {
                                svt_block_on_mutex(pcs->me_processed_b64_mutex);
                                pcs->me_processed_b64_count++;
                                // We need to finish ME for all SBs to do GM
                                if (pcs->me_processed_b64_count == pcs->b64_total_count) {
                                    if (pcs->gm_ctrls.enabled && (!pcs->gm_ctrls.pp_enabled || pcs->gm_pp_detected)) {
                                        svt_aom_global_motion_estimation(pcs, input_pic);
                                    } else {
                                        // Initilize global motion to be OFF when GM is OFF
                                        memset(
                                            pcs->is_global_motion, false, MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH);
                                    }
                                }

                                svt_release_mutex(pcs->me_processed_b64_mutex);
                            }
                        }
                    }
                }
            }
            // Get Empty Results Object
            svt_get_empty_object(me_context_ptr->motion_estimation_results_output_fifo_ptr, &out_results_wrapper);

            MotionEstimationResults* out_results = (MotionEstimationResults*)out_results_wrapper->object_ptr;
            out_results->pcs_wrapper             = in_results_ptr->pcs_wrapper;
            out_results->segment_index           = segment_index;
            out_results->task_type               = in_results_ptr->task_type;
            // Release the Input Results
            svt_release_object(in_results_wrapper_ptr);

            // Post the Full Results Object
            svt_post_full_object(out_results_wrapper);
        } else if (in_results_ptr->task_type == TASK_TFME) {
            //gm pre-processing for only base B
            if (pcs->gm_ctrls.pp_enabled && pcs->gm_pp_enabled && in_results_ptr->segment_index == 0) {
                svt_aom_gm_pre_processor(pcs, pcs->temp_filt_pcs_list);
            }
            // temporal filtering start
            me_context_ptr->me_ctx->me_type = ME_MCTF;
            svt_av1_init_temporal_filtering(
                pcs->temp_filt_pcs_list, pcs, me_context_ptr, in_results_ptr->segment_index);

            // Release the Input Results
            svt_release_object(in_results_wrapper_ptr);
        } else if (in_results_ptr->task_type == TASK_DG_DETECTOR_HME) {
            // dynamic gop detector
            dg_detector_hme_level0(pcs, in_results_ptr->segment_index);

            // Release the Input Results
            svt_release_object(in_results_wrapper_ptr);
        }
    }

    return NULL;
}
