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

#include "encode_context.h"
#include "EbSvtAv1ErrorCodes.h"
#include "svt_threads.h"

static EbErrorType create_stats_buffer(FIRSTPASS_STATS** frame_stats_buffer, STATS_BUFFER_CTX* stats_buf_context,
                                       int num_lap_buffers) {
    EbErrorType res = EB_ErrorNone;

    int size = get_stats_buf_size(num_lap_buffers, MAX_LAG_BUFFERS);
    // *frame_stats_buffer =
    //     (FIRSTPASS_STATS *)aom_calloc(size, sizeof(FIRSTPASS_STATS));
    EB_MALLOC_ARRAY((*frame_stats_buffer), size);
    if (*frame_stats_buffer == NULL) {
        return EB_ErrorInsufficientResources;
    }

    stats_buf_context->stats_in_start     = *frame_stats_buffer;
    stats_buf_context->stats_in_end_write = stats_buf_context->stats_in_start;
    stats_buf_context->stats_in_end       = stats_buf_context->stats_in_start;
    stats_buf_context->stats_in_buf_end   = stats_buf_context->stats_in_start + size;

    EB_MALLOC_ARRAY(stats_buf_context->total_left_stats, 1);
    if (stats_buf_context->total_left_stats == NULL) {
        return EB_ErrorInsufficientResources;
    }
    svt_av1_twopass_zero_stats(stats_buf_context->total_left_stats);
    EB_MALLOC_ARRAY(stats_buf_context->total_stats, 1);
    if (stats_buf_context->total_stats == NULL) {
        return EB_ErrorInsufficientResources;
    }
    svt_av1_twopass_zero_stats(stats_buf_context->total_stats);
    stats_buf_context->last_frame_accumulated = -1;

    EB_CREATE_MUTEX(stats_buf_context->stats_in_write_mutex);
    return res;
}

static void destroy_stats_buffer(STATS_BUFFER_CTX* stats_buf_context, FIRSTPASS_STATS* frame_stats_buffer) {
    EB_FREE_ARRAY(stats_buf_context->total_left_stats);
    EB_FREE_ARRAY(stats_buf_context->total_stats);
    EB_FREE_ARRAY(frame_stats_buffer);
    EB_DESTROY_MUTEX(stats_buf_context->stats_in_write_mutex);
}

static void encode_context_dctor(EbPtr p) {
    EncodeContext* obj = (EncodeContext*)p;
    EB_DESTROY_MUTEX(obj->total_number_of_recon_frame_mutex);
    EB_DESTROY_MUTEX(obj->total_number_of_shown_frames_mutex);
    EB_DESTROY_MUTEX(obj->sc_buffer_mutex);
    EB_DESTROY_MUTEX(obj->stat_file_mutex);
    EB_DESTROY_MUTEX(obj->frame_updated_mutex);
    EB_DELETE(obj->prediction_structure_group_ptr);
    EB_DELETE_PTR_ARRAY(obj->picture_decision_reorder_queue, obj->picture_decision_reorder_queue_size);
    obj->picture_decision_reorder_queue_size = 0;
    EB_FREE(obj->pre_assignment_buffer);
    EB_DELETE_PTR_ARRAY(obj->pic_mgr_input_pic_list, obj->pic_mgr_input_pic_list_size);
    obj->pic_mgr_input_pic_list_size = 0;
    EB_DELETE_PTR_ARRAY(obj->ref_pic_list, obj->ref_pic_list_length);
    EB_DESTROY_MUTEX(obj->ref_pic_list_mutex);
    EB_DELETE_PTR_ARRAY(obj->pd_dpb, REF_FRAMES);
    EB_DESTROY_MUTEX(obj->pd_dpb_mutex);
    EB_DELETE_PTR_ARRAY(obj->packetization_reorder_queue, obj->packetization_reorder_queue_size);
    obj->packetization_reorder_queue_size = 0;
    EB_FREE(obj->stats_out.stat);
    destroy_stats_buffer(&obj->stats_buf_context, obj->frame_stats_buffer);
    EB_DELETE_PTR_ARRAY(obj->rc.coded_frames_stat_queue, CODED_FRAMES_STAT_QUEUE_MAX_DEPTH);

    if (obj->rc_param_queue) {
        EB_FREE_2D(obj->rc_param_queue);
    }
    EB_DESTROY_MUTEX(obj->rc_param_queue_mutex);
    EB_DESTROY_MUTEX(obj->rc.rc_mutex);
}

