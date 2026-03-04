/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include "enc_handle.h"
#include "md_config_process.h"
#include "pcs.h"
#include "sequence_control_set.h"
#include "me_results.h"
#include "initial_rc_process.h"
#include "initial_rc_results.h"
#include "me_context.h"
#include "utility.h"
#include "reference_object.h"
#include "resize.h"
#include "common_dsp_rtcd.h"
#include "svt_log.h"
#include "pd_process.h"
#include "firstpass.h"

/**************************************
 * Context
 **************************************/
typedef struct LadQueueEntry {
    EbDctor                  dctor;
    PictureParentControlSet* pcs;
} LadQueueEntry;

typedef struct LadQueue {
    // circular buffer holding the entries in decode order; pics should be removed from the buffer in decode order
    LadQueueEntry** cir_buf;
    uint32_t        cir_buf_size;
    uint32_t        head;
    uint32_t        tail;
} LadQueue;

/* look ahead queue constructor*/
static EbErrorType lad_queue_entry_ctor(LadQueueEntry* entry_ptr) {
    entry_ptr->pcs = NULL;
    return EB_ErrorNone;
}

typedef struct InitialRateControlContext {
    EbFifo*   motion_estimation_results_input_fifo_ptr;
    EbFifo*   initialrate_control_results_output_fifo_ptr;
    LadQueue* lad_queue;

} InitialRateControlContext;

/**************************************
 * Macros
 **************************************/
static void initial_rate_control_context_dctor(EbPtr p) {
    EbThreadContext*           thread_ctx = (EbThreadContext*)p;
    InitialRateControlContext* obj        = (InitialRateControlContext*)thread_ctx->priv;

    EB_DELETE_PTR_ARRAY(obj->lad_queue->cir_buf, obj->lad_queue->cir_buf_size);
    obj->lad_queue->cir_buf_size = 0;
    EB_FREE(obj->lad_queue);
    EB_FREE_ARRAY(obj);
}

/************************************************
 * Initial Rate Control Context Constructor
 ************************************************/
EbErrorType svt_aom_initial_rate_control_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                      uint32_t ppcs_count) {
    InitialRateControlContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = initial_rate_control_context_dctor;

    context_ptr->motion_estimation_results_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->motion_estimation_results_resource_ptr, 0);
    context_ptr->initialrate_control_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->initial_rate_control_results_resource_ptr, 0);

    EB_MALLOC_OBJECT(context_ptr->lad_queue);

    context_ptr->lad_queue->cir_buf_size = ppcs_count;
    EB_ALLOC_PTR_ARRAY(context_ptr->lad_queue->cir_buf, ppcs_count);
    for (uint32_t picture_index = 0; picture_index < ppcs_count; ++picture_index) {
        EB_NEW(context_ptr->lad_queue->cir_buf[picture_index], lad_queue_entry_ctor);
    }
    context_ptr->lad_queue->head = 0;
    context_ptr->lad_queue->tail = 0;

    return EB_ErrorNone;
}

#if LAD_MG_PRINT

/*
 get size  of the  lad queue
*/
uint32_t get_lad_q_size(InitialRateControlContext* ctx) {
    uint32_t       size        = 0;
    uint32_t       idx         = ctx->lad_queue->head;
    LadQueueEntry* queue_entry = ctx->lad_queue->cir_buf[idx];
    while (queue_entry->pcs != NULL) {
        queue_entry = ctx->lad_queue->cir_buf[++idx];
        size++;
    }
    return size;
}

/*
 dump the content of the  queue for debug purpose
*/
void print_lad_queue(InitialRateControlContext* ctx, uint8_t log) {
    if (log) {
        LadQueue*      queue       = ctx->lad_queue;
        uint32_t       idx         = queue->head;
        LadQueueEntry* queue_entry = queue->cir_buf[idx];

        SVT_LOG("\n lad_queue size:%i  ", get_lad_q_size(ctx));

        while (queue_entry->pcs != NULL) {
            SVT_LOG("%i-%lld ", queue_entry->pcs->ext_mg_id, queue_entry->pcs->picture_number);
            idx         = OUT_Q_ADVANCE(idx, queue->cir_buf_size);
            queue_entry = queue->cir_buf[idx];
        }
        SVT_LOG("\n");
    }
}
#endif
/*
 store pictures in the lad queue
*/
static void push_to_lad_queue(PictureParentControlSet* pcs, InitialRateControlContext* ctx) {
    LadQueue*      queue       = ctx->lad_queue;
    uint32_t       entry_idx   = pcs->decode_order % queue->cir_buf_size;
    LadQueueEntry* queue_entry = queue->cir_buf[entry_idx];
    svt_aom_assert_err(queue_entry->pcs == NULL, "lad queue overflow");
    if (queue_entry->pcs == NULL) {
        queue_entry->pcs = pcs;
    } else {
        SVT_ERROR("\n lad queue overflow \n");
    }
}

