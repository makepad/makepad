/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at www.aomedia.org/license/patent.
*/

#include <stdlib.h>

#include "enc_handle.h"
#include "rest_process.h"
#include "enc_dec_results.h"
#include "svt_threads.h"
#include "pic_demux_results.h"
#include "reference_object.h"
#include "pcs.h"
#include "resource_coordination_process.h"
#include "resize.h"
#include "enc_mode_config.h"

/**************************************
 * Rest Context
 **************************************/
typedef struct RestContext {
    EbDctor dctor;
    EbFifo* rest_input_fifo_ptr;
    EbFifo* rest_output_fifo_ptr;
    EbFifo* picture_demux_fifo_ptr;

    EbPictureBufferDesc* trial_frame_rst;

    EbPictureBufferDesc* org_rec_frame;
    // while doing the filtering recon gets updated using setup/restore processing_stripe_bounadaries
    // many threads doing the above will result in race condition.
    // each thread will hence have his own copy of recon to work on.
    // later we can have a search version that does not need the exact right recon
    int32_t* rst_tmpbuf;
} RestContext;

void        svt_aom_recon_output(PictureControlSet* pcs, SequenceControlSet* scs);
void        svt_av1_loop_restoration_filter_frame(int32_t* rst_tmpbuf, Yv12BufferConfig* frame, Av1Common* cm,
                                                  int32_t optimized_lr);
EbErrorType psnr_calculations(PictureControlSet* pcs, SequenceControlSet* scs, bool free_memory);
EbErrorType svt_aom_ssim_calculations(PictureControlSet* pcs, SequenceControlSet* scs, bool free_memory);
void        pad_ref_and_set_flags(PictureControlSet* pcs, SequenceControlSet* scs);
void        restoration_seg_search(int32_t* rst_tmpbuf, Yv12BufferConfig* org_fts, const Yv12BufferConfig* src,
                                   Yv12BufferConfig* trial_frame_rst, PictureControlSet* pcs, uint32_t segment_index);
void        rest_finish_search(PictureControlSet* pcs);

static void rest_context_dctor(EbPtr p) {
    EbThreadContext* thread_ctx = (EbThreadContext*)p;
    RestContext*     obj        = (RestContext*)thread_ctx->priv;
    EB_DELETE(obj->trial_frame_rst);
    // buffer only malloc'd if boundaries are used in rest. search.
    // see scs->seq_header.use_boundaries_in_rest_search
    if (obj->org_rec_frame) {
        EB_DELETE(obj->org_rec_frame);
    }
    EB_FREE_ALIGNED(obj->rst_tmpbuf);
    EB_FREE_ARRAY(obj);
}

/******************************************************
 * Rest Context Constructor
 ******************************************************/
EbErrorType svt_aom_rest_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index,
                                      int demux_index) {
    const SequenceControlSet*       scs    = enc_handle_ptr->scs_instance->scs;
    const EbSvtAv1EncConfiguration* config = &scs->static_config;
    RestContext*                    context_ptr;
    const bool                      allintra = scs->allintra;
#if TUNE_STILL_IMAGE
    const bool rtc_tune = scs->static_config.rtc;
#endif
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = rest_context_dctor;

    // Input/Output System Resource Manager FIFOs
    context_ptr->rest_input_fifo_ptr  = svt_system_resource_get_consumer_fifo(enc_handle_ptr->cdef_results_resource_ptr,
                                                                             index);
    context_ptr->rest_output_fifo_ptr = svt_system_resource_get_producer_fifo(enc_handle_ptr->rest_results_resource_ptr,
                                                                              index);
    context_ptr->picture_demux_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_demux_results_resource_ptr, demux_index);

    bool is_16bit = scs->is_16bit_pipeline;
