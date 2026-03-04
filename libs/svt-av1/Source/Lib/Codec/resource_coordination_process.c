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
#include <string.h>

#include "enc_handle.h"
#include "sys_resource_manager.h"
#include "pcs.h"
#include "sequence_control_set.h"
#include "pic_buffer_desc.h"
#include "resource_coordination_process.h"
#include "resource_coordination_results.h"
#include "transforms.h"
#include "resize.h"
#include "svt_time.h"
#include "object.h"
#include "svt_log.h"
#include "pass2_strategy.h"
#include "common_dsp_rtcd.h"
#include "metadata_handle.h"
#include "enc_mode_config.h"

typedef struct ResourceCoordinationContext {
    EbFifo*                       input_cmd_fifo_ptr;
    EbFifo*                       resource_coordination_results_output_fifo_ptr;
    EbFifo*                       picture_control_set_fifo_ptr;
    EbSequenceControlSetInstance* scs_instance;
    EbObjectWrapper*              scs_active;
    EbFifo*                       scs_empty_fifo_ptr;
    EbCallback*                   app_callback_ptr;
    uint64_t                      picture_number;
    uint64_t                      average_enc_mod;
    uint8_t                       prev_enc_mod;
    int8_t                        prev_enc_mode_delta;
    uint8_t                       prev_change_cond;

    int64_t previous_mode_change_buffer;
    int64_t previous_mode_change_frame_in;
    int64_t previous_buffer_check1;
    int64_t previous_frame_in_check1;
    int64_t previous_frame_in_check2;
    int64_t previous_frame_in_check3;

    uint64_t cur_speed; // speed x 1000
    uint64_t prevs_time_seconds;
    uint64_t prevs_timeu_seconds;
    int64_t  prev_frame_out;

    uint64_t first_in_pic_arrived_time_seconds;
    uint64_t first_in_pic_arrived_timeu_seconds;
    bool     start_flag;

    // Sequence Parameter Change Flags
    bool seq_param_change;
    bool video_res_change;

} ResourceCoordinationContext;

static void resource_coordination_context_dctor(EbPtr p) {
    EbThreadContext* thread_contxt_ptr = (EbThreadContext*)p;
    if (thread_contxt_ptr->priv) {
        ResourceCoordinationContext* obj = (ResourceCoordinationContext*)thread_contxt_ptr->priv;
        EB_FREE_ARRAY(obj);
    }
}

/************************************************
 * Resource Coordination Context Constructor
 ************************************************/
EbErrorType svt_aom_resource_coordination_context_ctor(EbThreadContext* thread_contxt_ptr,
                                                       EbEncHandle*     enc_handle_ptr) {
    ResourceCoordinationContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_contxt_ptr->priv  = context_ptr;
    thread_contxt_ptr->dctor = resource_coordination_context_dctor;

    //ResourceCoordination works with ParentPCS
    context_ptr->picture_control_set_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_parent_control_set_pool_ptr, 0);
    context_ptr->input_cmd_fifo_ptr = svt_system_resource_get_consumer_fifo(enc_handle_ptr->input_cmd_resource_ptr, 0);
    context_ptr->resource_coordination_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->resource_coordination_results_resource_ptr, 0);
    context_ptr->scs_instance                = enc_handle_ptr->scs_instance;
    context_ptr->scs_active                  = NULL;
    context_ptr->scs_empty_fifo_ptr          = svt_system_resource_get_producer_fifo(enc_handle_ptr->scs_pool_ptr, 0);
    context_ptr->app_callback_ptr            = enc_handle_ptr->app_callback_ptr;
    context_ptr->picture_number              = 0;
    context_ptr->average_enc_mod             = 0;
    context_ptr->prev_enc_mod                = 0;
    context_ptr->prev_enc_mode_delta         = 0;
    context_ptr->cur_speed                   = 0; // speed x 1000
    context_ptr->previous_mode_change_buffer = 0;
    context_ptr->first_in_pic_arrived_time_seconds  = 0;
    context_ptr->first_in_pic_arrived_timeu_seconds = 0;
    context_ptr->previous_frame_in_check1           = 0;
    context_ptr->previous_frame_in_check2           = 0;
    context_ptr->previous_frame_in_check3           = 0;
    context_ptr->previous_mode_change_frame_in      = 0;
    context_ptr->prevs_time_seconds                 = 0;
    context_ptr->prevs_timeu_seconds                = 0;
    context_ptr->prev_frame_out                     = 0;
    context_ptr->start_flag                         = false;

    context_ptr->previous_buffer_check1 = 0;
    context_ptr->prev_change_cond       = 0;

    context_ptr->seq_param_change = 0;
    context_ptr->video_res_change = 0;
    return EB_ErrorNone;
}

