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

#include "sequence_control_set.h"
#include "utility.h"

static void free_scale_evts(SvtAv1FrameScaleEvts* evts) {
    EB_FREE_ARRAY(evts->resize_denoms);
    EB_FREE_ARRAY(evts->resize_kf_denoms);
    EB_FREE_ARRAY(evts->start_frame_nums);
    evts->evt_num = 0;
}

static void svt_sequence_control_set_dctor(EbPtr p) {
    SequenceControlSet* obj = (SequenceControlSet*)p;
    if (!obj) {
        return;
    }
    EB_FREE_ARRAY(obj->b64_geom);
    free_sb_geoms(obj->sb_geom);
    free_scale_evts(&obj->static_config.frame_scale_evts);
    EB_FREE_ARRAY(obj->static_config.sframe_posi.sframe_qps);
    EB_FREE_ARRAY(obj->static_config.sframe_posi.sframe_qp_offsets);
    obj->static_config.sframe_posi.sframe_qp_num = 0;
    EB_FREE_ARRAY(obj->static_config.sframe_posi.sframe_posis);
    obj->static_config.sframe_posi.sframe_num = 0;
}

/**************************************************************************************************
    General notes on how Sequence Control Sets (SCS) are used.

    SequenceControlSetInstance
        is the primary copy that interacts with the API in real-time.  When a
        change happens, the changeFlag is signaled so that appropriate action can
        be taken.  There is one scsInstance per stream/encode instance.  The scsInstance
        owns the encodeContext

    encodeContext
        has context type variables (i.e. non-config) that keep track of global parameters.

    SequenceControlSets
        general SCSs are controled by a system resource manager.  They are kept completely
        separate from the instances.  In general there is one active SCS at a time.  When the
        changeFlag is signaled, the old active SCS is no longer used for new input pictures.
        A fresh copy of the scsInstance is made to a new SCS, which becomes the active SCS.  The
        old SCS will eventually be released back into the SCS pool when its current pictures are
        finished encoding.

    Motivations
        The whole reason for this structure is due to the nature of the pipeline.  We have to
        take great care not to have pipeline mismanagement.  Once an object enters use in the
        pipeline, it cannot be changed on the fly or you will have pipeline coherency problems.

    **** Currently, real-time updates to the SCS are not supported.  Therefore, each instance
    has a single SCS (from the SequenceControlSetInstance) that is used for encoding the entire
    stream.  At the resource coordination kernel, a pointer to the SequenceControlSetInstance
    is saved in the PCS, that is not managed by an SRM.
 ***************************************************************************************************/
EbErrorType svt_sequence_control_set_ctor(SequenceControlSet* scs, EbPtr object_init_data_ptr) {
    UNUSED(object_init_data_ptr);
    scs->dctor = svt_sequence_control_set_dctor;

    // Allocation will happen in resource-coordination
    scs->b64_geom = NULL;

    scs->film_grain_random_seed = 7391;

    // Initialize certain sequence header variables here for write_sequence_header(),
    // which may be called before the first picture hits resource coordination thread
    // (e.g. when ffmpeg is used it may be called first to construct mkv/mp4 container headers).
    // Whenever possible, it is recommended to initialize all sequence header info here
    // instead of in resource coordination.
    scs->seq_header.frame_width_bits              = 16;
    scs->seq_header.frame_height_bits             = 16;
    scs->seq_header.frame_id_numbers_present_flag = 0;
    scs->seq_header.frame_id_length               = FRAME_ID_LENGTH;
    scs->seq_header.delta_frame_id_length         = DELTA_FRAME_ID_LENGTH;

    // 0 - disable dual interpolation filter
    // 1 - enable vertical and horiz filter selection
    scs->seq_header.enable_dual_filter = 0;

    // 0 - force off
    // 1 - force on
    // 2 - adaptive
    scs->seq_header.seq_force_screen_content_tools = 2;

    // 0 - Not to force. MV can be in 1/4 or 1/8
    // 1 - force to integer
    // 2 - adaptive
    scs->seq_header.seq_force_integer_mv = 2;

    scs->seq_header.order_hint_info.enable_ref_frame_mvs = 1;
    scs->seq_header.order_hint_info.enable_order_hint    = 1;
    scs->seq_header.order_hint_info.order_hint_bits      = 7;

    return EB_ErrorNone;
}

EbErrorType svt_aom_scs_set_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    SequenceControlSet* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, svt_sequence_control_set_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}

EbErrorType svt_aom_derive_input_resolution(EbInputResolution* input_resolution, uint32_t inputSize) {
    EbErrorType return_error = EB_ErrorNone;
    if (inputSize < INPUT_SIZE_240p_TH) {
        *input_resolution = INPUT_SIZE_240p_RANGE;
    } else if (inputSize < INPUT_SIZE_360p_TH) {
        *input_resolution = INPUT_SIZE_360p_RANGE;
    } else if (inputSize < INPUT_SIZE_480p_TH) {
        *input_resolution = INPUT_SIZE_480p_RANGE;
    } else if (inputSize < INPUT_SIZE_720p_TH) {
        *input_resolution = INPUT_SIZE_720p_RANGE;
    } else if (inputSize < INPUT_SIZE_1080p_TH) {
        *input_resolution = INPUT_SIZE_1080p_RANGE;
    } else if (inputSize < INPUT_SIZE_4K_TH) {
        *input_resolution = INPUT_SIZE_4K_RANGE;
    } else {
        *input_resolution = INPUT_SIZE_8K_RANGE;
    }

    return return_error;
}