#if TUNE_STILL_IMAGE
    uint8_t enable_restoration = allintra
        ? svt_aom_get_enable_restoration_allintra(config->enc_mode, config->enable_restoration_filtering)
        : rtc_tune
        ? svt_aom_get_enable_restoration_rtc(
              config->enc_mode, config->enable_restoration_filtering, scs->input_resolution, config->fast_decode)
        : svt_aom_get_enable_restoration_default(
              config->enc_mode, config->enable_restoration_filtering, scs->input_resolution, config->fast_decode);

    uint8_t enable_sg = allintra ? svt_aom_get_enable_sg_allintra()
        : rtc_tune ? svt_aom_get_enable_sg_rtc(config->enc_mode, scs->input_resolution, config->fast_decode)
                   : svt_aom_get_enable_sg_default(config->enc_mode, scs->input_resolution, config->fast_decode);

    if (enable_restoration) {
        EbPictureBufferDescInitData init_data;

        init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
        init_data.max_width          = (uint16_t)scs->max_input_luma_width;
        init_data.max_height         = (uint16_t)scs->max_input_luma_height;
        init_data.bit_depth          = is_16bit ? EB_SIXTEEN_BIT : EB_EIGHT_BIT;
        init_data.color_format       = config->encoder_color_format;
        init_data.left_padding       = AOM_RESTORATION_FRAME_BORDER;
        init_data.right_padding      = AOM_RESTORATION_FRAME_BORDER;
        init_data.top_padding        = AOM_RESTORATION_FRAME_BORDER;
        init_data.bot_padding        = AOM_RESTORATION_FRAME_BORDER;
        init_data.split_mode         = false;
        init_data.is_16bit_pipeline  = is_16bit;

        EB_NEW(context_ptr->trial_frame_rst, svt_picture_buffer_desc_ctor, (EbPtr)&init_data);
        if (scs->use_boundaries_in_rest_search) {
            EB_NEW(context_ptr->org_rec_frame, svt_picture_buffer_desc_ctor, (EbPtr)&init_data);
        } else {
            context_ptr->org_rec_frame = NULL;
        }
        if (!is_16bit) {
            context_ptr->trial_frame_rst->bit_depth = EB_EIGHT_BIT;
            if (scs->use_boundaries_in_rest_search) {
                context_ptr->org_rec_frame->bit_depth = EB_EIGHT_BIT;
            }
        }
        context_ptr->rst_tmpbuf = NULL;

        if (enable_sg) {
            EB_MALLOC_ALIGNED(context_ptr->rst_tmpbuf, RESTORATION_TMPBUF_SIZE);
        }
    }
#else
    if (svt_aom_get_enable_restoration(config->enc_mode,
                                       config->enable_restoration_filtering,
                                       scs->input_resolution,
                                       config->fast_decode,
                                       allintra,
                                       config->rtc)) {
        EbPictureBufferDescInitData init_data;

        init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
        init_data.max_width          = (uint16_t)scs->max_input_luma_width;
        init_data.max_height         = (uint16_t)scs->max_input_luma_height;
        init_data.bit_depth          = is_16bit ? EB_SIXTEEN_BIT : EB_EIGHT_BIT;
        init_data.color_format       = config->encoder_color_format;
        init_data.left_padding       = AOM_RESTORATION_FRAME_BORDER;
        init_data.right_padding      = AOM_RESTORATION_FRAME_BORDER;
        init_data.top_padding        = AOM_RESTORATION_FRAME_BORDER;
        init_data.bot_padding        = AOM_RESTORATION_FRAME_BORDER;
        init_data.split_mode         = false;
        init_data.is_16bit_pipeline  = is_16bit;

        EB_NEW(context_ptr->trial_frame_rst, svt_picture_buffer_desc_ctor, (EbPtr)&init_data);
        if (scs->use_boundaries_in_rest_search) {
            EB_NEW(context_ptr->org_rec_frame, svt_picture_buffer_desc_ctor, (EbPtr)&init_data);
        } else {
            context_ptr->org_rec_frame = NULL;
        }
        if (!is_16bit) {
            context_ptr->trial_frame_rst->bit_depth = EB_EIGHT_BIT;
            if (scs->use_boundaries_in_rest_search) {
                context_ptr->org_rec_frame->bit_depth = EB_EIGHT_BIT;
            }
        }
        context_ptr->rst_tmpbuf = NULL;
        if (svt_aom_get_enable_sg(config->enc_mode, scs->input_resolution, config->fast_decode, allintra)) {
            EB_MALLOC_ALIGNED(context_ptr->rst_tmpbuf, RESTORATION_TMPBUF_SIZE);
        }
    }