//******************************************************************************//
// Modify the Enc mode based on the buffer Status
// Inputs: TargetSpeed, Status of the SCbuffer
// Output: EncMod
//******************************************************************************//
void speed_buffer_control(ResourceCoordinationContext* context_ptr, PictureParentControlSet* pcs,
                          SequenceControlSet* scs) {
    uint64_t curs_time_seconds  = 0;
    uint64_t curs_time_useconds = 0;
    double   overall_duration   = 0.0;
    double   inst_duration      = 0.0;
    int8_t   encoder_mode_delta = 0;
    int64_t  input_frames_count = 0;
    int8_t   change_cond        = 0;
    int64_t  target_fps         = (60 >> 16);

    int64_t buffer_threshold_1 = SC_FRAMES_INTERVAL_T1;
    int64_t buffer_threshold_2 = SC_FRAMES_INTERVAL_T2;
    int64_t buffer_threshold_3 = MIN(target_fps * 3, SC_FRAMES_INTERVAL_T3);
    svt_block_on_mutex(scs->enc_ctx->sc_buffer_mutex);

    if (scs->enc_ctx->sc_frame_in == 0) {
        svt_av1_get_time(&context_ptr->first_in_pic_arrived_time_seconds,
                         &context_ptr->first_in_pic_arrived_timeu_seconds);
    } else if (scs->enc_ctx->sc_frame_in == SC_FRAMES_TO_IGNORE) {
        context_ptr->start_flag = true;
    }
    // Compute duration since the start of the encode and since the previous checkpoint
    svt_av1_get_time(&curs_time_seconds, &curs_time_useconds);

    overall_duration = svt_av1_compute_overall_elapsed_time_ms(context_ptr->first_in_pic_arrived_time_seconds,
                                                               context_ptr->first_in_pic_arrived_timeu_seconds,
                                                               curs_time_seconds,
                                                               curs_time_useconds);

    inst_duration = svt_av1_compute_overall_elapsed_time_ms(
        context_ptr->prevs_time_seconds, context_ptr->prevs_timeu_seconds, curs_time_seconds, curs_time_useconds);

    input_frames_count      = (int64_t)overall_duration * (60 >> 16) / 1000;
    scs->enc_ctx->sc_buffer = input_frames_count - scs->enc_ctx->sc_frame_in;

    encoder_mode_delta = 0;

    // Check every bufferTsshold1 for the changes (previous_frame_in_check1 variable)
    if ((scs->enc_ctx->sc_frame_in > context_ptr->previous_frame_in_check1 + buffer_threshold_1 &&
         scs->enc_ctx->sc_frame_in >= SC_FRAMES_TO_IGNORE)) {
        // Go to a slower mode based on the fullness and changes of the buffer
        if (scs->enc_ctx->sc_buffer < target_fps &&
            (context_ptr->prev_enc_mode_delta > -1 ||
             scs->enc_ctx->sc_frame_in > context_ptr->previous_mode_change_frame_in + target_fps * 2)) {
            if (context_ptr->previous_buffer_check1 > scs->enc_ctx->sc_buffer + buffer_threshold_1) {
                encoder_mode_delta += -1;
                change_cond = 2;
            } else if (context_ptr->previous_mode_change_buffer > buffer_threshold_1 + scs->enc_ctx->sc_buffer &&
                       scs->enc_ctx->sc_buffer < buffer_threshold_1) {
                encoder_mode_delta += -1;
                change_cond = 4;
            }
        }

        // Go to a faster mode based on the fullness and changes of the buffer
        if (scs->enc_ctx->sc_buffer > buffer_threshold_1 + context_ptr->previous_buffer_check1) {
            encoder_mode_delta += +1;
            change_cond = 1;
        } else if (scs->enc_ctx->sc_buffer > buffer_threshold_1 + context_ptr->previous_mode_change_buffer) {
            encoder_mode_delta += +1;
            change_cond = 3;
        }

        // Update the encode mode based on the fullness of the buffer
        // If previous ChangeCond was the same, double the threshold2
        if (scs->enc_ctx->sc_buffer > buffer_threshold_3 &&
            (context_ptr->prev_change_cond != 7 ||
             scs->enc_ctx->sc_frame_in > context_ptr->previous_mode_change_frame_in + buffer_threshold_2 * 2) &&
            scs->enc_ctx->sc_buffer > context_ptr->previous_mode_change_buffer) {
            encoder_mode_delta += 1;
            change_cond = 7;
        }
        encoder_mode_delta     = CLIP3(-1, 1, encoder_mode_delta);
        scs->enc_ctx->enc_mode = (EncMode)CLIP3(1, MAX_ENC_PRESET, (int8_t)scs->enc_ctx->enc_mode + encoder_mode_delta);

        // Update previous stats
        context_ptr->previous_frame_in_check1 = scs->enc_ctx->sc_frame_in;
        context_ptr->previous_buffer_check1   = scs->enc_ctx->sc_buffer;

        if (encoder_mode_delta) {
            context_ptr->previous_mode_change_buffer   = scs->enc_ctx->sc_buffer;
            context_ptr->previous_mode_change_frame_in = scs->enc_ctx->sc_frame_in;
            context_ptr->prev_enc_mode_delta           = encoder_mode_delta;
        }
    }

    // Check every buffer_threshold_2 for the changes (previous_frame_in_check2 variable)
    if ((scs->enc_ctx->sc_frame_in > context_ptr->previous_frame_in_check2 + buffer_threshold_2 &&
         scs->enc_ctx->sc_frame_in >= SC_FRAMES_TO_IGNORE)) {
        encoder_mode_delta = 0;

        // if no change in the encoder mode and buffer is low enough and level is not increasing,
        // switch to a slower encoder mode If previous ChangeCond was the same, double the
        // threshold2
        if (scs->enc_ctx->sc_frame_in > context_ptr->previous_mode_change_frame_in + buffer_threshold_2 &&
            (context_ptr->prev_change_cond != 8 ||
             scs->enc_ctx->sc_frame_in > context_ptr->previous_mode_change_frame_in + buffer_threshold_2 * 2) &&
            ((scs->enc_ctx->sc_buffer - context_ptr->previous_mode_change_buffer < (target_fps / 3)) ||
             context_ptr->previous_mode_change_buffer == 0) &&
            scs->enc_ctx->sc_buffer < buffer_threshold_3) {
            encoder_mode_delta = -1;
            change_cond        = 8;
        }

        encoder_mode_delta     = CLIP3(-1, 1, encoder_mode_delta);
        scs->enc_ctx->enc_mode = (EncMode)CLIP3(1, MAX_ENC_PRESET, (int8_t)scs->enc_ctx->enc_mode + encoder_mode_delta);

        // Update previous stats
        context_ptr->previous_frame_in_check2 = scs->enc_ctx->sc_frame_in;

        if (encoder_mode_delta) {
            context_ptr->previous_mode_change_buffer   = scs->enc_ctx->sc_buffer;
            context_ptr->previous_mode_change_frame_in = scs->enc_ctx->sc_frame_in;
            context_ptr->prev_enc_mode_delta           = encoder_mode_delta;
        }
    }
    // Check every SC_FRAMES_INTERVAL_SPEED frames for the speed calculation
    // (previous_frame_in_check3 variable)
    if (context_ptr->start_flag ||
        (scs->enc_ctx->sc_frame_in > context_ptr->previous_frame_in_check3 + SC_FRAMES_INTERVAL_SPEED &&
         scs->enc_ctx->sc_frame_in >= SC_FRAMES_TO_IGNORE)) {
        if (context_ptr->start_flag) {
            context_ptr->cur_speed = (uint64_t)(scs->enc_ctx->sc_frame_out - 0) * 1000 / (uint64_t)(overall_duration);
        } else {
            if (inst_duration != 0) {
                context_ptr->cur_speed = (uint64_t)(scs->enc_ctx->sc_frame_out - context_ptr->prev_frame_out) * 1000 /
                    (uint64_t)(inst_duration);
            }
        }
        context_ptr->start_flag = false;

        // Update previous stats
        context_ptr->previous_frame_in_check3 = scs->enc_ctx->sc_frame_in;
        context_ptr->prevs_time_seconds       = curs_time_seconds;
        context_ptr->prevs_timeu_seconds      = curs_time_useconds;
        context_ptr->prev_frame_out           = scs->enc_ctx->sc_frame_out;
    } else if (scs->enc_ctx->sc_frame_in < SC_FRAMES_TO_IGNORE && (overall_duration != 0)) {
        context_ptr->cur_speed = (uint64_t)(scs->enc_ctx->sc_frame_out - 0) * 1000 / (uint64_t)(overall_duration);
    }
    if (change_cond) {
        context_ptr->prev_change_cond = change_cond;
    }
    scs->enc_ctx->sc_frame_in++;
    if (scs->enc_ctx->sc_frame_in >= SC_FRAMES_TO_IGNORE) {
        context_ptr->average_enc_mod += scs->enc_ctx->enc_mode;
    } else {
        context_ptr->average_enc_mod = 0;
    }
    // Set the encoder level
    pcs->enc_mode = scs->enc_ctx->enc_mode;

    svt_release_mutex(scs->enc_ctx->sc_buffer_mutex);
    context_ptr->prev_enc_mod = scs->enc_ctx->enc_mode;
}

// Film grain (assigning the random-seed)
static void assign_film_grain_random_seed(PictureParentControlSet* pcs) {
    uint16_t* fgn_random_seed_ptr              = &pcs->scs->film_grain_random_seed;
    pcs->frm_hdr.film_grain_params.random_seed = *fgn_random_seed_ptr;
    *fgn_random_seed_ptr += 3381; // Changing random seed for film grain
    if (!(*fgn_random_seed_ptr)) { // Random seed should not be zero
        *fgn_random_seed_ptr += 7391;
    }
}

static uint8_t get_delta_q_res(uint8_t qp, bool enable_variance_boost) {
    uint8_t res = DEFAULT_DELTA_Q_RES;

    if (enable_variance_boost) {
        // use the (sequence) qp value to determine delta_q_res
        uint8_t qindex = quantizer_to_qindex[qp];

        // determine delta_q_res based on qindex
        // delta q overhead becomes proportionally bigger the higher the qindex,
        // and qstep jumps between qindexes become bigger the lower the qindex
        // so dynamically increase delta_q_res granularity as qindex decreases
        if (qindex >= 160) {
            res = 8;
        } else if (qindex >= 120) {
            res = 4;
        } else if (qindex >= 80) {
            res = 2;
        } else {
            res = DEFAULT_DELTA_Q_RES;
        }
    }

    return res;
}