static void svt_sequence_control_set_instance_dctor(EbPtr p) {
    EbSequenceControlSetInstance* obj = (EbSequenceControlSetInstance*)p;
    EB_DELETE(obj->enc_ctx);
    EB_DESTROY_SEMAPHORE(obj->scs->ref_buffer_available_semaphore);
    EB_DESTROY_MUTEX(obj->config_mutex);
    EB_DELETE(obj->scs);
}

EbErrorType svt_sequence_control_set_instance_ctor(EbSequenceControlSetInstance* object_ptr) {
    object_ptr->dctor = svt_sequence_control_set_instance_dctor;

    EB_NEW(object_ptr->enc_ctx, svt_aom_encode_context_ctor, NULL);
    EB_NEW(object_ptr->scs, svt_sequence_control_set_ctor, NULL);
    object_ptr->scs->enc_ctx = object_ptr->enc_ctx;

    EB_CREATE_MUTEX(object_ptr->config_mutex);
    return EB_ErrorNone;
}

/************************************************
 * Sequence Control Set Copy
 ************************************************/
EbErrorType copy_sequence_control_set(SequenceControlSet* dst, SequenceControlSet* src) {
    if (dst->sb_geom != NULL) {
        free_sb_geoms(dst->sb_geom);
    }
    if (dst->b64_geom != NULL) {
        EB_FREE_ARRAY(dst->b64_geom);
    }
    // Copy the non-pointer members
    *dst = *src;

    EB_MALLOC_ARRAY(dst->b64_geom, dst->b64_total_count);
    memcpy(dst->b64_geom, src->b64_geom, sizeof(B64Geom) * dst->b64_total_count);

    // allocate buffers and copy data preserving dst pointers
    alloc_sb_geoms(&dst->sb_geom, dst->picture_width_in_sb, dst->picture_height_in_sb);
    copy_sb_geoms(dst->sb_geom, src->sb_geom, dst->picture_width_in_sb, dst->picture_height_in_sb);

    if (src->static_config.frame_scale_evts.start_frame_nums) {
        EB_NO_THROW_MALLOC(dst->static_config.frame_scale_evts.start_frame_nums,
                           sizeof(int64_t) * src->static_config.frame_scale_evts.evt_num);
        memcpy(dst->static_config.frame_scale_evts.start_frame_nums,
               src->static_config.frame_scale_evts.start_frame_nums,
               sizeof(int64_t) * src->static_config.frame_scale_evts.evt_num);
    }
    if (src->static_config.frame_scale_evts.resize_kf_denoms) {
        EB_NO_THROW_MALLOC(dst->static_config.frame_scale_evts.resize_kf_denoms,
                           sizeof(int32_t) * src->static_config.frame_scale_evts.evt_num);
        memcpy(dst->static_config.frame_scale_evts.resize_kf_denoms,
               src->static_config.frame_scale_evts.resize_kf_denoms,
               sizeof(int32_t) * src->static_config.frame_scale_evts.evt_num);
    }
    if (src->static_config.frame_scale_evts.resize_denoms) {
        EB_NO_THROW_MALLOC(dst->static_config.frame_scale_evts.resize_denoms,
                           sizeof(int32_t) * src->static_config.frame_scale_evts.evt_num);
        memcpy(dst->static_config.frame_scale_evts.resize_denoms,
               src->static_config.frame_scale_evts.resize_denoms,
               sizeof(int32_t) * src->static_config.frame_scale_evts.evt_num);
    }
    if (src->static_config.sframe_posi.sframe_posis) {
        EB_MALLOC(dst->static_config.sframe_posi.sframe_posis,
                  sizeof(uint64_t) * src->static_config.sframe_posi.sframe_num);
        memcpy(dst->static_config.sframe_posi.sframe_posis,
               src->static_config.sframe_posi.sframe_posis,
               sizeof(uint64_t) * src->static_config.sframe_posi.sframe_num);
    }
    if (src->static_config.sframe_posi.sframe_qps) {
        EB_MALLOC(dst->static_config.sframe_posi.sframe_qps,
                  sizeof(src->static_config.sframe_posi.sframe_qps[0]) * src->static_config.sframe_posi.sframe_qp_num);
        memcpy(dst->static_config.sframe_posi.sframe_qps,
               src->static_config.sframe_posi.sframe_qps,
               sizeof(src->static_config.sframe_posi.sframe_qps[0]) * src->static_config.sframe_posi.sframe_qp_num);
    }
    if (src->static_config.sframe_posi.sframe_qp_offsets) {
        EB_MALLOC(
            dst->static_config.sframe_posi.sframe_qp_offsets,
            sizeof(src->static_config.sframe_posi.sframe_qp_offsets[0]) * src->static_config.sframe_posi.sframe_qp_num);
        memcpy(
            dst->static_config.sframe_posi.sframe_qp_offsets,
            src->static_config.sframe_posi.sframe_qp_offsets,
            sizeof(src->static_config.sframe_posi.sframe_qp_offsets[0]) * src->static_config.sframe_posi.sframe_qp_num);
    }

    // Continue this process for all other pointers within the struct...

    return EB_ErrorNone;
}