#endif

    return EB_ErrorNone;
}

// If using boundaries during the filter search, copy the recon pic to a new buffer (to
// avoid race conidition from many threads modifying the same recon pic).
//
// If not using boundaries during the filter search, return the input recon picture location
// to be used in restoration search (save cycles/memory of copying pic to a new buffer).
// The recon pic should not be modified during the search, otherwise there will be a race
// condition between threads.
//
// Return a pointer to the recon pic to be used during the restoration search.
static EbPictureBufferDesc* get_own_recon(SequenceControlSet* scs, PictureControlSet* pcs, RestContext* context_ptr,
                                          bool is_16bit) {
    EbPictureBufferDesc* recon_pic;
    svt_aom_get_recon_pic(pcs, &recon_pic, is_16bit);
    // if boundaries are not used, don't need to copy pic to new buffer, as the
    // search will not modify the pic
    if (!scs->use_boundaries_in_rest_search) {
        return recon_pic;
    }

    const uint32_t ss_x = scs->subsampling_x;
    const uint32_t ss_y = scs->subsampling_y;

    EbPictureBufferDesc* org_rec = context_ptr->org_rec_frame;

    int org_org_x     = org_rec->org_x << is_16bit;
    int org_org_x_c   = org_org_x >> ss_x;
    int org_stride_y  = org_rec->stride_y << is_16bit;
    int org_stride_cb = org_rec->stride_cb << is_16bit;
    int org_stride_cr = org_rec->stride_cr << is_16bit;

    int rec_org_x     = recon_pic->org_x << is_16bit;
    int rec_org_x_c   = rec_org_x >> ss_x;
    int rec_stride_y  = recon_pic->stride_y << is_16bit;
    int rec_stride_cb = recon_pic->stride_cb << is_16bit;
    int rec_stride_cr = recon_pic->stride_cr << is_16bit;

    uint8_t* org_ptr    = org_rec->buffer_y + org_org_x + org_rec->org_y * org_stride_y;
    uint8_t* org_ptr_cb = org_rec->buffer_cb + org_org_x_c + (org_rec->org_y >> ss_y) * org_stride_cb;
    uint8_t* org_ptr_cr = org_rec->buffer_cr + org_org_x_c + (org_rec->org_y >> ss_y) * org_stride_cr;

    uint8_t* rec_ptr    = recon_pic->buffer_y + rec_org_x + recon_pic->org_y * rec_stride_y;
    uint8_t* rec_ptr_cb = recon_pic->buffer_cb + rec_org_x_c + (recon_pic->org_y >> ss_y) * rec_stride_cb;
    uint8_t* rec_ptr_cr = recon_pic->buffer_cr + rec_org_x_c + (recon_pic->org_y >> ss_y) * rec_stride_cr;

    int rec_width = recon_pic->width << is_16bit;

    for (int r = 0; r < recon_pic->height; ++r) {
        svt_memcpy(org_ptr + r * org_stride_y, rec_ptr + r * rec_stride_y, rec_width);
    }

    for (int r = 0; r < (recon_pic->height >> ss_y); ++r) {
        svt_memcpy(org_ptr_cb + r * org_stride_cb, rec_ptr_cb + r * rec_stride_cb, rec_width >> ss_x);
        svt_memcpy(org_ptr_cr + r * org_stride_cr, rec_ptr_cr + r * rec_stride_cr, rec_width >> ss_x);
    }
    return org_rec;
}