static EbErrorType reset_pcs_av1(PictureParentControlSet* pcs) {
    FrameHeader* frm_hdr     = &pcs->frm_hdr;
    Av1Common*   cm          = pcs->av1_cm;
    pcs->filt_to_unfilt_diff = (uint32_t)~0;
    pcs->gm_pp_detected      = false;
    pcs->gm_pp_enabled       = false;
    pcs->is_gm_on            = -1;
    pcs->gf_interval         = 0;

    pcs->reference_released                     = 0;
    frm_hdr->skip_mode_params.skip_mode_allowed = 0;
    frm_hdr->skip_mode_params.skip_mode_flag    = 0;
    frm_hdr->frame_type                         = KEY_FRAME;
    frm_hdr->show_frame                         = 1;
    frm_hdr->showable_frame                     = 1; // frame can be used as show existing frame in future

    frm_hdr->disable_cdf_update      = 0;
    frm_hdr->allow_high_precision_mv = 0;
    frm_hdr->force_integer_mv        = 0; // 0 the default in AOM, 1 only integer
    frm_hdr->allow_warped_motion     = 0;

    /* profile settings */
#if CONFIG_ENTROPY_STATS
    int32_t coef_cdf_category;
#endif

    frm_hdr->quantization_params.base_q_idx              = 31;
    frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_Y] = 0;
    frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_Y] = pcs->scs->static_config.luma_y_dc_qindex_offset;
    frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_U] = pcs->scs->static_config.chroma_u_ac_qindex_offset;
    frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_U] = pcs->scs->static_config.chroma_u_dc_qindex_offset;
    frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_V] = pcs->scs->static_config.chroma_v_ac_qindex_offset;
    frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_V] = pcs->scs->static_config.chroma_v_dc_qindex_offset;

    // Encoder
    frm_hdr->quantization_params.using_qmatrix   = pcs->scs->static_config.enable_qm;
    frm_hdr->quantization_params.qm[AOM_PLANE_Y] = 5;
    frm_hdr->quantization_params.qm[AOM_PLANE_U] = 5;
    frm_hdr->quantization_params.qm[AOM_PLANE_V] = 5;
    frm_hdr->is_motion_mode_switchable           = 0;
    // Flag signaling how frame contexts should be updated at the end of
    // a frame decode
    pcs->refresh_frame_context = REFRESH_FRAME_CONTEXT_DISABLED;

    frm_hdr->loop_filter_params.filter_level[0] = 0;
    frm_hdr->loop_filter_params.filter_level[1] = 0;
    frm_hdr->loop_filter_params.filter_level_u  = 0;
    frm_hdr->loop_filter_params.filter_level_v  = 0;
    frm_hdr->loop_filter_params.sharpness_level = 0;

    frm_hdr->loop_filter_params.mode_ref_delta_enabled = 0;
    frm_hdr->loop_filter_params.mode_ref_delta_update  = 0;
    frm_hdr->loop_filter_params.mode_deltas[0]         = 0;
    frm_hdr->loop_filter_params.mode_deltas[1]         = 0;

    frm_hdr->loop_filter_params.ref_deltas[0] = 1;
    frm_hdr->loop_filter_params.ref_deltas[1] = 0;
    frm_hdr->loop_filter_params.ref_deltas[2] = 0;
    frm_hdr->loop_filter_params.ref_deltas[3] = 0;
    frm_hdr->loop_filter_params.ref_deltas[4] = -1;
    frm_hdr->loop_filter_params.ref_deltas[5] = 0;
    frm_hdr->loop_filter_params.ref_deltas[6] = -1;
    frm_hdr->loop_filter_params.ref_deltas[7] = -1;

    frm_hdr->all_lossless   = 0;
    frm_hdr->coded_lossless = 0;
    frm_hdr->reduced_tx_set = 0;
    frm_hdr->reference_mode = SINGLE_REFERENCE;
    pcs->frame_context_idx  = 0; /* Context to use/update */
    for (int32_t i = 0; i < REF_FRAMES; i++) {
        pcs->fb_of_context_type[i] = 0;
    }
    frm_hdr->primary_ref_frame = PRIMARY_REF_NONE;
    if (pcs->scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR &&
        pcs->scs->static_config.intra_period_length != -1) {
        pcs->frame_offset = pcs->picture_number % (pcs->scs->static_config.intra_period_length + 1);
    } else {
        pcs->frame_offset = pcs->picture_number;
    }
    frm_hdr->error_resilient_mode            = 0;
    cm->tiles_info.uniform_tile_spacing_flag = 1;
    pcs->large_scale_tile                    = 0;

    //cdef_pri_damping & cdef_sec_damping are consolidated to cdef_damping
    frm_hdr->cdef_params.cdef_damping = 0;
    //pcs->cdef_pri_damping = 0;
    //pcs->cdef_sec_damping = 0;

    pcs->nb_cdef_strengths = 1;
    for (int32_t i = 0; i < CDEF_MAX_STRENGTHS; i++) {
        frm_hdr->cdef_params.cdef_y_strength[i]  = 0;
        frm_hdr->cdef_params.cdef_uv_strength[i] = 0;
    }
    frm_hdr->cdef_params.cdef_bits            = 0;
    frm_hdr->delta_q_params.delta_q_present   = 1;
    frm_hdr->delta_lf_params.delta_lf_present = 0;

    frm_hdr->delta_q_params.delta_q_res = get_delta_q_res((uint8_t)pcs->scs->static_config.qp,
                                                          pcs->scs->static_config.enable_variance_boost);

    frm_hdr->delta_lf_params.delta_lf_present = 0;
    frm_hdr->delta_lf_params.delta_lf_res     = 0;
    frm_hdr->delta_lf_params.delta_lf_multi   = 0;

    frm_hdr->current_frame_id           = 0;
    frm_hdr->frame_refs_short_signaling = 0;
    pcs->allow_comp_inter_inter         = 0;
    //  int32_t all_one_sided_refs;
    pcs->me_data_wrapper               = NULL;
    pcs->downscaled_pic_wrapper        = NULL;
    pcs->ds_pics.picture_ptr           = NULL;
    pcs->ds_pics.quarter_picture_ptr   = NULL;
    pcs->ds_pics.sixteenth_picture_ptr = NULL;
    pcs->max_number_of_pus_per_sb      = SQUARE_PU_COUNT;

    svt_aom_atomic_set_u32(&pcs->pa_me_done, 0);

    svt_create_cond_var(&pcs->me_ready);

    SequenceControlSet* scs           = pcs->scs;
    pcs->me_segments_completion_count = 0;
    pcs->me_segments_column_count     = (uint8_t)(scs->me_segment_col_count_array);
    pcs->me_segments_row_count        = (uint8_t)(scs->me_segment_row_count_array);

    pcs->me_segments_total_count = (uint16_t)(pcs->me_segments_column_count * pcs->me_segments_row_count);
    pcs->tpl_disp_coded_sb_count = 0;

    pcs->tpl_src_data_ready  = 0;
    pcs->tf_motion_direction = -1;

    // Assign the film-grain random-seed
    assign_film_grain_random_seed(pcs);

    return EB_ErrorNone;
}

/***********************************************
**** Copy the input buffer from the
**** sample application to the library buffers
************************************************/
static EbErrorType copy_frame_buffer_overlay(SequenceControlSet* scs, uint8_t* dst, uint8_t* src) {
    EbSvtAv1EncConfiguration* config       = &scs->static_config;
    EbErrorType               return_error = EB_ErrorNone;

    EbPictureBufferDesc* dst_picture_ptr = (EbPictureBufferDesc*)dst;
    EbPictureBufferDesc* src_picture_ptr = (EbPictureBufferDesc*)src;
    bool                 is_16bit_input  = config->encoder_bit_depth > EB_EIGHT_BIT;

    // Need to include for Interlacing on the fly with pictureScanType = 1

    if (!is_16bit_input) {
        uint16_t input_row_index;
        uint32_t luma_buffer_offset = (dst_picture_ptr->stride_y * scs->top_padding + scs->left_padding)
            << is_16bit_input;
        uint32_t chroma_buffer_offset =
            (dst_picture_ptr->stride_cr * (scs->top_padding >> 1) + (scs->left_padding >> 1)) << is_16bit_input;
        uint16_t luma_stride   = dst_picture_ptr->stride_y << is_16bit_input;
        uint16_t chroma_stride = dst_picture_ptr->stride_cb << is_16bit_input;
        uint16_t luma_width    = (uint16_t)(dst_picture_ptr->width - scs->max_input_pad_right) << is_16bit_input;
        uint16_t chroma_width  = (luma_width >> 1) << is_16bit_input;
        uint16_t luma_height   = (uint16_t)(dst_picture_ptr->height - scs->max_input_pad_bottom);

        //uint16_t     luma_height  = input_pic->max_height;
        // Y
        for (input_row_index = 0; input_row_index < luma_height; input_row_index++) {
            svt_memcpy((dst_picture_ptr->buffer_y + luma_buffer_offset + luma_stride * input_row_index),
                       (src_picture_ptr->buffer_y + luma_buffer_offset + luma_stride * input_row_index),
                       luma_width);
        }

        // U
        for (input_row_index = 0; input_row_index < (luma_height >> 1); input_row_index++) {
            svt_memcpy((dst_picture_ptr->buffer_cb + chroma_buffer_offset + chroma_stride * input_row_index),
                       (src_picture_ptr->buffer_cb + chroma_buffer_offset + chroma_stride * input_row_index),
                       chroma_width);
        }

        // V
        for (input_row_index = 0; input_row_index < (luma_height >> 1); input_row_index++) {
            svt_memcpy((dst_picture_ptr->buffer_cr + chroma_buffer_offset + chroma_stride * input_row_index),
                       (src_picture_ptr->buffer_cr + chroma_buffer_offset + chroma_stride * input_row_index),
                       chroma_width);
        }
    } else { // 10bit packed

        svt_memcpy(dst_picture_ptr->buffer_y, src_picture_ptr->buffer_y, src_picture_ptr->luma_size);

        svt_memcpy(dst_picture_ptr->buffer_cb, src_picture_ptr->buffer_cb, src_picture_ptr->chroma_size);

        svt_memcpy(dst_picture_ptr->buffer_cr, src_picture_ptr->buffer_cr, src_picture_ptr->chroma_size);

        svt_memcpy(
            dst_picture_ptr->buffer_bit_inc_y, src_picture_ptr->buffer_bit_inc_y, src_picture_ptr->luma_size >> 2);

        svt_memcpy(
            dst_picture_ptr->buffer_bit_inc_cb, src_picture_ptr->buffer_bit_inc_cb, src_picture_ptr->chroma_size >> 2);

        svt_memcpy(
            dst_picture_ptr->buffer_bit_inc_cr, src_picture_ptr->buffer_bit_inc_cr, src_picture_ptr->chroma_size >> 2);
    }
    return return_error;
}