/* send picture out from irc process */
static void irc_send_picture_out(InitialRateControlContext* ctx, PictureParentControlSet* pcs, bool superres_recode) {
    EbObjectWrapper* out_results_wrapper;
    // Get Empty Results Object
    svt_get_empty_object(ctx->initialrate_control_results_output_fifo_ptr, &out_results_wrapper);
    InitialRateControlResults* out_results = (InitialRateControlResults*)out_results_wrapper->object_ptr;
    // SVT_LOG("iRC Out:%lld\n",pcs->picture_number);
    out_results->pcs_wrapper     = pcs->p_pcs_wrapper_ptr;
    out_results->superres_recode = superres_recode;
    svt_post_full_object(out_results_wrapper);
}

static uint8_t is_frame_already_exists(PictureParentControlSet* pcs, uint32_t end_index, uint64_t pic_num) {
    for (uint32_t i = 0; i < end_index; i++) {
        if (pcs->tpl_group[i]->picture_number == pic_num) {
            return 1;
        }
    }
    return 0;
}

// validate pictures that will be used by the tpl algorithm based on tpl opts
void validate_pic_for_tpl(PictureParentControlSet* pcs, uint32_t pic_index) {
    // Check whether the i-th pic already exists in the tpl group
    if (!is_frame_already_exists(pcs, pic_index, pcs->tpl_group[pic_index]->picture_number) &&
        // In the middle pass when rc_stat_gen_pass_mode is set, pictures in the highest temporal layer are skipped,
        // except the first one. The condition is added to prevent validating these frames for tpl
        !svt_aom_is_pic_skipped(pcs->tpl_group[pic_index])) {
        // Discard low important pictures from tpl group
        if (pcs->tpl_ctrls.reduced_tpl_group >= 0) {
            if (pcs->tpl_group[pic_index]->temporal_layer_index <= pcs->tpl_ctrls.reduced_tpl_group) {
                pcs->tpl_valid_pic[pic_index] = 1;
                pcs->used_tpl_frame_num++;
            }
        } else {
            pcs->tpl_valid_pic[pic_index] = 1;
            pcs->used_tpl_frame_num++;
        }
    }
}

uint8_t svt_aom_get_tpl_group_level(uint8_t tpl, int8_t enc_mode) {
    uint8_t tpl_group_level;
    if (!tpl) {
        tpl_group_level = 0;
    } else if (enc_mode <= ENC_M5) {
        tpl_group_level = 1;
    } else if (enc_mode <= ENC_M8) {
        tpl_group_level = 3;
    } else {
        tpl_group_level = 4;
    }
    return tpl_group_level;
}

uint8_t svt_aom_set_tpl_group(PictureParentControlSet* pcs, uint8_t tpl_group_level, uint32_t source_width,
                              uint32_t source_height) {
    TplControls  tpl_ctrls_struct = {0};
    TplControls* tpl_ctrls        = &tpl_ctrls_struct;

    switch (tpl_group_level) {
    case 0:
        tpl_ctrls->enable = 0;
        break;
    case 1:
        tpl_ctrls->enable            = 1;
        tpl_ctrls->reduced_tpl_group = -1;
        tpl_ctrls->synth_blk_size    = 16;
        break;
    case 2:
        tpl_ctrls->enable            = 1;
        tpl_ctrls->reduced_tpl_group = (pcs == NULL) ? -1
            : pcs->slice_type                        ? -1
                                                     : (pcs->hierarchical_levels == 5 ? 4 : 3);
        tpl_ctrls->synth_blk_size    = 16;
        break;
    case 3:
        tpl_ctrls->enable            = 1;
        tpl_ctrls->reduced_tpl_group = (pcs == NULL) ? -1 : pcs->hierarchical_levels == 5 ? 4 : 3;
        tpl_ctrls->synth_blk_size    = 16;
        break;
    case 4:
        tpl_ctrls->enable            = 1;
        tpl_ctrls->reduced_tpl_group = (pcs == NULL) ? -1
            : pcs->hierarchical_levels == 5
            ? pcs->slice_type ? 2 : (pcs->scs->input_resolution <= INPUT_SIZE_480p_RANGE ? 3 : 1)
            : pcs->hierarchical_levels == 4
            ? pcs->slice_type ? 2 : (pcs->scs->input_resolution <= INPUT_SIZE_480p_RANGE ? 2 : 1)
            : pcs->slice_type ? 3
                              : (pcs->scs->input_resolution <= INPUT_SIZE_480p_RANGE ? 2 : 0);
        tpl_ctrls->synth_blk_size    = AOMMIN(source_width, source_height) >= 720 ? 32 : 16;
        break;
    default:
        assert(0);
        break;
    }

    if (pcs == NULL) {
        return tpl_ctrls->synth_blk_size;
    }

    if ((int)pcs->hierarchical_levels <= tpl_ctrls->reduced_tpl_group) {
        tpl_ctrls->reduced_tpl_group = -1;
    }

    // TPL may only look at a subset of available pictures in tpl group, which may affect the r0 calcuation.
    // As a result, we defined a factor to adjust r0 (to compensate for TPL not using all available frames).
    if (tpl_ctrls->reduced_tpl_group >= 0) {
        switch ((pcs->hierarchical_levels - tpl_ctrls->reduced_tpl_group)) {
        case 0:
        default:
            tpl_ctrls->r0_adjust_factor = 0;
            break;
        case 1:
            tpl_ctrls->r0_adjust_factor = pcs->hierarchical_levels <= 2 ? 0.4
                : pcs->hierarchical_levels <= 3                         ? 0.8
                                                                        : 1.6;
            break;
        case 2:
            tpl_ctrls->r0_adjust_factor = pcs->hierarchical_levels <= 2 ? 0.6
                : pcs->hierarchical_levels <= 3                         ? 1.2
                                                                        : 2.4;
            break;
        case 3:
            tpl_ctrls->r0_adjust_factor = pcs->hierarchical_levels <= 3 ? 1.4 : 2.8;
            break;
        case 4:
            tpl_ctrls->r0_adjust_factor = 4.0;
            break;
        case 5:
            tpl_ctrls->r0_adjust_factor = 6.0;
            break;
        }

        // Adjust r0 scaling factor based on GOP structure and lookahead
        if (!pcs->scs->tpl_lad_mg) {
            tpl_ctrls->r0_adjust_factor *= 1.25;
        }
    } else {
        // No r0 adjustment when all frames are used
        tpl_ctrls->r0_adjust_factor = 0;

        // If no lookahead, apply r0 scaling
        if (!pcs->scs->tpl_lad_mg) {
            tpl_ctrls->r0_adjust_factor = pcs->slice_type ? 0
                : pcs->hierarchical_levels <= 2           ? 0.4
                : pcs->hierarchical_levels <= 3           ? 0.8
                                                          : 1.6;
        }
    }
    if (pcs->scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR) {
        tpl_ctrls->r0_adjust_factor *= 1.25;
        tpl_ctrls->r0_adjust_factor = MIN(3, tpl_ctrls->r0_adjust_factor);
    }
    memcpy(&pcs->tpl_ctrls, tpl_ctrls, sizeof(TplControls));
    return tpl_ctrls->synth_blk_size;
}