static void copy_statistics_to_ref_obj_ect(PictureControlSet* pcs, SequenceControlSet* scs) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    FrameHeader*             frm_hdr = &ppcs->frm_hdr;
    EbReferenceObject*       obj     = (EbReferenceObject*)ppcs->ref_pic_wrapper->object_ptr;

    pcs->intra_coded_area = (100 * pcs->intra_coded_area) / (ppcs->aligned_width * ppcs->aligned_height);
    pcs->skip_coded_area  = (100 * pcs->skip_coded_area) / (ppcs->aligned_width * ppcs->aligned_height);
    pcs->hp_coded_area    = (100 * pcs->hp_coded_area) / (ppcs->aligned_width * ppcs->aligned_height);
    pcs->avg_cnt_zeromv   = (100 * pcs->avg_cnt_zeromv) / (ppcs->aligned_width * ppcs->aligned_height);
    if (pcs->slice_type == I_SLICE) {
        pcs->intra_coded_area = 0;
    }
    obj->intra_coded_area = (uint8_t)pcs->intra_coded_area;
    obj->skip_coded_area  = (uint8_t)pcs->skip_coded_area;
    obj->hp_coded_area    = (uint8_t)pcs->hp_coded_area;
    obj->is_mfmv_used     = frm_hdr->use_ref_frame_mvs;

    obj->filter_level[0] = frm_hdr->loop_filter_params.filter_level[0];
    obj->filter_level[1] = frm_hdr->loop_filter_params.filter_level[1];
    obj->filter_level_u  = frm_hdr->loop_filter_params.filter_level_u;
    obj->filter_level_v  = frm_hdr->loop_filter_params.filter_level_v;
    obj->dlf_dist_dev    = pcs->dlf_dist_dev;
    obj->cdef_dist_dev   = pcs->cdef_dist_dev;

    obj->ref_cdef_strengths_num = ppcs->nb_cdef_strengths;
    for (int i = 0; i < ppcs->nb_cdef_strengths; i++) {
        obj->ref_cdef_strengths[0][i] = frm_hdr->cdef_params.cdef_y_strength[i];
        obj->ref_cdef_strengths[1][i] = frm_hdr->cdef_params.cdef_uv_strength[i];
    }
    uint32_t sb_index;
    for (sb_index = 0; sb_index < pcs->b64_total_count; ++sb_index) {
        obj->sb_intra[sb_index]           = pcs->sb_intra[sb_index];
        obj->sb_skip[sb_index]            = pcs->sb_skip[sb_index];
        obj->sb_64x64_mvp[sb_index]       = pcs->sb_64x64_mvp[sb_index];
        obj->sb_me_64x64_dist[sb_index]   = ppcs->me_64x64_distortion[sb_index];
        obj->sb_me_8x8_cost_var[sb_index] = ppcs->me_8x8_cost_variance[sb_index];
        obj->sb_min_sq_size[sb_index]     = pcs->sb_min_sq_size[sb_index];
        obj->sb_max_sq_size[sb_index]     = pcs->sb_max_sq_size[sb_index];
    }
    obj->tmp_layer_idx   = pcs->temporal_layer_index;
    obj->is_scene_change = ppcs->scene_change_flag;

    Av1Common* cm = ppcs->av1_cm;
    if (scs->mfmv_enabled || !ppcs->is_not_scaled) {
        obj->frame_type = frm_hdr->frame_type;
        obj->order_hint = ppcs->cur_order_hint;
        svt_memcpy(obj->ref_order_hint, ppcs->ref_order_hint, sizeof(obj->ref_order_hint));
    }
    // Copy the prev frame wn filter coeffs
    if (cm->wn_filter_ctrls.enabled && cm->wn_filter_ctrls.use_prev_frame_coeffs) {
        for (int32_t plane = 0; plane < MAX_MB_PLANE; ++plane) {
            int32_t ntiles = pcs->rst_info[plane].units_per_tile;
            for (int32_t u = 0; u < ntiles; ++u) {
                obj->unit_info[plane][u].restoration_type = pcs->rst_info[plane].unit_info[u].restoration_type;
                if (pcs->rst_info[plane].unit_info[u].restoration_type == RESTORE_WIENER) {
                    obj->unit_info[plane][u].wiener_info = pcs->rst_info[plane].unit_info[u].wiener_info;
                }
            }
        }
    }
}