/* overlay specific version of copy_input_buffer without passes specializations */
static void copy_input_buffer_overlay(SequenceControlSet* sequenceControlSet, EbBufferHeaderType* dst,
                                      EbBufferHeaderType* src) {
    // Copy the higher level structure
    dst->n_alloc_len  = src->n_alloc_len;
    dst->n_filled_len = src->n_filled_len;
    dst->flags        = src->flags;
    dst->pts          = src->pts;
    dst->n_tick_count = src->n_tick_count;
    dst->size         = src->size;
    dst->qp           = src->qp;
    dst->pic_type     = src->pic_type;

    // Copy the metadata array
    if (svt_aom_copy_metadata_buffer(dst, src->metadata) != EB_ErrorNone) {
        dst->metadata = NULL;
    }

    // Copy the picture buffer
    if (src->p_buffer != NULL) {
        copy_frame_buffer_overlay(sequenceControlSet, dst->p_buffer, src->p_buffer);
    }
}

/******************************************************
 * Read Stat from File
 ******************************************************/
void svt_aom_read_stat(SequenceControlSet* scs) {
    EncodeContext* enc_ctx = scs->enc_ctx;

    enc_ctx->rc_stats_buffer = scs->static_config.rc_stats_buffer;
}

void svt_aom_setup_two_pass(SequenceControlSet* scs) {
    EncodeContext* enc_ctx     = scs->enc_ctx;
    scs->twopass.passes        = scs->passes;
    scs->twopass.stats_buf_ctx = &enc_ctx->stats_buf_context;
    scs->twopass.stats_in      = scs->twopass.stats_buf_ctx->stats_in_start;
    if (scs->static_config.pass == ENC_SECOND_PASS) {
        const size_t packet_sz = sizeof(FIRSTPASS_STATS);
        const int    packets   = (int)(enc_ctx->rc_stats_buffer.sz / packet_sz);

        if (!scs->lap_rc) {
            /*Re-initialize to stats buffer, populated by application in the case of
             * two pass*/
            scs->twopass.stats_buf_ctx->stats_in_start     = enc_ctx->rc_stats_buffer.buf;
            scs->twopass.stats_in                          = scs->twopass.stats_buf_ctx->stats_in_start;
            scs->twopass.stats_buf_ctx->stats_in_end_write = &scs->twopass.stats_buf_ctx->stats_in_start[packets - 1];
            scs->twopass.stats_buf_ctx->stats_in_end       = &scs->twopass.stats_buf_ctx->stats_in_start[packets - 1];
            svt_av1_init_second_pass(scs);
            //less than 200 frames or gop_constraint_rc, used in VBR and set in multipass encode
            scs->is_short_clip = scs->twopass.stats_buf_ctx->total_stats->count < 200 ? 1 : scs->is_short_clip;
        }
    } else if (scs->lap_rc) {
        svt_av1_init_single_pass_lap(scs);
    } else if (scs->static_config.pass == ENC_FIRST_PASS) {
        svt_aom_set_rc_param(scs);
    }
}

static EbErrorType realloc_sb_param(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    EB_FREE_ARRAY(pcs->b64_geom);
    EB_MALLOC_ARRAY(pcs->b64_geom, scs->b64_total_count);
    memcpy(pcs->b64_geom, scs->b64_geom, sizeof(B64Geom) * scs->b64_total_count);
    free_sb_geoms(pcs->sb_geom);
    // allocate buffers and copy data preserving dst pointers
    alloc_sb_geoms(&pcs->sb_geom, scs->picture_width_in_sb, scs->picture_height_in_sb);
    copy_sb_geoms(pcs->sb_geom, scs->sb_geom, scs->picture_width_in_sb, scs->picture_height_in_sb);
    pcs->is_pcs_sb_params = true;
    return EB_ErrorNone;
}

static void retrieve_resize_event(SequenceControlSet* scs, uint64_t pic_num, bool* rc_reset_flag) {
    if (scs->static_config.resize_mode != RESIZE_RANDOM_ACCESS) {
        return;
    }
    const SvtAv1FrameScaleEvts* events = &scs->static_config.frame_scale_evts;
    for (uint32_t i = 0; i < events->evt_num; i++) {
        if (!events->start_frame_nums || pic_num != events->start_frame_nums[i]) {
            continue;
        }
        EbRefFrameScale* target_evt = &scs->enc_ctx->resize_evt;
        // update scaling event for future pictures
        target_evt->scale_mode     = RESIZE_FIXED;
        target_evt->scale_denom    = events->resize_denoms ? events->resize_denoms[i] : 8;
        target_evt->scale_kf_denom = events->resize_kf_denoms ? events->resize_kf_denoms[i] : 8;
        // set reset flag of rate control
        *rc_reset_flag = true;
    }
}

/**************************************
* buffer_update_needed: check if updating the buffer needed based on the current width and height and the scs settings
**************************************/
bool buffer_update_needed(EbBufferHeaderType* input_buffer, struct SequenceControlSet* scs) {
    uint32_t max_width = !(scs->max_input_luma_width % 8) ? scs->max_input_luma_width
                                                          : scs->max_input_luma_width + (scs->max_input_luma_width % 8);

    uint32_t max_height = !(scs->max_input_luma_height % 8)
        ? scs->max_input_luma_height
        : scs->max_input_luma_height + (scs->max_input_luma_height % 8);
    if (((EbPictureBufferDesc*)(input_buffer->p_buffer))->max_width != max_width ||
        ((EbPictureBufferDesc*)(input_buffer->p_buffer))->max_height != max_height) {
        return true;
    } else {
        return false;
    }
}

/**************************************
* svt_overlay_buffer_header_update: update the parameters in overlay_buffer_header for changing the resolution on the fly
**************************************/
static EbErrorType svt_overlay_buffer_header_update(EbBufferHeaderType* input_buffer, SequenceControlSet* scs,
                                                    bool noy8b) {
    EbPictureBufferDescInitData input_pic_buf_desc_init_data;
    EbSvtAv1EncConfiguration*   config   = &scs->static_config;
    uint8_t                     is_16bit = config->encoder_bit_depth > 8 ? 1 : 0;

    input_pic_buf_desc_init_data.max_width = !(scs->max_input_luma_width % 8)
        ? scs->max_input_luma_width
        : scs->max_input_luma_width + (scs->max_input_luma_width % 8);

    input_pic_buf_desc_init_data.max_height = !(scs->max_input_luma_height % 8)
        ? scs->max_input_luma_height
        : scs->max_input_luma_height + (scs->max_input_luma_height % 8);

    input_pic_buf_desc_init_data.bit_depth    = (EbBitDepth)config->encoder_bit_depth;
    input_pic_buf_desc_init_data.color_format = (EbColorFormat)config->encoder_color_format;

    input_pic_buf_desc_init_data.left_padding  = scs->left_padding;
    input_pic_buf_desc_init_data.right_padding = scs->right_padding;
    input_pic_buf_desc_init_data.top_padding   = scs->top_padding;
    input_pic_buf_desc_init_data.bot_padding   = scs->bot_padding;

    input_pic_buf_desc_init_data.split_mode = is_16bit ? true : false;

    input_pic_buf_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    input_pic_buf_desc_init_data.is_16bit_pipeline  = 0;

    // Enhanced Picture Buffer
    if (!noy8b) {
        svt_picture_buffer_desc_update((EbPictureBufferDesc*)input_buffer->p_buffer,
                                       (EbPtr)&input_pic_buf_desc_init_data);
    } else {
        svt_picture_buffer_desc_noy8b_update((EbPictureBufferDesc*)input_buffer->p_buffer,
                                             (EbPtr)&input_pic_buf_desc_init_data);
    }

    return EB_ErrorNone;
}