static uint8_t get_tpl_params_level(int8_t enc_mode) {
    uint8_t tpl_params_level;
    if (enc_mode <= ENC_M2) {
        tpl_params_level = 1;
    } else if (enc_mode <= ENC_M7) {
        tpl_params_level = 4;
    } else {
        tpl_params_level = 5;
    }
    return tpl_params_level;
}

static void set_tpl_params(PictureParentControlSet* pcs, uint8_t tpl_level) {
    TplControls*            tpl_ctrls  = &pcs->tpl_ctrls;
    SequenceControlSet*     scs        = pcs->scs;
    const EbInputResolution resolution = scs->input_resolution;

    switch (tpl_level) {
    case 0:
        tpl_ctrls->compute_rate            = 0;
        tpl_ctrls->enable_tpl_qps          = 0;
        tpl_ctrls->disable_intra_pred_nref = 0;
        tpl_ctrls->intra_mode_end          = DC_PRED;
        tpl_ctrls->pf_shape                = DEFAULT_SHAPE;
        tpl_ctrls->use_sad_in_src_search   = 0;
        tpl_ctrls->dispenser_search_level  = 0;
        tpl_ctrls->subsample_tx            = 0;
        tpl_ctrls->subpel_depth            = FULL_PEL;
        tpl_ctrls->subpel_diag_refinement  = 0;
        break;
    case 1:
        tpl_ctrls->compute_rate            = 1;
        tpl_ctrls->enable_tpl_qps          = 1;
        tpl_ctrls->disable_intra_pred_nref = 0;
        tpl_ctrls->intra_mode_end          = PAETH_PRED;
        tpl_ctrls->pf_shape                = DEFAULT_SHAPE;
        tpl_ctrls->use_sad_in_src_search   = 0;
        tpl_ctrls->dispenser_search_level  = 0;
        tpl_ctrls->subsample_tx            = 0;
        tpl_ctrls->subpel_depth            = QUARTER_PEL;
        tpl_ctrls->subpel_diag_refinement  = 0;
        break;
    case 2:
        tpl_ctrls->compute_rate            = 0;
        tpl_ctrls->enable_tpl_qps          = 0;
        tpl_ctrls->disable_intra_pred_nref = 1;
        tpl_ctrls->intra_mode_end          = PAETH_PRED;
        tpl_ctrls->pf_shape                = resolution <= INPUT_SIZE_480p_RANGE ? N2_SHAPE : N4_SHAPE;
        tpl_ctrls->use_sad_in_src_search   = 1;
        tpl_ctrls->dispenser_search_level  = 0;
        tpl_ctrls->subsample_tx            = 0;
        tpl_ctrls->subpel_depth            = QUARTER_PEL;
        tpl_ctrls->subpel_diag_refinement  = 0;
        break;
    case 3:
        tpl_ctrls->compute_rate            = 0;
        tpl_ctrls->enable_tpl_qps          = 0;
        tpl_ctrls->disable_intra_pred_nref = 1;
        tpl_ctrls->intra_mode_end          = DC_PRED;
        tpl_ctrls->pf_shape                = resolution <= INPUT_SIZE_480p_RANGE ? N2_SHAPE : N4_SHAPE;
        tpl_ctrls->use_sad_in_src_search   = 1;
        tpl_ctrls->dispenser_search_level  = 0;
        tpl_ctrls->subsample_tx            = 0;
        tpl_ctrls->subpel_depth            = QUARTER_PEL;
        tpl_ctrls->subpel_diag_refinement  = 4;
        break;
    case 4:
        tpl_ctrls->compute_rate            = 0;
        tpl_ctrls->enable_tpl_qps          = 0;
        tpl_ctrls->disable_intra_pred_nref = 1;
        tpl_ctrls->intra_mode_end          = DC_PRED;
        tpl_ctrls->pf_shape                = resolution <= INPUT_SIZE_480p_RANGE ? N2_SHAPE : N4_SHAPE;
        tpl_ctrls->use_sad_in_src_search   = 1;
        tpl_ctrls->dispenser_search_level  = 0;
        tpl_ctrls->subsample_tx            = 0;
        tpl_ctrls->subpel_depth            = FULL_PEL;
        tpl_ctrls->subpel_diag_refinement  = 4;
        break;
    case 5:
        tpl_ctrls->compute_rate            = 0;
        tpl_ctrls->enable_tpl_qps          = 0;
        tpl_ctrls->disable_intra_pred_nref = 1;
        tpl_ctrls->intra_mode_end          = DC_PRED;
        tpl_ctrls->pf_shape                = resolution <= INPUT_SIZE_480p_RANGE ? N2_SHAPE : N4_SHAPE;
        tpl_ctrls->use_sad_in_src_search   = 1;
        tpl_ctrls->dispenser_search_level  = 1;
        tpl_ctrls->subsample_tx            = 2;
        tpl_ctrls->subpel_depth            = FULL_PEL;
        tpl_ctrls->subpel_diag_refinement  = 4;
        break;
    default:
        assert(0);
        break;
    }
}