EbErrorType svt_aom_encode_context_ctor(EncodeContext* enc_ctx, EbPtr object_init_data_ptr) {
    uint32_t picture_index;

    enc_ctx->dctor = encode_context_dctor;

    (void)object_init_data_ptr;
    CHECK_REPORT_ERROR(1, enc_ctx->app_callback_ptr, EB_ENC_EC_ERROR29);

    EB_CREATE_MUTEX(enc_ctx->total_number_of_recon_frame_mutex);
    EB_CREATE_MUTEX(enc_ctx->frame_updated_mutex);
    enc_ctx->picture_decision_reorder_queue_size = 0;
    EB_ALLOC_PTR_ARRAY(enc_ctx->pre_assignment_buffer, PRE_ASSIGNMENT_MAX_DEPTH);

    // input pic list allocated in svt_av1_enc_init based on number of ppcs
    enc_ctx->pic_mgr_input_pic_list_size = 0;

    EB_ALLOC_PTR_ARRAY(enc_ctx->pd_dpb, REF_FRAMES);
    for (picture_index = 0; picture_index < REF_FRAMES; ++picture_index) {
        EB_NEW(enc_ctx->pd_dpb[picture_index], svt_aom_pa_reference_queue_entry_ctor);
    }
    EB_CREATE_MUTEX(enc_ctx->pd_dpb_mutex);
    EB_CREATE_MUTEX(enc_ctx->total_number_of_shown_frames_mutex);
    EB_CREATE_MUTEX(enc_ctx->ref_pic_list_mutex);

    enc_ctx->initial_picture = true;

    // Sequence Termination Flags
    enc_ctx->terminating_picture_number = ~0u;
    EB_CREATE_MUTEX(enc_ctx->sc_buffer_mutex);
    enc_ctx->enc_mode         = SPEED_CONTROL_INIT_MOD;
    enc_ctx->recode_tolerance = 25;
    enc_ctx->rc_cfg.min_cr    = 0;
    EB_CREATE_MUTEX(enc_ctx->stat_file_mutex);
    enc_ctx->num_lap_buffers = 0; // lap not supported for now
    int* num_lap_buffers     = &enc_ctx->num_lap_buffers;
    create_stats_buffer(&enc_ctx->frame_stats_buffer, &enc_ctx->stats_buf_context, *num_lap_buffers);
    EB_ALLOC_PTR_ARRAY(enc_ctx->rc.coded_frames_stat_queue, CODED_FRAMES_STAT_QUEUE_MAX_DEPTH);

    EB_CREATE_MUTEX(enc_ctx->rc.rc_mutex);
    for (picture_index = 0; picture_index < CODED_FRAMES_STAT_QUEUE_MAX_DEPTH; ++picture_index) {
        EB_NEW(enc_ctx->rc.coded_frames_stat_queue[picture_index],
               svt_aom_rate_control_coded_frames_stats_context_ctor,
               picture_index);
    }
#if DEBUG_RC_CAP_LOG
    enc_ctx->rc.min_bit_actual_per_gop = 0xfffffffffffff;
#endif
    EB_MALLOC_2D(enc_ctx->rc_param_queue, (int32_t)PARALLEL_GOP_MAX_NUMBER, 1);

    for (int interval_index = 0; interval_index < PARALLEL_GOP_MAX_NUMBER; interval_index++) {
        enc_ctx->rc_param_queue[interval_index]->first_poc                = 0;
        enc_ctx->rc_param_queue[interval_index]->processed_frame_number   = 0;
        enc_ctx->rc_param_queue[interval_index]->size                     = -1;
        enc_ctx->rc_param_queue[interval_index]->end_of_seq_seen          = 0;
        enc_ctx->rc_param_queue[interval_index]->vbr_bits_off_target      = 0;
        enc_ctx->rc_param_queue[interval_index]->vbr_bits_off_target_fast = 0;
        enc_ctx->rc_param_queue[interval_index]->rolling_target_bits      = enc_ctx->rc.avg_frame_bandwidth;
        enc_ctx->rc_param_queue[interval_index]->rolling_actual_bits      = enc_ctx->rc.avg_frame_bandwidth;
        enc_ctx->rc_param_queue[interval_index]->rate_error_estimate      = 0;
        enc_ctx->rc_param_queue[interval_index]->total_actual_bits        = 0;
        enc_ctx->rc_param_queue[interval_index]->total_target_bits        = 0;
        enc_ctx->rc_param_queue[interval_index]->extend_minq              = 0;
        enc_ctx->rc_param_queue[interval_index]->extend_maxq              = 0;
        enc_ctx->rc_param_queue[interval_index]->extend_minq_fast         = 0;
    }
    enc_ctx->rc_param_queue_head_index = 0;
    enc_ctx->cr_sb_end                 = 0;

    EB_CREATE_MUTEX(enc_ctx->rc_param_queue_mutex);

    enc_ctx->roi_map_evt = NULL;
    return EB_ErrorNone;
}