/***********************************************************************
* update_new_param: Update the parameters based on the on the fly changes
************************************************************************/
static void update_new_param(SequenceControlSet* scs) {
    uint16_t subsampling_x = scs->subsampling_x;
    uint16_t subsampling_y = scs->subsampling_y;
    // Update picture width, and picture height
    if (scs->max_input_luma_width % MIN_BLOCK_SIZE) {
        scs->max_input_pad_right  = MIN_BLOCK_SIZE - (scs->max_input_luma_width % MIN_BLOCK_SIZE);
        scs->max_input_luma_width = scs->max_input_luma_width + scs->max_input_pad_right;
    } else {
        scs->max_input_pad_right = 0;
    }

    if (scs->max_input_luma_height % MIN_BLOCK_SIZE) {
        scs->max_input_pad_bottom  = MIN_BLOCK_SIZE - (scs->max_input_luma_height % MIN_BLOCK_SIZE);
        scs->max_input_luma_height = scs->max_input_luma_height + scs->max_input_pad_bottom;
    } else {
        scs->max_input_pad_bottom = 0;
    }
    scs->chroma_width                = scs->max_input_luma_width >> subsampling_x;
    scs->chroma_height               = scs->max_input_luma_height >> subsampling_y;
    scs->static_config.source_width  = scs->max_input_luma_width;
    scs->static_config.source_height = scs->max_input_luma_height;
    scs->seq_header.max_frame_width  = scs->static_config.forced_max_frame_width > 0
         ? scs->static_config.forced_max_frame_width
         : scs->static_config.sframe_dist > 0 || scs->static_config.sframe_posi.sframe_posis ? 16384
                                                                                             : scs->max_input_luma_width;
    scs->seq_header.max_frame_height = scs->static_config.forced_max_frame_height > 0
        ? scs->static_config.forced_max_frame_height
        : scs->static_config.sframe_dist > 0 || scs->static_config.sframe_posi.sframe_posis
        ? 8704
        : scs->max_input_luma_height;

    svt_aom_derive_input_resolution(&scs->input_resolution, scs->max_input_luma_width * scs->max_input_luma_height);

    svt_aom_set_mfmv_config(scs);

    // Update the number of segments based on the new resolution
    set_segments_numbers(scs);
}

// Update the input picture definitions: resolution of the sequence
static void update_input_pic_def(ResourceCoordinationContext* ctx, EbBufferHeaderType* input_ptr,
                                 SequenceControlSet* scs) {
    EbPrivDataNode* node = (EbPrivDataNode*)input_ptr->p_app_private;
    while (node) {
        if (node->node_type == RES_CHANGE_EVENT) {
            if (input_ptr->pic_type == EB_AV1_KEY_PICTURE) {
                svt_aom_assert_err(node->size == sizeof(SvtAv1InputPicDef) && node->data,
                                   "invalid private data of type RES_CHANGE_EVENT");
                SvtAv1InputPicDef* input_pic_def = (SvtAv1InputPicDef*)node->data;
                // Check if a resolution change occurred
                scs->max_input_luma_width  = input_pic_def->input_luma_width;
                scs->max_input_luma_height = input_pic_def->input_luma_height;
                scs->max_input_pad_right   = input_pic_def->input_pad_right;
                scs->max_input_pad_bottom  = input_pic_def->input_pad_bottom;
                ctx->seq_param_change      = true;
                ctx->video_res_change      = true;
                update_new_param(scs);
            }
        }
        node = node->next;
    }
}

// Update the target rate, sequence QP...
static void update_rate_info(ResourceCoordinationContext* ctx, EbBufferHeaderType* input_ptr, SequenceControlSet* scs) {
    EbPrivDataNode* node = (EbPrivDataNode*)input_ptr->p_app_private;
    while (node) {
        if (node->node_type == RATE_CHANGE_EVENT) {
            svt_aom_assert_err(node->size == sizeof(SvtAv1RateInfo) && node->data,
                               "invalid private data of type RATE_CHANGE_EVENT");
            SvtAv1RateInfo* input_pic_def = (SvtAv1RateInfo*)node->data;
            if (input_pic_def->seq_qp != 0) {
                scs->static_config.qp = input_pic_def->seq_qp;
            }
            if (input_pic_def->target_bit_rate != 0) {
                scs->static_config.target_bit_rate = input_pic_def->target_bit_rate;
            }
            ctx->seq_param_change = true;
        }
        node = node->next;
    }
}

// Update the target rate, sequence QP...
static void update_frame_rate_info(ResourceCoordinationContext* ctx, EbBufferHeaderType* input_ptr,
                                   SequenceControlSet* scs) {
    EbPrivDataNode* node = (EbPrivDataNode*)input_ptr->p_app_private;
    while (node) {
        if (node->node_type == FRAME_RATE_CHANGE_EVENT) {
            svt_aom_assert_err(node->size == sizeof(SvtAv1FrameRateInfo) && node->data,
                               "invalid private data of type FRAME_RATE_CHANGE_EVENT");
            SvtAv1FrameRateInfo* input_pic_def        = (SvtAv1FrameRateInfo*)node->data;
            scs->static_config.frame_rate_numerator   = input_pic_def->frame_rate_numerator;
            scs->static_config.frame_rate_denominator = input_pic_def->frame_rate_denominator;
            scs->frame_rate                           = (double)scs->static_config.frame_rate_numerator /
                (double)scs->static_config.frame_rate_denominator;
            ctx->seq_param_change = true;
        }
        node = node->next;
    }
}

static void update_frame_event(PictureParentControlSet* pcs, uint64_t pic_num) {
    SequenceControlSet* scs  = pcs->scs;
    EbPrivDataNode*     node = (EbPrivDataNode*)pcs->input_ptr->p_app_private;
    while (node) {
        if (node->node_type == REF_FRAME_SCALING_EVENT) {
            // update resize denominator by input event
            svt_aom_assert_err(node->size == sizeof(EbRefFrameScale),
                               "private data size mismatch of REF_FRAME_SCALING_EVENT");
            // update scaling event for future pictures
            scs->enc_ctx->resize_evt = *(EbRefFrameScale*)node->data;
            // set reset flag of rate control
            pcs->rc_reset_flag = true;
        } else if (node->node_type == ROI_MAP_EVENT) {
            svt_aom_assert_err(node->size == sizeof(SvtAv1RoiMapEvt*) && node->data,
                               "invalid private data of type ROI_MAP_EVENT");
            scs->enc_ctx->roi_map_evt = (SvtAv1RoiMapEvt*)node->data;
        } else if (node->node_type == COMPUTE_QUALITY_EVENT) {
            svt_aom_assert_err(node->size == sizeof(SvtAv1ComputeQualityInfo) && node->data,
                               "invalid private data of type COMPUTE_QUALITY_EVENT");
            SvtAv1ComputeQualityInfo* quality_info = (SvtAv1ComputeQualityInfo*)node->data;
            pcs->compute_psnr                      = pcs->compute_psnr || quality_info->compute_psnr;
            pcs->compute_ssim                      = pcs->compute_ssim || quality_info->compute_ssim;
        }
        node = node->next;
    }
    retrieve_resize_event(pcs->scs, pic_num, &pcs->rc_reset_flag);
    // update current picture scaling event
    pcs->resize_evt = scs->enc_ctx->resize_evt;
    if (scs->static_config.enable_roi_map) {
        pcs->roi_map_evt = scs->enc_ctx->roi_map_evt;
    }
}