/*
 copy the number of pcs entries from the the output queue to extended  buffer
*/
void store_extended_group(PictureParentControlSet* pcs, InitialRateControlContext* ctx, uint32_t start_idx,
                          int64_t end_mg) {
    LadQueue*      queue = ctx->lad_queue;
    uint32_t       pic_i = 0;
    uint32_t       q_idx = start_idx;
    LadQueueEntry* entry = queue->cir_buf[q_idx];

    while (entry->pcs != NULL) {
        if (entry->pcs->ext_mg_id <= end_mg) {
            svt_aom_assert_err(pic_i < MAX_TPL_EXT_GROUP_SIZE, "exceeding size of ext group");
            pcs->ext_group[pic_i++] = queue->cir_buf[q_idx]->pcs;
        }

        //Increment the queue_index Iterator
        q_idx = OUT_Q_ADVANCE(q_idx, queue->cir_buf_size);
        //get the next entry
        entry = queue->cir_buf[q_idx];
    }

    pcs->ext_group_size = pic_i;
#if LAD_MG_PRINT
    const uint8_t log = 0;
    if (log) {
        SVT_LOG("\n EXT group Pic:%lld  size:%i  \n", pcs->picture_number, pcs->ext_group_size);
        for (uint32_t i = 0; i < pcs->ext_group_size; i++) {
            if (pcs->ext_group[i]->temporal_layer_index == 0) {
                SVT_LOG(" | ");
            }
            SVT_LOG("%lld ", pcs->ext_group[i]->picture_number);
        }
        SVT_LOG("\n");
    }
#endif
    //new tpl group needs to stop at the second I
    pcs->tpl_group_size = 0;
    memset(pcs->tpl_valid_pic, 0, MAX_TPL_EXT_GROUP_SIZE * sizeof(uint8_t));
    pcs->tpl_valid_pic[0]   = 1;
    pcs->used_tpl_frame_num = 0;
    uint8_t  is_gop_end     = 0;
    int64_t  last_intra_mg_id;
    uint32_t mg_size                = 1 << pcs->hierarchical_levels;
    uint32_t limited_tpl_group_size = pcs->slice_type == I_SLICE
        ? MIN(1 + (pcs->scs->tpl_lad_mg + 1) * mg_size, pcs->ext_group_size)
        : MIN((pcs->scs->tpl_lad_mg + 1) * mg_size, pcs->ext_group_size);
    if (pcs->scs->static_config.startup_mg_size > 0) {
        if (pcs->slice_type == I_SLICE) {
            limited_tpl_group_size = MIN(
                1 + pcs->scs->tpl_lad_mg * (1 << pcs->scs->static_config.hierarchical_levels) + mg_size,
                pcs->ext_group_size);
        } else {
            const uint32_t startup_mg_size = 1 << pcs->scs->static_config.startup_mg_size;
            if (pcs->last_idr_picture + startup_mg_size == pcs->picture_number) {
                limited_tpl_group_size = MIN(
                    pcs->scs->tpl_lad_mg * (1 << pcs->scs->static_config.hierarchical_levels) + mg_size,
                    pcs->ext_group_size);
            }
        }
    }
    for (uint32_t i = 0; i < limited_tpl_group_size; i++) {
        PictureParentControlSet* cur_pcs = pcs->ext_group[i];
        if (cur_pcs->slice_type == I_SLICE) {
            if (svt_aom_is_delayed_intra(cur_pcs)) {
                if (i == 0) {
                    pcs->tpl_group[pcs->tpl_group_size++] = cur_pcs;
                    validate_pic_for_tpl(pcs, i);
                } else {
                    break;
                }
            } else {
                if (i == 0) {
                    pcs->tpl_group[pcs->tpl_group_size++] = cur_pcs;
                    validate_pic_for_tpl(pcs, i);
                } else {
                    pcs->tpl_group[pcs->tpl_group_size++] = cur_pcs;
                    validate_pic_for_tpl(pcs, i);
                    last_intra_mg_id = cur_pcs->ext_mg_id;
                    is_gop_end       = 1;
                }
            }
        } else {
            if (is_gop_end == 0) {
                pcs->tpl_group[pcs->tpl_group_size++] = cur_pcs;
                validate_pic_for_tpl(pcs, i);
            } else if (cur_pcs->ext_mg_id == last_intra_mg_id) {
                pcs->tpl_group[pcs->tpl_group_size++] = cur_pcs;
                validate_pic_for_tpl(pcs, i);
            } else {
                break;
            }
        }
    }

    for (uint32_t pic_index = 0; pic_index < pcs->tpl_group_size; pic_index++) {
        if (!pcs->tpl_group[pic_index]->tpl_params_ready) {
            set_tpl_params(pcs->tpl_group[pic_index], get_tpl_params_level(pcs->scs->static_config.enc_mode));
            pcs->tpl_group[pic_index]->tpl_params_ready = 1;
        }
    }

#if LAD_MG_PRINT
    if (log) {
        SVT_LOG("\n NEW TPL group Pic:%lld  gop:%i  size:%i  \n",
                pcs->picture_number,
                pcs->hierarchical_levels,
                pcs->tpl_group_size);
        for (uint32_t i = 0; i < pcs->tpl_group_size; i++) {
            if (pcs->ext_group[i]->temporal_layer_index == 0) {
                SVT_LOG(" | ");
            }
            SVT_LOG("%lld ", pcs->tpl_group[i]->picture_number);
        }
        SVT_LOG("\n");
    }
#endif
}