/******************************************************
 * Rest Kernel
 ******************************************************/
void* svt_aom_rest_kernel(void* input_ptr) {
    EbThreadContext* thread_ctx  = (EbThreadContext*)input_ptr;
    RestContext*     context_ptr = (RestContext*)thread_ctx->priv;

    for (;;) {
        // Get Cdef Results
        EbObjectWrapper* cdef_results_wrapper;
        EB_GET_FULL_OBJECT(context_ptr->rest_input_fifo_ptr, &cdef_results_wrapper);

        CdefResults*             cdef_results = (CdefResults*)cdef_results_wrapper->object_ptr;
        PictureControlSet*       pcs          = (PictureControlSet*)cdef_results->pcs_wrapper->object_ptr;
        PictureParentControlSet* ppcs         = pcs->ppcs;
        SequenceControlSet*      scs          = pcs->scs;
        FrameHeader*             frm_hdr      = &ppcs->frm_hdr;
        bool                     is_16bit     = scs->is_16bit_pipeline;
        Av1Common*               cm           = ppcs->av1_cm;
        if (ppcs->enable_restoration && frm_hdr->allow_intrabc == 0) {
            // If using boundaries during the filter search, copy the recon pic to a new buffer (to
            // avoid race condition from many threads modifying the same recon pic).
            //
            // If not using boundaries during the filter search, copy the input recon picture
            // location to be used in restoration search (save cycles/memory of copying pic to a new
            // buffer). The recon pic should not be modified during the search, otherwise there will
            // be a race condition between threads.
            EbPictureBufferDesc* recon_pic = get_own_recon(scs, pcs, context_ptr, is_16bit);
            EbPictureBufferDesc* input_pic = is_16bit ? pcs->input_frame16bit : ppcs->enhanced_unscaled_pic;

            // downscale input picture if recon is resized
            bool is_resized = recon_pic->width != input_pic->width || recon_pic->height != input_pic->height;
            if (is_resized) {
                input_pic = pcs->scaled_input_pic;
            }

            // there are padding pixels if input pics are not 8 pixel aligned
            // but there is no extra padding after input pics are resized for
            // reference scaling
            Yv12BufferConfig cpi_source;
            svt_aom_link_eb_to_aom_buffer_desc(input_pic,
                                               &cpi_source,
                                               is_resized ? 0 : scs->max_input_pad_right,
                                               is_resized ? 0 : scs->max_input_pad_bottom,
                                               is_16bit);

            Yv12BufferConfig trial_frame_rst;
            svt_aom_link_eb_to_aom_buffer_desc(context_ptr->trial_frame_rst,
                                               &trial_frame_rst,
                                               is_resized ? 0 : scs->max_input_pad_right,
                                               is_resized ? 0 : scs->max_input_pad_bottom,
                                               is_16bit);

            Yv12BufferConfig org_fts;
            svt_aom_link_eb_to_aom_buffer_desc(recon_pic,
                                               &org_fts,
                                               is_resized ? 0 : scs->max_input_pad_right,
                                               is_resized ? 0 : scs->max_input_pad_bottom,
                                               is_16bit);

            if (ppcs->slice_type != I_SLICE && cm->wn_filter_ctrls.enabled &&
                cm->wn_filter_ctrls.use_prev_frame_coeffs) {
                EbReferenceObject* ref_obj_l0 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
                for (int32_t plane = 0; plane < MAX_MB_PLANE; ++plane) {
                    int32_t ntiles = pcs->rst_info[plane].units_per_tile;
                    for (int32_t u = 0; u < ntiles; ++u) {
                        pcs->rst_info[plane].unit_info[u].restoration_type =
                            ref_obj_l0->unit_info[plane][u].restoration_type;
                        if (ref_obj_l0->unit_info[plane][u].restoration_type == RESTORE_WIENER) {
                            pcs->rst_info[plane].unit_info[u].wiener_info = ref_obj_l0->unit_info[plane][u].wiener_info;
                        }
                    }
                }
            }
            restoration_seg_search(
                context_ptr->rst_tmpbuf, &org_fts, &cpi_source, &trial_frame_rst, pcs, cdef_results->segment_index);
        }

        //all seg based search is done. update total processed segments. if all done, finish the search and perfrom application.
        svt_block_on_mutex(pcs->rest_search_mutex);

        pcs->tot_seg_searched_rest++;
        if (pcs->tot_seg_searched_rest == pcs->rest_segments_total_count) {
            if (ppcs->enable_restoration && frm_hdr->allow_intrabc == 0) {
                rest_finish_search(pcs);

                // Only need recon if REF pic or recon is output
                if (ppcs->is_ref || scs->static_config.recon_enabled) {
                    if (pcs->rst_info[0].frame_restoration_type != RESTORE_NONE ||
                        pcs->rst_info[1].frame_restoration_type != RESTORE_NONE ||
                        pcs->rst_info[2].frame_restoration_type != RESTORE_NONE) {
                        svt_av1_loop_restoration_filter_frame(context_ptr->rst_tmpbuf, cm->frame_to_show, cm, 0);
                    }
                }
            } else {
                pcs->rst_info[0].frame_restoration_type = RESTORE_NONE;
                pcs->rst_info[1].frame_restoration_type = RESTORE_NONE;
                pcs->rst_info[2].frame_restoration_type = RESTORE_NONE;
            }

            // delete scaled_input_pic after lr finished
            EB_DELETE(pcs->scaled_input_pic);
            if (ppcs->ref_pic_wrapper != NULL) {
                // copy stat to ref object (intra_coded_area, Luminance, Scene change detection
                // flags)
                copy_statistics_to_ref_obj_ect(pcs, scs);
            }

            bool superres_recode = ppcs->superres_total_recode_loop > 0 ? true : false;

            // Pad the reference picture and set ref POC
            {
                if (ppcs->is_ref == true) {
                    pad_ref_and_set_flags(pcs, scs);
                } else {
                    // convert non-reference frame buffer from 16-bit to 8-bit, to export recon and
                    // psnr/ssim calculation
                    if (is_16bit && scs->static_config.encoder_bit_depth == EB_EIGHT_BIT) {
                        EbPictureBufferDesc* ref_pic_ptr       = ppcs->enc_dec_ptr->recon_pic;
                        EbPictureBufferDesc* ref_pic_16bit_ptr = ppcs->enc_dec_ptr->recon_pic_16bit;
                        // Y
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
                        svt_convert_16bit_to_8bit(
                            buf_16bit,
                            ref_pic_16bit_ptr->stride_cb,
                            buf_8bit,
                            ref_pic_ptr->stride_cb,
                            (ref_pic_16bit_ptr->width + (ref_pic_ptr->org_x << 1)) >> scs->subsampling_x,
                            (ref_pic_16bit_ptr->height + (ref_pic_ptr->org_y << 1)) >> scs->subsampling_y);

                        //CR
                        buf_16bit = (uint16_t*)(ref_pic_16bit_ptr->buffer_cr);
                        buf_8bit  = ref_pic_ptr->buffer_cr;
                        svt_convert_16bit_to_8bit(
                            buf_16bit,
                            ref_pic_16bit_ptr->stride_cr,
                            buf_8bit,
                            ref_pic_ptr->stride_cr,
                            (ref_pic_16bit_ptr->width + (ref_pic_ptr->org_x << 1)) >> scs->subsampling_x,
                            (ref_pic_16bit_ptr->height + (ref_pic_ptr->org_y << 1)) >> scs->subsampling_y);
                    }
                }
            }

            // PSNR and SSIM Calculation.
            if (superres_recode) { // superres needs psnr to compute rdcost
                // Note: if superres recode is actived, memory needs to be freed in packetization process by calling free_temporal_filtering_buffer()
                EbErrorType return_error = psnr_calculations(pcs, scs, false);
                if (return_error != EB_ErrorNone) {
                    svt_aom_assert_err(0,
                                       "Couldn't allocate memory for uncompressed 10bit buffers for PSNR "
                                       "calculations");
                }
            } else {
                EbErrorType return_error = EB_ErrorNone;
                if (pcs->ppcs->compute_psnr) {
                    // Note: if temporal_filtering is used, memory needs to be freed in the last of these calls
                    return_error = psnr_calculations(pcs, scs, !pcs->ppcs->compute_ssim);
                    if (return_error != EB_ErrorNone) {
                        svt_aom_assert_err(0,
                                           "Couldn't allocate memory for uncompressed 10bit buffers for PSNR "
                                           "calculations");
                    }
                }
                if (pcs->ppcs->compute_ssim) {
                    return_error = svt_aom_ssim_calculations(pcs, scs, true /* free memory here */);
                    if (return_error != EB_ErrorNone) {
                        svt_aom_assert_err(0,
                                           "Couldn't allocate memory for uncompressed 10bit buffers for SSIM "
                                           "calculations");
                    }
                }
            }

            if (!superres_recode) {
                if (scs->static_config.recon_enabled) {
                    svt_aom_recon_output(pcs, scs);
                }
                // post reference picture task in packetization process if it's superres_recode
                if (ppcs->is_ref) {
                    // Get Empty PicMgr Results
                    EbObjectWrapper* picture_demux_results_wrapper_ptr;
                    svt_get_empty_object(context_ptr->picture_demux_fifo_ptr, &picture_demux_results_wrapper_ptr);

                    PictureDemuxResults* picture_demux_results_rtr = (PictureDemuxResults*)
                                                                         picture_demux_results_wrapper_ptr->object_ptr;
                    picture_demux_results_rtr->ref_pic_wrapper = ppcs->ref_pic_wrapper;
                    picture_demux_results_rtr->scs             = pcs->scs;
                    picture_demux_results_rtr->picture_number  = pcs->picture_number;
                    picture_demux_results_rtr->picture_type    = EB_PIC_REFERENCE;

                    // Post Reference Picture
                    svt_post_full_object(picture_demux_results_wrapper_ptr);
                }
            }

            int tile_cols = ppcs->av1_cm->tiles_info.tile_cols;
            int tile_rows = ppcs->av1_cm->tiles_info.tile_rows;

            for (int tile_row_idx = 0; tile_row_idx < tile_rows; tile_row_idx++) {
                for (int tile_col_idx = 0; tile_col_idx < tile_cols; tile_col_idx++) {
                    const int        tile_idx = tile_row_idx * tile_cols + tile_col_idx;
                    EbObjectWrapper* rest_results_wrapper;
                    svt_get_empty_object(context_ptr->rest_output_fifo_ptr, &rest_results_wrapper);
                    RestResults* rest_results = (RestResults*)rest_results_wrapper->object_ptr;
                    rest_results->pcs_wrapper = cdef_results->pcs_wrapper;
                    rest_results->tile_index  = tile_idx;
                    // Post Rest Results
                    svt_post_full_object(rest_results_wrapper);
                }
            }
        }
        svt_release_mutex(pcs->rest_search_mutex);

        // Release input Results
        svt_release_object(cdef_results_wrapper);
    }

    return NULL;
}