// When the end of sequence received, there is no need to inject a new PCS.
// terminating_picture_number and terminating_sequence_flag_received are set. When all
// the pictures in the packetization queue are processed, EOS is signalled to the application.
static void set_eos_terminating_signals(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;

    svt_block_on_mutex(enc_ctx->total_number_of_shown_frames_mutex);
    enc_ctx->terminating_sequence_flag_received = true;
    enc_ctx->terminating_picture_number         = pcs->picture_number - 1;
    // if all the pictures are already processed, send the EOS signal to the app
    if (enc_ctx->total_number_of_shown_frames == enc_ctx->terminating_picture_number + 1) {
        EbObjectWrapper* tmp_out_str_wrp;
        svt_get_empty_object(scs->enc_ctx->stream_output_fifo_ptr, &tmp_out_str_wrp);
        EbBufferHeaderType* tmp_out_str = (EbBufferHeaderType*)tmp_out_str_wrp->object_ptr;

        tmp_out_str->flags        = EB_BUFFERFLAG_EOS;
        tmp_out_str->n_filled_len = 0;

        svt_post_full_object(tmp_out_str_wrp);

        // if applicable, also need to signal recon EOS
        if (scs->static_config.recon_enabled) {
            EbObjectWrapper* tmp_out_recon_wrp;
            svt_get_empty_object(scs->enc_ctx->recon_output_fifo_ptr, &tmp_out_recon_wrp);
            EbBufferHeaderType* tmp_out_recon = (EbBufferHeaderType*)tmp_out_recon_wrp->object_ptr;

            tmp_out_recon->flags        = EB_BUFFERFLAG_EOS;
            tmp_out_recon->n_filled_len = 0;

            svt_post_full_object(tmp_out_recon_wrp);
        }

        release_references_eos(scs);
    }

    svt_release_mutex(enc_ctx->total_number_of_shown_frames_mutex);
}

/* Resource Coordination Kernel */
/*********************************************************************************
 *
 * @brief
 *  The Resource Coordination Process is the first stage that input pictures
 *  this process is a single threaded, picture-based process that handles one picture at a time
 *  in display order
 *
 * @par Description:
 *  Input input picture samples are available once the input_buffer_fifo_ptr queue gets any items
 *  The Resource Coordination Process assembles the input information and creates
 *  the appropriate buffers that would travel with the input picture all along
 *  the encoding pipeline and passes this data along with the current encoder settings
 *  to the picture analysis process
 *  Encoder settings include, but are not limited to QPs, picture type, encoding
 *  parameters that change per picture sequence
 *
 * @param[in] EbBufferHeaderType
 *  EbBufferHeaderType containing the input picture samples along with settings specific to that
 *picture
 *
 * @param[out] Input picture in Picture buffers
 *  Initialized picture level (PictureParentControlSet) / sequence level
 *  (SequenceControlSet if it's the initial picture) structures
 *
 * @param[out] Settings
 *  Encoder settings include picture timing and order settings (POC) resolution settings, sequence
 *level parameters (if it is the initial picture) and other encoding parameters such as QP, Bitrate,
 *picture type ...
 *
 ********************************************************************************/