/*
 scan the queue and determine if pictures can go outside
 pictures are stored in dec order.
 only base pictures are hold. the rest including LDP ones are pass-thru
*/
static void process_lad_queue(InitialRateControlContext* ctx, uint8_t pass_thru) {
    LadQueue*      queue      = ctx->lad_queue;
    LadQueueEntry* head_entry = queue->cir_buf[queue->head];

    while (head_entry->pcs != NULL) {
        PictureParentControlSet* head_pcs = head_entry->pcs;

        uint8_t send_out;
        if (!pass_thru) {
            if (head_pcs->temporal_layer_index == 0) {
                //delayed Intra can use the whole window relative to the next Base
                uint8_t target_mgs = svt_aom_is_delayed_intra(head_pcs) ? head_pcs->scs->lad_mg + 1
                                                                        : head_pcs->scs->lad_mg;
                target_mgs++; //add one for the MG including the head
                {
                    uint8_t num_mgs =
                        0; //number of MGs accumulated so far in the queue including the MG where the head belongs
                    int64_t cur_mg = head_pcs->ext_mg_id;

                    uint32_t       tmp_idx                  = queue->head;
                    LadQueueEntry* tmp_entry                = queue->cir_buf[tmp_idx];
                    uint32_t       tot_acc_frames_in_cur_mg = 0;
                    send_out                                = 0;
                    while (tmp_entry->pcs != NULL) {
                        PictureParentControlSet* tmp_pcs = tmp_entry->pcs;

                        svt_aom_assert_err(tmp_pcs->ext_mg_id >= head_pcs->ext_mg_id, "err in mg id");
                        //adjust the lad if we hit an EOS
                        if (tmp_pcs->end_of_sequence_flag) {
                            target_mgs = MIN(target_mgs,
                                             (uint8_t)(tmp_pcs->ext_mg_id - head_pcs->ext_mg_id +
                                                       1)); //+1: to include the MG where the head belongs

                            head_pcs->end_of_sequence_region = true;
                        }
                        if (tmp_pcs->ext_mg_id >= cur_mg) {
                            if (tmp_pcs->ext_mg_id > cur_mg) {
                                svt_aom_assert_err(tmp_pcs->ext_mg_id == cur_mg + 1, "err continuity in mg id");
                            }

                            tot_acc_frames_in_cur_mg++;

                            if (tot_acc_frames_in_cur_mg == tmp_pcs->ext_mg_size) {
                                num_mgs++;
                                cur_mg                   = tmp_pcs->ext_mg_id;
                                tot_acc_frames_in_cur_mg = 0;
                            }

                            if (num_mgs == target_mgs) {
                                store_extended_group(head_pcs, ctx, queue->head, tmp_pcs->ext_mg_id);
                                send_out = 1;
                                break;
                            }
                        }

                        tmp_idx   = OUT_Q_ADVANCE(tmp_idx, queue->cir_buf_size);
                        tmp_entry = queue->cir_buf[tmp_idx];
                    }
                }

            } else {
                send_out = 1;
            }
        } else {
            send_out = 1;
        }

        if (send_out) {
            if (head_pcs->scs->static_config.pass == ENC_FIRST_PASS ||
                head_pcs->scs->static_config.pass == ENC_SECOND_PASS || head_pcs->scs->lap_rc) {
                head_pcs->stats_in_offset = head_pcs->decode_order;
                svt_block_on_mutex(head_pcs->scs->twopass.stats_buf_ctx->stats_in_write_mutex);
                head_pcs->stats_in_end_offset = head_pcs->ext_group_size && head_pcs->scs->lap_rc
                    ? MIN((uint64_t)(head_pcs->scs->twopass.stats_buf_ctx->stats_in_end_write -
                                     head_pcs->scs->twopass.stats_buf_ctx->stats_in_start),
                          head_pcs->stats_in_offset + (uint64_t)head_pcs->ext_group_size)
                    : (uint64_t)(head_pcs->scs->twopass.stats_buf_ctx->stats_in_end_write -
                                 head_pcs->scs->twopass.stats_buf_ctx->stats_in_start);
                head_pcs->frames_in_sw        = (int)(head_pcs->stats_in_end_offset - head_pcs->stats_in_offset);
                if (head_pcs->scs->enable_dec_order == 0 && head_pcs->scs->lap_rc &&
                    head_pcs->temporal_layer_index == 0) {
                    for (uint64_t num_frames = head_pcs->stats_in_offset; num_frames < head_pcs->stats_in_end_offset;
                         ++num_frames) {
                        FIRSTPASS_STATS* cur_frame = head_pcs->scs->twopass.stats_buf_ctx->stats_in_start + num_frames;
                        if ((int64_t)cur_frame->frame > head_pcs->scs->twopass.stats_buf_ctx->last_frame_accumulated) {
                            svt_av1_accumulate_stats(head_pcs->scs->twopass.stats_buf_ctx->total_stats, cur_frame);
                            head_pcs->scs->twopass.stats_buf_ctx->last_frame_accumulated = (int64_t)cur_frame->frame;
                        }
                    }
                }
                svt_release_mutex(head_pcs->scs->twopass.stats_buf_ctx->stats_in_write_mutex);
            }
            //take the picture out from iRc process
            irc_send_picture_out(ctx, head_pcs, false);
            //advance the head
            head_entry->pcs = NULL;
            queue->head     = OUT_Q_ADVANCE(queue->head, queue->cir_buf_size);
            head_entry      = queue->cir_buf[queue->head];
        } else {
            break;
        }
    }
}

#define HIGH_8x8_DIST_VAR_TH 50000
#define MIN_AVG_ME_DIST 1000
#define VBR_CODED_ERROR_FACTOR 30

/*
 set_1pvbr_param: Set the 1 Pass VBR parameters based on the look ahead data
*/
static void set_1pvbr_param(PictureParentControlSet* pcs) {
    SequenceControlSet* scs = pcs->scs;

    svt_block_on_mutex(scs->twopass.stats_buf_ctx->stats_in_write_mutex);
    pcs->stat_struct = (scs->twopass.stats_buf_ctx->stats_in_start + pcs->picture_number)->stat_struct;
    if (pcs->slice_type != I_SLICE) {
        uint64_t avg_me_dist          = 0;
        uint64_t avg_variance_me_dist = 0;
        for (int b64_idx = 0; b64_idx < pcs->b64_total_count; ++b64_idx) {
            avg_me_dist += pcs->rc_me_distortion[b64_idx];
            avg_variance_me_dist += pcs->me_8x8_cost_variance[b64_idx];
        }
        avg_me_dist /= pcs->b64_total_count;
        avg_variance_me_dist /= pcs->b64_total_count;

        double weight = 1;
        if (avg_variance_me_dist > HIGH_8x8_DIST_VAR_TH) {
            weight = 1.5;
        }

        if (scs->input_resolution <= INPUT_SIZE_480p_RANGE) {
            weight = 1.5 * weight;
        }
        pcs->stat_struct.poc = pcs->picture_number;
        (scs->twopass.stats_buf_ctx->stats_in_start + pcs->picture_number)->stat_struct.total_num_bits = MAX(
            MIN_AVG_ME_DIST, avg_me_dist);
        (scs->twopass.stats_buf_ctx->stats_in_start + pcs->picture_number)->coded_error = (double)avg_me_dist *
            pcs->b64_total_count * weight / VBR_CODED_ERROR_FACTOR;
        (scs->twopass.stats_buf_ctx->stats_in_start + pcs->picture_number)->stat_struct.poc = pcs->picture_number;
    }
    svt_release_mutex(scs->twopass.stats_buf_ctx->stats_in_write_mutex);
}