void* svt_aom_resource_coordination_kernel(void* input_ptr) {
    EbThreadContext*             enc_contxt_ptr = (EbThreadContext*)input_ptr;
    ResourceCoordinationContext* context_ptr    = (ResourceCoordinationContext*)enc_contxt_ptr->priv;

    EbObjectWrapper* pcs_wrapper;

    PictureParentControlSet* pcs;
    SequenceControlSet*      scs;
    EbObjectWrapper*         prev_scs_wrapper;

    EbObjectWrapper*             eb_input_wrapper_ptr;
    EbBufferHeaderType*          eb_input_ptr;
    EbObjectWrapper*             output_wrapper_ptr;
    ResourceCoordinationResults* out_results;
    EbObjectWrapper*             eb_input_cmd_wrapper;
    InputCommand*                input_cmd_obj;
    EbObjectWrapper*             input_pic_wrapper;
    EbObjectWrapper*             ref_pic_wrapper;

    bool             end_of_sequence_flag = false;
    EbObjectWrapper* prev_pcs_wrapper_ptr = 0;

    for (;;) {
        // Get the input command containing 2 input buffers: y8b & rest(uv8b+yuvbitInc)
        EB_GET_FULL_OBJECT(context_ptr->input_cmd_fifo_ptr, &eb_input_cmd_wrapper);

        input_cmd_obj = (InputCommand*)eb_input_cmd_wrapper->object_ptr;

        EbObjectWrapper*    y8b_wrapper = input_cmd_obj->y8b_wrapper;
        EbBufferHeaderType* y8b_header  = (EbBufferHeaderType*)y8b_wrapper->object_ptr;
        uint8_t*            buff_y8b    = ((EbPictureBufferDesc*)y8b_header->p_buffer)->buffer_y;
        eb_input_wrapper_ptr            = input_cmd_obj->eb_input_wrapper_ptr;
        eb_input_ptr                    = (EbBufferHeaderType*)eb_input_wrapper_ptr->object_ptr;

        // Set the SequenceControlSet
        scs = context_ptr->scs_instance->scs;
        // Update the input picture definitions: resolution of the sequence
        update_input_pic_def(context_ptr, eb_input_ptr, scs);
        // Update the target rate
        update_rate_info(context_ptr, eb_input_ptr, scs);
        // Update the frame rate
        update_frame_rate_info(context_ptr, eb_input_ptr, scs);
        // If config changes occurred since the last picture began encoding, then
        //   prepare a new scs containing the new changes and update the state
        //   of the previous Active scs
        svt_block_on_mutex(context_ptr->scs_instance->config_mutex);
        if (scs->enc_ctx->initial_picture || context_ptr->seq_param_change) {
            // Update picture width, picture height, cropping right offset, cropping bottom offset,
            // and conformance windows
            scs->chroma_width  = (scs->max_input_luma_width >> 1);
            scs->chroma_height = (scs->max_input_luma_height >> 1);

            scs->pad_right  = scs->max_input_pad_right;
            scs->pad_bottom = scs->max_input_pad_bottom;

            // Pre-Analysis Signal(s) derivation
            svt_aom_sig_deriv_pre_analysis_scs(scs);

            // Init SB Params
            const uint32_t input_size = scs->max_input_luma_width * scs->max_input_luma_height;
            svt_aom_derive_input_resolution(&scs->input_resolution, input_size);

            scs->pic_width_in_b64  = DIVIDE_AND_CEIL(scs->max_input_luma_width, scs->b64_size);
            scs->pic_height_in_b64 = DIVIDE_AND_CEIL(scs->max_input_luma_height, scs->b64_size);
            scs->b64_total_count   = scs->pic_width_in_b64 * scs->pic_height_in_b64;

            scs->picture_width_in_sb  = DIVIDE_AND_CEIL(scs->max_input_luma_width, scs->sb_size);
            scs->picture_height_in_sb = DIVIDE_AND_CEIL(scs->max_input_luma_height, scs->sb_size);
            scs->sb_total_count       = scs->picture_width_in_sb * scs->picture_height_in_sb;

            b64_geom_init(scs, scs->max_input_luma_width, scs->max_input_luma_height, &scs->b64_geom);
            sb_geom_init(scs, scs->max_input_luma_width, scs->max_input_luma_height, &scs->sb_geom);

            // sf_identity
            svt_av1_setup_scale_factors_for_frame(&scs->sf_identity,
                                                  scs->max_input_luma_width,
                                                  scs->max_input_luma_height,
                                                  scs->max_input_luma_width,
                                                  scs->max_input_luma_height);

            if (scs->enc_ctx->initial_picture) {
                if (scs->static_config.pass == ENC_SECOND_PASS) {
                    svt_aom_read_stat(scs);
                }
                if (scs->static_config.pass != ENC_SINGLE_PASS || scs->lap_rc) {
                    svt_aom_setup_two_pass(scs);
                } else {
                    svt_aom_set_rc_param(scs);
                }
            }

            // Copy previous Active SequenceControlSetPtr to a place holder
            prev_scs_wrapper = context_ptr->scs_active;
            // Get empty SequenceControlSet [BLOCKING]
            svt_get_empty_object(context_ptr->scs_empty_fifo_ptr, &context_ptr->scs_active);

            // Copy the contents of the active SequenceControlSet into the new empty SequenceControlSet
            // if (scs->enc_ctx->initial_picture)
            copy_sequence_control_set((SequenceControlSet*)context_ptr->scs_active->object_ptr,
                                      context_ptr->scs_instance->scs);

            // Disable releaseFlag of new SequenceControlSet
            svt_object_release_disable(context_ptr->scs_active);

            if (prev_scs_wrapper != NULL) {
                // Enable releaseFlag of old SequenceControlSet
                svt_object_release_enable(prev_scs_wrapper);

                // Check to see if previous SequenceControlSet is already inactive, if true then release the SequenceControlSet
                if (prev_scs_wrapper->live_count == 0) {
                    svt_release_object(prev_scs_wrapper);
                }
            }
        }
        svt_release_mutex(context_ptr->scs_instance->config_mutex);
        // Sequence Control Set is released by Rate Control after passing through MDC->MD->ENCDEC->Packetization->RateControl
        //   and in the PictureManager
        svt_object_inc_live_count( //EbObjectIncLiveCount(
            context_ptr->scs_active,
            1);

        // Set the current SequenceControlSet
        scs = (SequenceControlSet*)context_ptr->scs_active->object_ptr;
        // Since at this stage we do not know the prediction structure and the location of ALT_REF
        // pictures, for every picture (except first picture), we allocate two: 1. original
        // picture, 2. potential Overlay picture. In Picture Decision Process, where the overlay
        // frames are known, they extra pictures are released
        uint8_t has_overlay = (scs->static_config.enable_overlays == false ||
                               context_ptr->scs_instance->enc_ctx->initial_picture)
            ? 0
            : 1;
        for (uint8_t loop_index = 0; loop_index <= has_overlay && !end_of_sequence_flag; loop_index++) {
            // Get a New ParentPCS where we will hold the new input_picture
            svt_get_empty_object(context_ptr->picture_control_set_fifo_ptr, &pcs_wrapper);

            // Parent PCS is released by the Rate Control after passing through
            // MDC->MD->ENCDEC->Packetization
            svt_object_inc_live_count(pcs_wrapper, 1);

            pcs      = (PictureParentControlSet*)pcs_wrapper->object_ptr;
            pcs->scs = scs;
            // if resolution has changed, and the pcs settings do not match scs settings, update ppcs params
            if (pcs->frame_width != scs->max_input_luma_width || pcs->frame_height != scs->max_input_luma_height) {
                ppcs_update_param(pcs);
            }
            // - p_pcs_wrapper_ptr is a direct copy of pcs_wrapper (live_count == 1).
            // - Most of p_pcs_wrapper_ptr in pre-allocated overlay candidates will be released &
            // recycled to empty fifo
            //     by altref candidate's
            //     svt_release_object(pcs->overlay_ppcs_ptr->p_pcs_wrapper_ptr) in PictureDecision.
            // - The recycled ppcs may be assigned a new picture_number in ResourceCoordination.
            // - If the to-be-removed overlay candidate runs in svt_aom_picture_decision_kernel()
            // after above release/recycle/assign,
            //     picture_decision_reorder_queue will update by the same picture_number (of the
            //     same ppcs ptr) twice and CHECK_REPORT_ERROR_NC occur.
            // - So need ppcs live_count + 1 before post ResourceCoordinationResults, and release
            // ppcs before end of PictureDecision,
            //     to avoid recycling overlay candidate's ppcs to empty fifo too early.
            pcs->p_pcs_wrapper_ptr = pcs_wrapper;

            // reallocate sb_param_array and sb_geom for super-res or reference scaling mode on
            if (scs->static_config.superres_mode > SUPERRES_NONE || scs->static_config.resize_mode > RESIZE_NONE) {
                realloc_sb_param(scs, pcs);
            } else {
                pcs->b64_geom         = scs->b64_geom;
                pcs->sb_geom          = scs->sb_geom;
                pcs->is_pcs_sb_params = false;
            }
            pcs->input_resolution  = scs->input_resolution;
            pcs->picture_sb_width  = scs->pic_width_in_b64;
            pcs->picture_sb_height = scs->pic_height_in_b64;

            pcs->overlay_ppcs_ptr   = NULL;
            pcs->is_alt_ref         = 0;
            pcs->transition_present = -1;
            pcs->is_noise_level     = 0;
            if (loop_index) {
                pcs->is_overlay = 1;
                // set the overlay_ppcs_ptr in the original (ALT_REF) ppcs to the current ppcs
                EbObjectWrapper* alt_ref_picture_control_set_wrapper_ptr =
                    (context_ptr->scs_instance->enc_ctx->initial_picture)
                    ? pcs_wrapper
                    : scs->enc_ctx->previous_picture_control_set_wrapper_ptr;

                pcs->alt_ref_ppcs_ptr = ((PictureParentControlSet*)alt_ref_picture_control_set_wrapper_ptr->object_ptr);
                pcs->alt_ref_ppcs_ptr->overlay_ppcs_ptr = pcs;
            } else {
                pcs->is_overlay       = 0;
                pcs->alt_ref_ppcs_ptr = NULL;
            }
            // Set the Encoder mode
            pcs->enc_mode = scs->static_config.enc_mode;

            // Keep track of the previous input for the ZZ SADs computation
            pcs->previous_picture_control_set_wrapper_ptr = (context_ptr->scs_instance->enc_ctx->initial_picture)
                ? pcs_wrapper
                : scs->enc_ctx->previous_picture_control_set_wrapper_ptr;
            if (loop_index == 0) {
                scs->enc_ctx->previous_picture_control_set_wrapper_ptr = pcs_wrapper;
            }
            // Copy data from the svt buffer to the input frame
            // *Note - Assumes 4:2:0 planar
            input_pic_wrapper = eb_input_wrapper_ptr;
            pcs->enhanced_pic = (EbPictureBufferDesc*)eb_input_ptr->p_buffer;
            // make pcs input buffer access the luma8bit part from the Luma8bit Pool
            pcs->enhanced_pic->buffer_y = buff_y8b;
            pcs->input_ptr              = eb_input_ptr;
            end_of_sequence_flag        = (pcs->input_ptr->flags & EB_BUFFERFLAG_EOS) ? true : false;
            // Check whether super-res is previously enabled in this recycled parent pcs and restore
            // to non-scale-down default if so.
            if (pcs->frame_superres_enabled || pcs->frame_resize_enabled) {
                svt_aom_reset_resized_picture(scs, pcs, pcs->enhanced_pic);
            }
            pcs->superres_total_recode_loop = 0;
            pcs->superres_recode_loop       = 0;
            svt_av1_get_time(&pcs->start_time_seconds, &pcs->start_time_u_seconds);
            pcs->seq_param_changed = (context_ptr->seq_param_change) ? true : false;
            // set the scs wrapper to be released after the picture is done
            pcs->scs_wrapper = context_ptr->scs_active;
            // Reset seq_param_change and video_res_change to false
            context_ptr->seq_param_change = false;
            context_ptr->video_res_change = false;
            pcs->scs                      = scs;
            pcs->input_pic_wrapper        = input_pic_wrapper;
            //store the y8b warapper to be used for release later
            pcs->y8b_wrapper          = y8b_wrapper;
            pcs->end_of_sequence_flag = end_of_sequence_flag;
            pcs->rc_reset_flag        = false;
            pcs->compute_psnr         = scs->static_config.stat_report;
            pcs->compute_ssim         = scs->static_config.stat_report;
            update_frame_event(pcs, context_ptr->picture_number);
            pcs->is_not_scaled = (scs->static_config.superres_mode == SUPERRES_NONE) &&
                scs->static_config.resize_mode == RESIZE_NONE;
            if (loop_index == 1) {
                // Get a new input picture for overlay.
                EbObjectWrapper* input_pic_wrapper_ptr;

                // Get a new input picture for overlay.
                svt_get_empty_object(scs->enc_ctx->overlay_input_picture_pool_fifo_ptr, &input_pic_wrapper_ptr);
                // if resolution has changed, and the overlay_buffer_header settings do not match scs settings, update overlay_buffer_header settings
                if (buffer_update_needed((EbBufferHeaderType*)input_pic_wrapper_ptr->object_ptr, scs)) {
                    svt_overlay_buffer_header_update(
                        (EbBufferHeaderType*)input_pic_wrapper_ptr->object_ptr, scs, false);
                }

                // Copy from original picture (pcs->input_pic_wrapper), which is shared
                // between overlay and alt_ref up to this point, to the new input picture.
                if (pcs->alt_ref_ppcs_ptr->input_pic_wrapper->object_ptr != NULL) {
                    copy_input_buffer_overlay(
                        scs,
                        (EbBufferHeaderType*)input_pic_wrapper_ptr->object_ptr,
                        (EbBufferHeaderType*)pcs->alt_ref_ppcs_ptr->input_pic_wrapper->object_ptr);
                }
                // Assign the new picture to the new pointers
                pcs->input_ptr         = (EbBufferHeaderType*)input_pic_wrapper_ptr->object_ptr;
                pcs->enhanced_pic      = (EbPictureBufferDesc*)pcs->input_ptr->p_buffer;
                pcs->input_pic_wrapper = input_pic_wrapper_ptr;

                // overlay does NOT use y8b buffer, set to NULL to avoid
                // y8b_wrapper->live_count disorder
                pcs->y8b_wrapper = NULL;
            }
            // Set Picture Control Flags
            pcs->idr_flag          = scs->enc_ctx->initial_picture;
            pcs->cra_flag          = 0;
            pcs->scene_change_flag = false;
            pcs->qp_on_the_fly     = false;
            pcs->b64_total_count   = scs->b64_total_count;
            if (scs->speed_control_flag) {
                speed_buffer_control(context_ptr, pcs, scs);
            } else {
                pcs->enc_mode = (EncMode)scs->static_config.enc_mode;
            }
            //  If the mode of the second pass is not set from CLI, it is set to enc_mode

            // Pre-Analysis Signal(s) derivation
            svt_aom_sig_deriv_pre_analysis_pcs(pcs);
            // Rate Control

            // Picture Stats
            if (loop_index == has_overlay || end_of_sequence_flag) {
                pcs->picture_number = context_ptr->picture_number++;
            } else {
                pcs->picture_number = context_ptr->picture_number;
            }
            if (scs->passes == 2 && !end_of_sequence_flag && scs->static_config.pass == ENC_SECOND_PASS &&
                scs->static_config.rate_control_mode) {
                pcs->stat_struct = (scs->twopass.stats_buf_ctx->stats_in_start + pcs->picture_number)->stat_struct;
                if (pcs->stat_struct.poc != pcs->picture_number) {
                    SVT_LOG("Error reading data in multi pass encoding\n");
                }
            }
            if (scs->static_config.use_qp_file) {
                pcs->qp_on_the_fly = true;
                if (pcs->input_ptr->qp > MAX_QP_VALUE) {
                    SVT_WARN("INPUT QP/CRF OUTSIDE OF RANGE\n");
                    pcs->qp_on_the_fly = false;
                }
                pcs->picture_qp = (uint8_t)pcs->input_ptr->qp;
            } else {
                pcs->qp_on_the_fly = false;
                pcs->picture_qp    = (uint8_t)scs->static_config.qp;
            }
            pcs->sframe_qp_offset = 0;

            // Initialize variables for calculating the average QP
            pcs->tot_qindex               = 0;
            pcs->valid_qindex_area        = 0;
            pcs->ts_duration              = (double)10000000 / scs->frame_rate;
            scs->enc_ctx->initial_picture = false;
            pcs->sframe_ref_pruned        = false;

            // Get Empty Reference Picture Object
            svt_get_empty_object(scs->enc_ctx->pa_reference_picture_pool_fifo_ptr, &ref_pic_wrapper);

            pcs->pa_ref_pic_wrapper = ref_pic_wrapper;
            // make pa_ref full sample buffer access the luma8bit part from the y8b Pool
            EbPaReferenceObject* pa_ref_obj = (EbPaReferenceObject*)pcs->pa_ref_pic_wrapper->object_ptr;
            // if resolution has changed, and the pa_ref settings do not match scs settings, update pa reference params
            if (pa_ref_obj->input_padded_pic->max_width != scs->max_input_luma_width ||
                pa_ref_obj->input_padded_pic->max_height != scs->max_input_luma_height) {
                svt_pa_reference_param_update(pa_ref_obj, scs);
            }
            EbPictureBufferDesc* input_padded_pic = (EbPictureBufferDesc*)pa_ref_obj->input_padded_pic;
            input_padded_pic->buffer_y            = buff_y8b;
            svt_object_inc_live_count(pcs->pa_ref_pic_wrapper, 1);
            if (pcs->y8b_wrapper) {
                // y8b follows longest life cycle of pa ref and input. so it needs to build on top of live count of pa ref
                svt_object_inc_live_count(pcs->y8b_wrapper, 1);
            }
            // Get Empty Output Results Object
            // For the low delay mode, buffering for receiving EOS does not happen
            if (scs->static_config.pred_structure == LOW_DELAY) {
                PictureParentControlSet* ppcs_out = pcs;

                ppcs_out->end_of_sequence_flag = end_of_sequence_flag;
                // since overlay frame has the end of sequence set properly, set the end of sequence to true in the alt ref picture
                if (ppcs_out->is_overlay && end_of_sequence_flag) {
                    ppcs_out->alt_ref_ppcs_ptr->end_of_sequence_flag = true;
                }

                reset_pcs_av1(ppcs_out);
                if (!ppcs_out->end_of_sequence_flag) {
                    svt_get_empty_object(context_ptr->resource_coordination_results_output_fifo_ptr,
                                         &output_wrapper_ptr);
                    out_results = (ResourceCoordinationResults*)output_wrapper_ptr->object_ptr;

                    if (scs->static_config.enable_overlays == true) {
                        // ppcs live_count + 1 for PictureAnalysis & PictureDecision, will svt_release_object(ppcs) at the end of picture_decision_kernel.
                        svt_object_inc_live_count(pcs_wrapper, 1);
                    }

                    out_results->pcs_wrapper = pcs_wrapper;
                    // Post the finished Results Object
                    svt_post_full_object(output_wrapper_ptr);
                } else {
                    // When the end of sequence received, there is no need to inject a new PCS.
                    // terminating_picture_number and terminating_sequence_flag_received are set. When all
                    // the pictures in the packetization queue are processed, EOS is signalled to the application.
                    set_eos_terminating_signals(ppcs_out);
                }
            } else {
                // Get Empty Output Results Object
                if (pcs->picture_number > 0 && (prev_pcs_wrapper_ptr != NULL)) {
                    PictureParentControlSet* ppcs_out = (PictureParentControlSet*)prev_pcs_wrapper_ptr->object_ptr;

                    ppcs_out->end_of_sequence_flag = end_of_sequence_flag;
                    // since overlay frame has the end of sequence set properly, set the end of sequence to true in the alt ref picture
                    if (ppcs_out->is_overlay && end_of_sequence_flag) {
                        ppcs_out->alt_ref_ppcs_ptr->end_of_sequence_flag = true;
                    }

                    reset_pcs_av1(ppcs_out);

                    svt_get_empty_object(context_ptr->resource_coordination_results_output_fifo_ptr,
                                         &output_wrapper_ptr);
                    out_results = (ResourceCoordinationResults*)output_wrapper_ptr->object_ptr;

                    if (scs->static_config.enable_overlays == true) {
                        // ppcs live_count + 1 for PictureAnalysis & PictureDecision, will svt_release_object(ppcs) at the end of svt_aom_picture_decision_kernel.
                        svt_object_inc_live_count(prev_pcs_wrapper_ptr, 1);
                        svt_object_inc_live_count(
                            ((PictureParentControlSet*)prev_pcs_wrapper_ptr->object_ptr)->scs_wrapper, 1);
                    }

                    out_results->pcs_wrapper = prev_pcs_wrapper_ptr;
                    // Post the finished Results Object
                    svt_post_full_object(output_wrapper_ptr);
                }
                if (end_of_sequence_flag) {
                    // When the end of sequence received, there is no need to inject a new PCS.
                    // terminating_picture_number and terminating_sequence_flag_received are set. When all
                    // the pictures in the packetization queue are processed, EOS is signalled to the application.
                    set_eos_terminating_signals(pcs);
                }
            }
            prev_pcs_wrapper_ptr = pcs_wrapper;
        }
        // Release the Input Command
        svt_release_object(eb_input_cmd_wrapper);
    }

    return NULL;
}