/* Initial Rate Control Kernel */

/*********************************************************************************
 *
 * @brief
 *  The Initial Rate Control process determines the initial bit budget for each picture
 *  depending on the data gathered in the Picture Analysis and Motion Estimation processes
 *  as well as the settings determined in the Picture Decision process.
 *
 * @par Description:
 *  The Initial Rate Control process employs a sliding window buffer to analyze
 *  multiple pictures if a delay is allowed. Note that no reference picture data is
 *  used in this process.
 *
 * @param[in] Picture
 *  The Initial Rate Control Kernel takes a picture and determines the initial bit budget
 *  for each picture depending on the data that was gathered in Picture Analysis and
 *  Motion Estimation processes
 *
 * @param[out] Bit Budget
 *  Bit Budget is the amount of budgetted bits for a picture
 *
 * @remarks
 *  Temporal noise reduction is currently performed in Initial Rate Control Process.
 *  In the future we might decide to move it to Motion Analysis Process.
 *
 ********************************************************************************/
void* svt_aom_initial_rate_control_kernel(void* input_ptr) {
    EbThreadContext*           thread_ctx  = (EbThreadContext*)input_ptr;
    InitialRateControlContext* context_ptr = (InitialRateControlContext*)thread_ctx->priv;

    EbObjectWrapper* in_results_wrapper_ptr;

    // Segments
    for (;;) {
        // Get Input Full Object
        EB_GET_FULL_OBJECT(context_ptr->motion_estimation_results_input_fifo_ptr, &in_results_wrapper_ptr);

        MotionEstimationResults* in_results_ptr = (MotionEstimationResults*)in_results_wrapper_ptr->object_ptr;
        PictureParentControlSet* pcs            = (PictureParentControlSet*)in_results_ptr->pcs_wrapper->object_ptr;

        // Set the segment counter
        pcs->me_segments_completion_count++;

        // If the picture is complete, proceed
        if (pcs->me_segments_completion_count == pcs->me_segments_total_count) {
            SequenceControlSet* scs = pcs->scs;

            pcs->norm_me_dist = 0;
            if (pcs->slice_type != I_SLICE) {
                uint32_t b64_idx;
                uint64_t dist = 0;
                for (b64_idx = 0; b64_idx < pcs->b64_total_count; ++b64_idx) {
                    dist += pcs->me_8x8_distortion[b64_idx];
                }
                pcs->norm_me_dist = dist / pcs->b64_total_count;
            }

            pcs->tpl_params_ready = 0;
            svt_aom_set_tpl_group(pcs,
                                  svt_aom_get_tpl_group_level(scs->tpl, scs->static_config.enc_mode),
                                  scs->max_input_luma_width,
                                  scs->max_input_luma_height);
            // If TPL results are needed for the current hierarchical layer, but are not available, shut r0-based QPS/QPM
            if (!pcs->tpl_ctrls.enable ||
                (pcs->tpl_ctrls.reduced_tpl_group >= 0 &&
                 pcs->temporal_layer_index > pcs->tpl_ctrls.reduced_tpl_group)) {
                pcs->r0_gen            = 0;
                pcs->r0_qps            = 0;
                pcs->r0_delta_qp_md    = 0;
                pcs->r0_delta_qp_quant = 0;

            } else {
                // When another delta-QP modulator (e.g., Variance Boost) is active alongside TPL,
                // r0_delta_qp_quant has no effect and is assumed equal to r0_delta_qp_md
                pcs->r0_gen = 1;
                if (pcs->hierarchical_levels == 5) { // 6L
                    pcs->r0_qps            = pcs->r0_gen;
                    pcs->r0_delta_qp_md    = pcs->r0_gen && pcs->temporal_layer_index <= 3;
                    pcs->r0_delta_qp_quant = pcs->r0_delta_qp_md && pcs->temporal_layer_index == 0;
                } else if (pcs->hierarchical_levels == 4) { // 5L
                    pcs->r0_qps            = pcs->r0_gen;
                    pcs->r0_delta_qp_md    = pcs->r0_gen && pcs->temporal_layer_index <= 2;
                    pcs->r0_delta_qp_quant = pcs->r0_delta_qp_md && pcs->temporal_layer_index == 0;
                } else if (pcs->hierarchical_levels == 3) { // 4L
                    pcs->r0_qps            = pcs->r0_gen;
                    pcs->r0_delta_qp_md    = pcs->r0_gen && pcs->temporal_layer_index <= 1;
                    pcs->r0_delta_qp_quant = pcs->r0_delta_qp_md && pcs->slice_type == I_SLICE;
                } else { // 3L
                    pcs->r0_qps            = pcs->r0_gen && pcs->temporal_layer_index == 0;
                    pcs->r0_delta_qp_md    = pcs->r0_gen && pcs->temporal_layer_index == 0;
                    pcs->r0_delta_qp_quant = (pcs->r0_delta_qp_md && pcs->slice_type == I_SLICE);
                }
            }
            if (in_results_ptr->task_type == TASK_SUPERRES_RE_ME) {
                // do necessary steps as normal routine
                {
                    // Release Pa Ref pictures when not needed
                    // Don't release if superres recode loop is actived (auto-dual or auto-all mode)
                    if (pcs->superres_total_recode_loop == 0) { // QThreshold or auto-solo mode
                        if (pcs->tpl_ctrls.enable) {
                            for (uint32_t i = 0; i < pcs->tpl_group_size; i++) {
                                if (svt_aom_is_incomp_mg_frame(pcs->tpl_group[i])) {
                                    if (pcs->tpl_group[i]->ext_mg_id == pcs->ext_mg_id + 1) {
                                        svt_aom_release_pa_reference_objects(scs, pcs->tpl_group[i]);
                                    }
                                } else {
                                    if (pcs->tpl_group[i]->ext_mg_id == pcs->ext_mg_id) {
                                        svt_aom_release_pa_reference_objects(scs, pcs->tpl_group[i]);
                                    }
                                }
                                if (pcs->tpl_group[i]->non_tf_input) {
                                    EB_DELETE(pcs->tpl_group[i]->non_tf_input);
                                }
                            }
                        } else {
                            svt_aom_release_pa_reference_objects(scs, pcs);
                        }
                    }

                    /*In case Look-Ahead is zero there is no need to place pictures in the
                      re-order queue. this will cause an artificial delay since pictures come in dec-order*/
                    pcs->frames_in_sw           = 0;
                    pcs->end_of_sequence_region = false;
                }

                // post to downstream process
                irc_send_picture_out(context_ptr, pcs, true);

                // Release the Input Results
                svt_release_object(in_results_wrapper_ptr);
                continue;
            }
            // The quant/dequant params derivation is performaed 1 time per sequence assuming the qindex offset(s) are 0
            // then adjusted per TU prior of the quantization at svt_aom_quantize_inv_quantize() depending on the qindex offset(s)
            if (pcs->picture_number == 0) {
                Quants* const   quants_8bit = &scs->enc_ctx->quants_8bit;
                Dequants* const deq_8bit    = &scs->enc_ctx->deq_8bit;
                svt_av1_build_quantizer(pcs, EB_EIGHT_BIT, 0, 0, 0, 0, 0, quants_8bit, deq_8bit);

                if (scs->static_config.encoder_bit_depth == EB_TEN_BIT) {
                    Quants* const   quants_bd = &scs->enc_ctx->quants_bd;
                    Dequants* const deq_bd    = &scs->enc_ctx->deq_bd;
                    svt_av1_build_quantizer(pcs, EB_TEN_BIT, 0, 0, 0, 0, 0, quants_bd, deq_bd);
                }
            }
            // Set the one pass VBR parameters based on the look ahead data
            if (scs->lap_rc) {
                set_1pvbr_param(pcs);
            }
            // tpl_la can be performed on unscaled frames in super-res q-threshold and auto mode
            if (pcs->tpl_ctrls.enable && !pcs->frame_superres_enabled) {
                svt_set_cond_var(&pcs->me_ready, 1);
            }

            // Release Pa Ref pictures when not needed
            // Release Pa ref when
            //   1. TPL is OFF and
            //   2. super-res mode is NONE or FIXED or RANDOM.
            //     For other super-res modes, pa_ref_objs are needed in TASK_SUPERRES_RE_ME task
            if (pcs->tpl_ctrls.enable == 0 && scs->static_config.superres_mode <= SUPERRES_RANDOM) {
                svt_aom_release_pa_reference_objects(scs, pcs);
            }

            /*In case Look-Ahead is zero there is no need to place pictures in the
              re-order queue. this will cause an artificial delay since pictures come in dec-order*/
            pcs->frames_in_sw           = 0;
            pcs->end_of_sequence_region = false;

            push_to_lad_queue(pcs, context_ptr);
#if LAD_MG_PRINT
            print_lad_queue(context_ptr, 0);
#endif
            // tpl_la can be performed on unscaled frame when in super-res q-threshold and auto mode
            uint8_t lad_queue_pass_thru = !(pcs->tpl_ctrls.enable && !pcs->frame_superres_enabled);
            process_lad_queue(context_ptr, lad_queue_pass_thru);
        }
        // Release the Input Results
        svt_release_object(in_results_wrapper_ptr);
    }
    return NULL;
}
