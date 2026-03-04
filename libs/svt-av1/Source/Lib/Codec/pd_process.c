#include "enc_mode_config.h"
/*
* Copyright(c) 2019 Intel Corporation
* Copyright(c) 2019 Netflix, Inc.
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdlib.h>
#include <string.h>
#include <limits.h>

#include "pd_process.h"
#include "definitions.h"
#include "enc_handle.h"
#include "pcs.h"
#include "sequence_control_set.h"
#include "pic_analysis_process.h"
#include "pic_analysis_results.h"
#include "pd_results.h"
#include "reference_object.h"
#include "EbSvtAv1ErrorCodes.h"
#include "temporal_filtering.h"
#include "object.h"
#include "utility.h"
#include "svt_log.h"
#include "common_dsp_rtcd.h"
#include "resize.h"
#include "svt_malloc.h"
#include "inter_prediction.h"
#include "aom_dsp_rtcd.h"

#include "pic_operators.h"
/************************************************
 * Defines
 ************************************************/
#define LAY1_OFF 3
#define LAY2_OFF 5
#define LAY3_OFF 6
#define LAY4_OFF 7

void             svt_aom_get_max_allocated_me_refs(uint8_t ref_count_used_list0, uint8_t ref_count_used_list1,
                                                   uint8_t* max_ref_to_alloc, uint8_t* max_cand_to_alloc);
void             svt_aom_init_resize_picture(SequenceControlSet* scs, PictureParentControlSet* pcs);
MvReferenceFrame svt_get_ref_frame_type(uint8_t list, uint8_t ref_idx);

static uint32_t calc_ahd(SequenceControlSet* scs, PictureParentControlSet* input_pcs, PictureParentControlSet* ref_pcs,
                         uint8_t* active_region_cnt) {
    uint32_t ahd           = 0;
    uint32_t region_width  = ref_pcs->enhanced_pic->width / scs->picture_analysis_number_of_regions_per_width;
    uint32_t region_height = ref_pcs->enhanced_pic->height / scs->picture_analysis_number_of_regions_per_height;
    // Loop over regions inside the picture
    for (uint32_t region_in_picture_width_index = 0;
         region_in_picture_width_index < scs->picture_analysis_number_of_regions_per_width;
         region_in_picture_width_index++) { // loop over horizontal regions
        for (uint32_t region_in_picture_height_index = 0;
             region_in_picture_height_index < scs->picture_analysis_number_of_regions_per_height;
             region_in_picture_height_index++) { // loop over vertical regions
            uint32_t ahd_per_region = 0;
            for (int bin = 0; bin < HISTOGRAM_NUMBER_OF_BINS; ++bin) {
                ahd_per_region += ABS(
                    (int32_t)input_pcs
                        ->picture_histogram[region_in_picture_width_index][region_in_picture_height_index][bin] -
                    (int32_t)
                        ref_pcs->picture_histogram[region_in_picture_width_index][region_in_picture_height_index][bin]);
            }

            ahd += ahd_per_region;
            if (ahd_per_region > (region_width * region_height)) {
                (*active_region_cnt)++;
            }
        }
    }
    return ahd;
}

static INLINE int get_relative_dist(const OrderHintInfo* oh, int a, int b) {
    if (!oh->enable_order_hint) {
        return 0;
    }

    const int bits = oh->order_hint_bits;

    assert(bits >= 1);
    assert(a >= 0 && a < (1 << bits));
    assert(b >= 0 && b < (1 << bits));

    int       diff = a - b;
    const int m    = 1 << (bits - 1);
    diff           = (diff & (m - 1)) - (diff & m);
    return diff;
}

void svt_av1_setup_skip_mode_allowed(PictureParentControlSet* pcs) {
    FrameHeader*               frm_hdr         = &pcs->frm_hdr;
    const OrderHintInfo* const order_hint_info = &pcs->scs->seq_header.order_hint_info;
    SkipModeInfo* const        skip_mode_info  = &frm_hdr->skip_mode_params;

    skip_mode_info->skip_mode_allowed = 0;
    skip_mode_info->ref_frame_idx_0   = INVALID_IDX;
    skip_mode_info->ref_frame_idx_1   = INVALID_IDX;

    uint32_t* ref_order_hint = pcs->ref_order_hint;

    // If these conditions are true, skip mode is not allowed, so return early
    if (!order_hint_info->enable_order_hint || pcs->slice_type == I_SLICE /*frame_is_intra_only(cm)*/ ||
        frm_hdr->reference_mode == SINGLE_REFERENCE) {
        return;
    }

    const int cur_order_hint     = (int)pcs->cur_order_hint;
    int       ref_order_hints[2] = {-1, INT_MAX};
    int       ref_idx[2]         = {INVALID_IDX, INVALID_IDX};

    // Identify the nearest forward and backward references.
    for (int i = 0; i < INTER_REFS_PER_FRAME; ++i) {
        const int ref_hint = (const int)ref_order_hint[i]; // buf->order_hint;
        if (get_relative_dist(order_hint_info, ref_hint, cur_order_hint) < 0) {
            // Forward reference
            if (ref_order_hints[0] == -1 || get_relative_dist(order_hint_info, ref_hint, ref_order_hints[0]) > 0) {
                ref_order_hints[0] = ref_hint;
                ref_idx[0]         = i;
            }
        } else if (get_relative_dist(order_hint_info, ref_hint, cur_order_hint) > 0) {
            // Backward reference
            if (ref_order_hints[1] == INT_MAX || get_relative_dist(order_hint_info, ref_hint, ref_order_hints[1]) < 0) {
                ref_order_hints[1] = ref_hint;
                ref_idx[1]         = i;
            }
        }
    }

    if (ref_idx[0] != INVALID_IDX && ref_idx[1] != INVALID_IDX) {
        // == Bi-directional prediction ==
        skip_mode_info->skip_mode_allowed = 1;
        skip_mode_info->ref_frame_idx_0   = LAST_FRAME + MIN(ref_idx[0], ref_idx[1]);
        skip_mode_info->ref_frame_idx_1   = LAST_FRAME + MAX(ref_idx[0], ref_idx[1]);
    } else if (ref_idx[0] != INVALID_IDX && ref_idx[1] == INVALID_IDX) {
        // == Forward prediction only ==
        // Identify the second nearest forward reference.
        ref_order_hints[1] = -1;
        for (int i = 0; i < INTER_REFS_PER_FRAME; ++i) {
            const int ref_hint = (const int)ref_order_hint[i]; // buf->order_hint;
            if ((ref_order_hints[0] != -1 && get_relative_dist(order_hint_info, ref_hint, ref_order_hints[0]) < 0) &&
                (ref_order_hints[1] == -1 || get_relative_dist(order_hint_info, ref_hint, ref_order_hints[1]) > 0)) {
                // Second closest forward reference
                ref_order_hints[1] = ref_hint;
                ref_idx[1]         = i;
            }
        }
        if (ref_order_hints[1] != -1) {
            skip_mode_info->skip_mode_allowed = 1;
            skip_mode_info->ref_frame_idx_0   = LAST_FRAME + MIN(ref_idx[0], ref_idx[1]);
            skip_mode_info->ref_frame_idx_1   = LAST_FRAME + MAX(ref_idx[0], ref_idx[1]);
        }
    }
}

uint8_t circ_inc(uint8_t max, uint8_t off, uint8_t input) {
    input++;
    if (input >= max) {
        input = 0;
    }

    if (off == 2) {
        input++;
        if (input >= max) {
            input = 0;
        }
    }

    return input;
}

#define FLASH_TH 5
#define FADE_TH 3
#define SCENE_TH 3000
#define NUM64x64INPIC(w, h) ((w * h) >> (svt_log2f(BLOCK_SIZE_64) << 1))
#define QUEUE_GET_PREVIOUS_SPOT(h, size) (((h) == 0) ? (size) - 1 : (h) - 1)
#define QUEUE_GET_NEXT_SPOT(h, off, size) (((int)(h + off) >= (int)(size)) ? h + off - (int)(size) : h + off)

static void picture_decision_context_dctor(EbPtr p) {
    EbThreadContext*        thread_ctx = (EbThreadContext*)p;
    PictureDecisionContext* obj        = (PictureDecisionContext*)thread_ctx->priv;

    if (obj->prev_picture_histogram) {
        for (int region_in_picture_width_index = 0; region_in_picture_width_index < MAX_NUMBER_OF_REGIONS_IN_WIDTH;
             region_in_picture_width_index++) {
            if (obj->prev_picture_histogram[region_in_picture_width_index]) {
                for (int region_in_picture_height_index = 0;
                     region_in_picture_height_index < MAX_NUMBER_OF_REGIONS_IN_HEIGHT;
                     region_in_picture_height_index++) {
                    EB_FREE_ARRAY(
                        obj->prev_picture_histogram[region_in_picture_width_index][region_in_picture_height_index]);
                }
            }
            EB_FREE_PTR_ARRAY(obj->prev_picture_histogram[region_in_picture_width_index],
                              MAX_NUMBER_OF_REGIONS_IN_HEIGHT);
        }
        EB_FREE_PTR_ARRAY(obj->prev_picture_histogram, MAX_NUMBER_OF_REGIONS_IN_WIDTH);
    }
    EB_FREE_2D(obj->ahd_running_avg);
    EB_FREE_2D(obj->ahd_running_avg_cr);
    EB_FREE_2D(obj->ahd_running_avg_cb);
    EB_FREE_ARRAY(obj);
}

/************************************************
  * Picture Analysis Context Constructor
  ************************************************/
EbErrorType svt_aom_picture_decision_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                  uint8_t calc_hist) {
    PictureDecisionContext* pd_ctx;
    EB_CALLOC_ARRAY(pd_ctx, 1);
    thread_ctx->priv  = pd_ctx;
    thread_ctx->dctor = picture_decision_context_dctor;

    memset(pd_ctx->tf_pic_array, 0, (1 << MAX_TEMPORAL_LAYERS) * sizeof(PictureParentControlSet*));
    pd_ctx->tf_pic_arr_cnt                          = 0;
    pd_ctx->picture_analysis_results_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->picture_analysis_results_resource_ptr, 0);
    pd_ctx->picture_decision_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_decision_results_resource_ptr, 0);
    if (calc_hist) {
        EB_ALLOC_PTR_ARRAY(pd_ctx->prev_picture_histogram, MAX_NUMBER_OF_REGIONS_IN_WIDTH);
        for (uint32_t region_in_picture_width_index = 0; region_in_picture_width_index < MAX_NUMBER_OF_REGIONS_IN_WIDTH;
             region_in_picture_width_index++) { // loop over horizontal regions
            EB_ALLOC_PTR_ARRAY(pd_ctx->prev_picture_histogram[region_in_picture_width_index],
                               MAX_NUMBER_OF_REGIONS_IN_HEIGHT);
            for (uint32_t region_in_picture_height_index = 0;
                 region_in_picture_height_index < MAX_NUMBER_OF_REGIONS_IN_HEIGHT;
                 region_in_picture_height_index++) {
                EB_CALLOC_ARRAY(
                    pd_ctx->prev_picture_histogram[region_in_picture_width_index][region_in_picture_height_index],
                    HISTOGRAM_NUMBER_OF_BINS * sizeof(uint32_t));
            }
        }

        EB_CALLOC_2D(pd_ctx->ahd_running_avg,
                     MAX_NUMBER_OF_REGIONS_IN_WIDTH * sizeof(uint32_t),
                     MAX_NUMBER_OF_REGIONS_IN_HEIGHT * sizeof(uint32_t));
    }
    pd_ctx->reset_running_avg = true;
    pd_ctx->me_fifo_ptr       = svt_system_resource_get_producer_fifo(enc_handle_ptr->me_pool_ptr, 0);

    pd_ctx->mg_progress_id                    = 0;
    pd_ctx->last_i_noise_levels_log1p_fp16[0] = 0;
    pd_ctx->transition_detected               = -1;
    pd_ctx->sframe_poc                        = 0;
    pd_ctx->sframe_due                        = 0;
    pd_ctx->last_long_base_pic                = 0;
    pd_ctx->enable_startup_mg                 = false;
    pd_ctx->is_startup_gop                    = false;
    pd_ctx->sframe_hier_lvls                  = 0;
    pd_ctx->sframe_last_arf                   = 0;
    pd_ctx->next_arf_is_s                     = false;
    return EB_ErrorNone;
}

static bool scene_transition_detector(PictureDecisionContext* pd_ctx, SequenceControlSet* scs,
                                      PictureParentControlSet** parent_pcs_window) {
    PictureParentControlSet* current_pcs_ptr = parent_pcs_window[1];
    PictureParentControlSet* future_pcs_ptr  = parent_pcs_window[2];

    // calculating the frame threshold based on the number of 64x64 blocks in the frame
    uint32_t region_threshold;

    bool is_abrupt_change; // this variable signals an abrupt change (scene change or flash)
    bool is_scene_change; // this variable signals a frame representing a scene change

    uint32_t** ahd_running_avg = pd_ctx->ahd_running_avg;

    uint32_t region_in_picture_width_index;
    uint32_t region_in_picture_height_index;

    uint32_t region_width;
    uint32_t region_height;
    uint32_t region_width_offset;
    uint32_t region_height_offset;

    uint32_t is_abrupt_change_count = 0;
    uint32_t is_scene_change_count  = 0;

    uint32_t region_count_threshold = (uint32_t)(((float)((scs->picture_analysis_number_of_regions_per_width *
                                                           scs->picture_analysis_number_of_regions_per_height) *
                                                          50) /
                                                  100) +
                                                 0.5);

    region_width  = parent_pcs_window[1]->enhanced_pic->width / scs->picture_analysis_number_of_regions_per_width;
    region_height = parent_pcs_window[1]->enhanced_pic->height / scs->picture_analysis_number_of_regions_per_height;

    // Loop over regions inside the picture
    for (region_in_picture_width_index = 0;
         region_in_picture_width_index < scs->picture_analysis_number_of_regions_per_width;
         region_in_picture_width_index++) { // loop over horizontal regions
        for (region_in_picture_height_index = 0;
             region_in_picture_height_index < scs->picture_analysis_number_of_regions_per_height;
             region_in_picture_height_index++) { // loop over vertical regions

            is_abrupt_change = false;
            is_scene_change  = false;

            // accumulative histogram (absolute) differences between the past and current frame
            uint32_t ahd = 0;

            region_width_offset = (region_in_picture_width_index ==
                                   scs->picture_analysis_number_of_regions_per_width - 1)
                ? parent_pcs_window[1]->enhanced_pic->width -
                    (scs->picture_analysis_number_of_regions_per_width * region_width)
                : 0;

            region_height_offset = (region_in_picture_height_index ==
                                    scs->picture_analysis_number_of_regions_per_height - 1)
                ? parent_pcs_window[1]->enhanced_pic->height -
                    (scs->picture_analysis_number_of_regions_per_height * region_height)
                : 0;

            region_width += region_width_offset;
            region_height += region_height_offset;

            region_threshold = SCENE_TH * NUM64x64INPIC(region_width, region_height);

            for (int bin = 0; bin < HISTOGRAM_NUMBER_OF_BINS; ++bin) {
                ahd += ABS((int32_t)current_pcs_ptr
                               ->picture_histogram[region_in_picture_width_index][region_in_picture_height_index][bin] -
                           (int32_t)pd_ctx->prev_picture_histogram[region_in_picture_width_index]
                                                                  [region_in_picture_height_index][bin]);
            }

            if (pd_ctx->reset_running_avg) {
                ahd_running_avg[region_in_picture_width_index][region_in_picture_height_index] = ahd;
            }

            uint32_t ahd_error = ABS(
                (int32_t)ahd_running_avg[region_in_picture_width_index][region_in_picture_height_index] - (int32_t)ahd);

            if (ahd_error > region_threshold && ahd >= ahd_error) {
                is_abrupt_change = true;
            }
            if (is_abrupt_change) {
                // this variable denotes the average intensity difference between the next and the past frames
                uint8_t aid_future_past = (uint8_t)ABS(
                    (int16_t)future_pcs_ptr
                        ->average_intensity_per_region[region_in_picture_width_index][region_in_picture_height_index] -
                    (int16_t)pd_ctx->prev_average_intensity_per_region[region_in_picture_width_index]
                                                                      [region_in_picture_height_index]);
                uint8_t aid_future_present = (uint8_t)ABS(
                    (int16_t)future_pcs_ptr
                        ->average_intensity_per_region[region_in_picture_width_index][region_in_picture_height_index] -
                    (int16_t)current_pcs_ptr
                        ->average_intensity_per_region[region_in_picture_width_index][region_in_picture_height_index]);
                uint8_t aid_present_past = (uint8_t)ABS(
                    (int16_t)current_pcs_ptr
                        ->average_intensity_per_region[region_in_picture_width_index][region_in_picture_height_index] -
                    (int16_t)pd_ctx->prev_average_intensity_per_region[region_in_picture_width_index]
                                                                      [region_in_picture_height_index]);

                if (aid_future_past < FLASH_TH && aid_future_present >= FLASH_TH && aid_present_past >= FLASH_TH) {
                    //SVT_LOG ("\nFlash in frame# %i , %i\n", current_pcs_ptr->picture_number,aid_future_past);
                } else if (aid_future_present < FADE_TH && aid_present_past < FADE_TH) {
                    //SVT_LOG ("\nFlash in frame# %i , %i\n", current_pcs_ptr->picture_number,aid_future_past);
                } else {
                    is_scene_change = true;
                    //SVT_LOG ("\nScene Change in frame# %i , %i\n", current_pcs_ptr->picture_number,aid_future_past);
                }
            } else {
                ahd_running_avg[region_in_picture_width_index][region_in_picture_height_index] =
                    (3 * ahd_running_avg[region_in_picture_width_index][region_in_picture_height_index] + ahd) / 4;
            }
            is_abrupt_change_count += is_abrupt_change;
            is_scene_change_count += is_scene_change;
        }
    }

    pd_ctx->reset_running_avg = is_abrupt_change_count >= region_count_threshold;
    return is_scene_change_count >= region_count_threshold;
}

/***************************************************************************************************
* release_prev_picture_from_reorder_queue
***************************************************************************************************/
EbErrorType release_prev_picture_from_reorder_queue(EncodeContext* enc_ctx) {
    EbErrorType return_error = EB_ErrorNone;

    PictureDecisionReorderEntry* queue_previous_entry_ptr;
    int32_t                      previous_entry_index;

    // Get the previous entry from the Picture Decision Reordering Queue (Entry N-1)
    // P.S. The previous entry in display order is needed for Scene Change Detection
    previous_entry_index     = (enc_ctx->picture_decision_reorder_queue_head_index == 0)
            ? enc_ctx->picture_decision_reorder_queue_size - 1
            : enc_ctx->picture_decision_reorder_queue_head_index - 1;
    queue_previous_entry_ptr = enc_ctx->picture_decision_reorder_queue[previous_entry_index];

    // SB activity classification based on (0,0) SAD & picture activity derivation
    if (queue_previous_entry_ptr->ppcs_wrapper) {
        // Reset the Picture Decision Reordering Queue Entry
        // P.S. The reset of the Picture Decision Reordering Queue Entry could not be done before running the Scene Change Detector
        queue_previous_entry_ptr->picture_number += enc_ctx->picture_decision_reorder_queue_size;
        queue_previous_entry_ptr->ppcs_wrapper = NULL;
    }

    return return_error;
}

static void early_hme_b64(uint8_t* sixteenth_b64_buffer, uint32_t sixteenth_b64_buffer_stride,
                          uint8_t  hme_search_method, //
                          int16_t  org_x, // Block position in the horizontal direction- sixteenth resolution
                          int16_t  org_y, // Block position in the vertical direction- sixteenth resolution
                          uint32_t block_width, // Block width - sixteenth resolution
                          uint32_t block_height, // Block height - sixteenth resolution
                          int16_t  sa_width, // search area width
                          int16_t  sa_height, // search area height
                          EbPictureBufferDesc* sixteenth_ref_pic_ptr, // sixteenth-downsampled reference picture
                          uint64_t*            best_sad, // output: Level0 SAD
                          Mv*                  sr_center // output: Level0 xMV, Level0 yMV
) {
    // round up the search region width to nearest multiple of 8 because the SAD calculation performance (for
    // intrinsic functions) is the same for search region width from 1 to 8
    sa_width           = (int16_t)((sa_width + 7) & ~0x07);
    int16_t pad_width  = (int16_t)(sixteenth_ref_pic_ptr->org_x) - 1;
    int16_t pad_height = (int16_t)(sixteenth_ref_pic_ptr->org_y) - 1;

    int16_t sa_origin_x = -(int16_t)(sa_width >> 1);
    int16_t sa_origin_y = -(int16_t)(sa_height >> 1);

    // Correct the left edge of the Search Area if it is not on the reference picture
    if (((org_x + sa_origin_x) < -pad_width)) {
        sa_origin_x = -pad_width - org_x;
        sa_width    = sa_width - (-pad_width - (org_x + sa_origin_x));
    }

    // Correct the right edge of the Search Area if its not on the reference picture
    if (((org_x + sa_origin_x) > (int16_t)sixteenth_ref_pic_ptr->width - 1)) {
        sa_origin_x = sa_origin_x - ((org_x + sa_origin_x) - ((int16_t)sixteenth_ref_pic_ptr->width - 1));
    }

    if (((org_x + sa_origin_x + sa_width) > (int16_t)sixteenth_ref_pic_ptr->width)) {
        sa_width = MAX(1, sa_width - ((org_x + sa_origin_x + sa_width) - (int16_t)sixteenth_ref_pic_ptr->width));
    }
    // Constrain x_HME_L1 to be a multiple of 8 (round down as cropping alrea performed)
    sa_width = (sa_width < 8) ? sa_width : sa_width & ~0x07;
    // Correct the top edge of the Search Area if it is not on the reference picture
    if (((org_y + sa_origin_y) < -pad_height)) {
        sa_origin_y = -pad_height - org_y;
        sa_height   = sa_height - (-pad_height - (org_y + sa_origin_y));
    }

    // Correct the bottom edge of the Search Area if its not on the reference picture
    if (((org_y + sa_origin_y) > (int16_t)sixteenth_ref_pic_ptr->height - 1)) {
        sa_origin_y = sa_origin_y - ((org_y + sa_origin_y) - ((int16_t)sixteenth_ref_pic_ptr->height - 1));
    }

    if ((org_y + sa_origin_y + sa_height > (int16_t)sixteenth_ref_pic_ptr->height)) {
        sa_height = MAX(1, sa_height - ((org_y + sa_origin_y + sa_height) - (int16_t)sixteenth_ref_pic_ptr->height));
    }

    // Move to the top left of the search region
    int16_t  x_top_left_search_region = ((int16_t)sixteenth_ref_pic_ptr->org_x + org_x) + sa_origin_x;
    int16_t  y_top_left_search_region = ((int16_t)sixteenth_ref_pic_ptr->org_y + org_y) + sa_origin_y;
    uint32_t search_region_index      = x_top_left_search_region +
        y_top_left_search_region * sixteenth_ref_pic_ptr->stride_y;

    // Put the first search location into level0 results
    svt_sad_loop_kernel(
        &sixteenth_b64_buffer[0],
        (hme_search_method == FULL_SAD_SEARCH) ? sixteenth_b64_buffer_stride : sixteenth_b64_buffer_stride * 2,
        &sixteenth_ref_pic_ptr->buffer_y[search_region_index],
        (hme_search_method == FULL_SAD_SEARCH) ? sixteenth_ref_pic_ptr->stride_y : sixteenth_ref_pic_ptr->stride_y * 2,
        (hme_search_method == FULL_SAD_SEARCH) ? block_height : block_height >> 1,
        block_width,
        /* results */
        best_sad,
        &sr_center->x,
        &sr_center->y,
        /* range */
        sixteenth_ref_pic_ptr->stride_y,
        0, // skip search line
        sa_width,
        sa_height);

    *best_sad = (hme_search_method == FULL_SAD_SEARCH)
        ? *best_sad
        : *best_sad * 2; // Multiply by 2 because considered only ever other line

    sr_center->x += sa_origin_x;
    sr_center->x *= 4; // Multiply by 4 because operating on 1/4 resolution
    sr_center->y += sa_origin_y;
    sr_center->y *= 4; // Multiply by 4 because operating on 1/4 resolution

    return;
}

void dg_detector_hme_level0(PictureParentControlSet* ppcs, uint32_t seg_idx) {
    EbPictureBufferDesc* src_sixt_ds_pic =
        ((EbPaReferenceObject*)ppcs->pa_ref_pic_wrapper->object_ptr)->sixteenth_downsampled_picture_ptr;

    EbPictureBufferDesc* ref_sixt_ds_pic =
        ((EbPaReferenceObject*)ppcs->dg_detector->ref_pic->pa_ref_pic_wrapper->object_ptr)
            ->sixteenth_downsampled_picture_ptr;

    int16_t sa_width  = ppcs->input_resolution <= INPUT_SIZE_360p_RANGE ? 16
         : ppcs->input_resolution <= INPUT_SIZE_480p_RANGE              ? 64
                                                                        : 128;
    int16_t sa_height = ppcs->input_resolution <= INPUT_SIZE_360p_RANGE ? 16
        : ppcs->input_resolution <= INPUT_SIZE_480p_RANGE               ? 64
                                                                        : 128;

    uint64_t hme_level0_sad = (uint64_t)~0;
    Mv       sr_center      = {.as_int = 0};

    uint8_t hme_search_method = FULL_SAD_SEARCH;

    // determine the starting and ending block for each segment
    uint32_t pic_width_in_b64  = (ppcs->aligned_width + ppcs->scs->b64_size - 1) / ppcs->scs->b64_size;
    uint32_t pic_height_in_b64 = (ppcs->aligned_height + ppcs->scs->b64_size - 1) / ppcs->scs->b64_size;
    uint32_t y_seg_idx;
    uint32_t x_seg_idx;

    SEGMENT_CONVERT_IDX_TO_XY(seg_idx, x_seg_idx, y_seg_idx, ppcs->me_segments_column_count);
    uint32_t x_b64_start_idx = SEGMENT_START_IDX(x_seg_idx, pic_width_in_b64, ppcs->me_segments_column_count);
    uint32_t x_b64_end_idx   = SEGMENT_END_IDX(x_seg_idx, pic_width_in_b64, ppcs->me_segments_column_count);
    uint32_t y_b64_start_idx = SEGMENT_START_IDX(y_seg_idx, pic_height_in_b64, ppcs->me_segments_row_count);
    uint32_t y_b64_end_idx   = SEGMENT_END_IDX(y_seg_idx, pic_height_in_b64, ppcs->me_segments_row_count);

    for (uint32_t y_b64_idx = y_b64_start_idx; y_b64_idx < y_b64_end_idx; ++y_b64_idx) {
        for (uint32_t x_b64_idx = x_b64_start_idx; x_b64_idx < x_b64_end_idx; ++x_b64_idx) {
            uint32_t b64_origin_x = x_b64_idx * 64;
            uint32_t b64_origin_y = y_b64_idx * 64;

            uint32_t buffer_index = (src_sixt_ds_pic->org_y + (b64_origin_y >> 2)) * src_sixt_ds_pic->stride_y +
                src_sixt_ds_pic->org_x + (b64_origin_x >> 2);

            early_hme_b64(&src_sixt_ds_pic->buffer_y[buffer_index],
                          src_sixt_ds_pic->stride_y,
                          hme_search_method,
                          ((int16_t)b64_origin_x) >> 2,
                          ((int16_t)b64_origin_y) >> 2,
                          16,
                          16,
                          sa_width,
                          sa_height,
                          ref_sixt_ds_pic,
                          &hme_level0_sad,
                          &sr_center);

            // lock the dg metrics calculation using a mutex, only one segment can modify the data at a time
            svt_block_on_mutex(ppcs->dg_detector->metrics_mutex);
            ppcs->dg_detector->metrics.tot_dist += hme_level0_sad;

            ppcs->dg_detector->metrics.tot_cplx += (hme_level0_sad > (16 * 16 * 30));
            ppcs->dg_detector->metrics.tot_active += ((abs(sr_center.x) > 0) || (abs(sr_center.y) > 0));
            if (y_b64_idx < pic_height_in_b64 / 2) {
                if (sr_center.y > 0) {
                    --ppcs->dg_detector->metrics.sum_in_vectors;
                } else if (sr_center.y < 0) {
                    ++ppcs->dg_detector->metrics.sum_in_vectors;
                }
            } else if (y_b64_idx > pic_height_in_b64 / 2) {
                if (sr_center.y > 0) {
                    ++ppcs->dg_detector->metrics.sum_in_vectors;
                } else if (sr_center.y < 0) {
                    --ppcs->dg_detector->metrics.sum_in_vectors;
                }
            }

            // Does the col vector point inwards or outwards?
            if (x_b64_idx < pic_width_in_b64 / 2) {
                if (sr_center.x > 0) {
                    --ppcs->dg_detector->metrics.sum_in_vectors;
                } else if (sr_center.x < 0) {
                    ++ppcs->dg_detector->metrics.sum_in_vectors;
                }
            } else if (x_b64_idx > pic_width_in_b64 / 2) {
                if (sr_center.x > 0) {
                    ++ppcs->dg_detector->metrics.sum_in_vectors;
                } else if (sr_center.x < 0) {
                    --ppcs->dg_detector->metrics.sum_in_vectors;
                }
            }
            svt_release_mutex(ppcs->dg_detector->metrics_mutex);
        }
    }
    svt_block_on_mutex(ppcs->dg_detector->metrics_mutex);
    ppcs->dg_detector->metrics.seg_completed++;
    if (ppcs->dg_detector->metrics.seg_completed == (ppcs->me_segments_column_count * ppcs->me_segments_row_count)) {
        // signal that all the hme_level0 segments have been performed and dg metrics collected for the frame
        svt_post_semaphore(ppcs->dg_detector->frame_done_sem);
    }
    svt_release_mutex(ppcs->dg_detector->metrics_mutex);
}

static void early_hme(PictureDecisionContext* ctx, PictureParentControlSet* src_pcs, PictureParentControlSet* ref_pcs) {
    // store the ref pic so it can be used by dg detector when the src picture is sent to the motion estimation kernel
    src_pcs->dg_detector->ref_pic = ref_pcs;

    uint16_t dg_detector_seg_total_count = (uint16_t)(src_pcs->me_segments_column_count) *
        (uint16_t)(src_pcs->me_segments_row_count);
    // reset all metrics for the frame, must be performed here since the frame can be used again in a future comparison
    src_pcs->dg_detector->metrics.seg_completed  = 0;
    src_pcs->dg_detector->metrics.sum_in_vectors = 0;
    src_pcs->dg_detector->metrics.tot_dist       = 0;
    src_pcs->dg_detector->metrics.tot_cplx       = 0;
    src_pcs->dg_detector->metrics.tot_active     = 0;

    // create segments for the dg detector and send them to the motion estimation kernel
    for (uint16_t seg_idx = 0; seg_idx < dg_detector_seg_total_count; ++seg_idx) {
        EbObjectWrapper*        out_results_wrp;
        PictureDecisionResults* out_results;
        svt_get_empty_object(ctx->picture_decision_results_output_fifo_ptr, &out_results_wrp);
        out_results                = (PictureDecisionResults*)out_results_wrp->object_ptr;
        out_results->pcs_wrapper   = src_pcs->p_pcs_wrapper_ptr;
        out_results->segment_index = seg_idx;
        out_results->task_type     = TASK_DG_DETECTOR_HME;
        svt_post_full_object(out_results_wrp);
    }

    // wait for all segments to complete before the frame based calculations can be performed using the dg metrics
    svt_block_on_semaphore(src_pcs->dg_detector->frame_done_sem);

    // 64x64 Block Loop
    uint32_t pic_width_in_b64  = (src_pcs->aligned_width + 63) / 64;
    uint32_t pic_height_in_b64 = (src_pcs->aligned_height + 63) / 64;

    ctx->mv_in_out_count = src_pcs->dg_detector->metrics.sum_in_vectors * 100 /
        (int)(pic_height_in_b64 * pic_width_in_b64);
    ctx->norm_dist   = src_pcs->dg_detector->metrics.tot_dist / (pic_height_in_b64 * pic_width_in_b64);
    ctx->perc_cplx   = (src_pcs->dg_detector->metrics.tot_cplx * 100) / (pic_height_in_b64 * pic_width_in_b64);
    ctx->perc_active = (src_pcs->dg_detector->metrics.tot_active * 100) / (pic_height_in_b64 * pic_width_in_b64);
}

#define HIGH_DIST_TH 16 * 16 * 18
#define LOW_DIST_TH 16 * 16 * 2

static void calc_mini_gop_activity(PictureDecisionContext* ctx, EncodeContext* enc_ctx, uint64_t top_layer_idx,
                                   uint64_t top_layer_dist, uint8_t top_layer_perc_active, uint8_t top_layer_perc_cplx,
                                   uint64_t sub_layer_idx0, uint64_t sub_layer_dist0, uint8_t sub_layer0_perc_active,
                                   uint8_t sub_layer0_perc_cplx, uint64_t sub_layer_idx1, uint64_t sub_layer_dist1,
                                   uint8_t sub_layer1_perc_active, uint8_t sub_layer1_perc_cplx,
                                   int16_t top_layer_mv_in_out_count, int16_t sub_layer_mv_in_out_count1,
                                   int16_t sub_layer_mv_in_out_count2) {
    (void)top_layer_mv_in_out_count;
    // The bias is function of the previous mini-gop structure towards less switch(es) within the same gop
    // 6L will be maintained unless the presence of a significant change compared to the previous mini-gop
    // To do: make the bias function of the preset; higher is the preset, higher is the bias towards less 6L
    int bias = (enc_ctx->mini_gop_cnt_per_gop > 1 && enc_ctx->previous_mini_gop_hierarchical_levels == 5) ? 25 : 75;
    const bool cond1 = top_layer_perc_active >= 95 && !(sub_layer0_perc_active >= 95 && sub_layer1_perc_active < 75) &&
        !(sub_layer0_perc_active < 75 && sub_layer1_perc_active >= 95);
    const bool cond2 = top_layer_dist > LOW_DIST_TH && sub_layer_dist0 < HIGH_DIST_TH &&
        sub_layer_dist1 < HIGH_DIST_TH && top_layer_perc_cplx > 0 && sub_layer0_perc_cplx < 25 &&
        sub_layer1_perc_cplx < 25 && (((sub_layer_dist0 + sub_layer_dist1) / 2) < ((bias * top_layer_dist) / 100));

    const bool cond3 = MIN(sub_layer_mv_in_out_count1, sub_layer_mv_in_out_count2) > 40 &&
        MAX(sub_layer_mv_in_out_count1, sub_layer_mv_in_out_count2) > 55;

    if (cond1 && (cond2 || cond3)) {
        ctx->mini_gop_activity_array[top_layer_idx]  = true;
        ctx->mini_gop_activity_array[sub_layer_idx0] = false;
        ctx->mini_gop_activity_array[sub_layer_idx1] = false;
    }
}

static void eval_sub_mini_gop(PictureDecisionContext* ctx, EncodeContext* enc_ctx, uint64_t top_layer_idx,
                              uint64_t sub_layer_idx0, uint64_t sub_layer_idx1, PictureParentControlSet* start_pcs,
                              PictureParentControlSet* mid_pcs, PictureParentControlSet* end_pcs) {
    early_hme(ctx, end_pcs, start_pcs);

    uint64_t dist_end_start            = ctx->norm_dist;
    uint8_t  perc_cplx_end_start       = ctx->perc_cplx;
    uint8_t  perc_active_end_start     = ctx->perc_active;
    int16_t  mv_in_out_count_end_start = ctx->mv_in_out_count;
    early_hme(ctx, end_pcs, mid_pcs);

    uint64_t dist_end_mid            = ctx->norm_dist;
    uint8_t  perc_cplx_end_mid       = ctx->perc_cplx;
    uint8_t  perc_active_end_mid     = ctx->perc_active;
    int16_t  mv_in_out_count_end_mid = ctx->mv_in_out_count;

    early_hme(ctx, mid_pcs, start_pcs);

    uint64_t dist_mid_start            = ctx->norm_dist;
    uint8_t  perc_cplx_mid_start       = ctx->perc_cplx;
    uint8_t  perc_active_mid_start     = ctx->perc_active;
    int16_t  mv_in_out_count_mid_start = ctx->mv_in_out_count;

    calc_mini_gop_activity(ctx,
                           enc_ctx,
                           top_layer_idx,
                           dist_end_start,
                           perc_active_end_start,
                           perc_cplx_end_start,
                           sub_layer_idx0,
                           dist_mid_start,
                           perc_active_mid_start,
                           perc_cplx_mid_start,
                           sub_layer_idx1,
                           dist_end_mid,
                           perc_active_end_mid,
                           perc_cplx_end_mid,
                           mv_in_out_count_end_start,
                           mv_in_out_count_end_mid,
                           mv_in_out_count_mid_start);
}

/***************************************************************************************************
* Initializes mini GOP activity array
*
***************************************************************************************************/
static void initialize_mini_gop_activity_array(SequenceControlSet* scs, PictureParentControlSet* pcs,
                                               EncodeContext* enc_ctx, PictureDecisionContext* ctx) {
    (void)scs;

    // Loop over all mini GOPs to initialize the activity
    for (uint32_t gopindex = 0; gopindex < MINI_GOP_MAX_COUNT; ++gopindex) {
        ctx->mini_gop_activity_array[gopindex] = svt_aom_get_mini_gop_stats(gopindex)->hierarchical_levels >
            MIN_HIERARCHICAL_LEVEL;
    }

    // Assign the MGs to be used; if the MG is incomplete, the pre-assignment buffer will hold
    // fewer than (1 << scs->static_config.hierarchical_levels) pics
    if (enc_ctx->pre_assignment_buffer_count >= 32 && !(enc_ctx->pre_assignment_buffer_count == 32 && pcs->idr_flag)) {
        ctx->mini_gop_activity_array[L6_INDEX] = false;
    } else if (enc_ctx->pre_assignment_buffer_count >= 16 &&
               !(enc_ctx->pre_assignment_buffer_count == 16 && pcs->idr_flag)) {
        ctx->mini_gop_activity_array[L5_0_INDEX] = false;

        if ((enc_ctx->pre_assignment_buffer_count - 16) >= 8 &&
            !((enc_ctx->pre_assignment_buffer_count - 16) == 8 && pcs->idr_flag)) {
            ctx->mini_gop_activity_array[L4_2_INDEX] = false;

            if ((enc_ctx->pre_assignment_buffer_count - 16 - 8) >= 4 &&
                !((enc_ctx->pre_assignment_buffer_count - 16 - 8) == 4 && pcs->idr_flag)) {
                ctx->mini_gop_activity_array[L3_6_INDEX] = false;

                if ((enc_ctx->pre_assignment_buffer_count - 16 - 8 - 4) >= 2 &&
                    !((enc_ctx->pre_assignment_buffer_count - 16 - 8 - 4) == 2 && pcs->idr_flag)) {
                    ctx->mini_gop_activity_array[L2_14_INDEX] = false;
                }
            } else if ((enc_ctx->pre_assignment_buffer_count - 16 - 8) >= 2 &&
                       !((enc_ctx->pre_assignment_buffer_count - 16 - 8) == 2 && pcs->idr_flag)) {
                ctx->mini_gop_activity_array[L2_12_INDEX] = false;
            }
        } else if ((enc_ctx->pre_assignment_buffer_count - 16) >= 4 &&
                   !((enc_ctx->pre_assignment_buffer_count - 16) == 4 && pcs->idr_flag)) {
            ctx->mini_gop_activity_array[L3_4_INDEX] = false;

            if ((enc_ctx->pre_assignment_buffer_count - 16 - 4) >= 2 &&
                !((enc_ctx->pre_assignment_buffer_count - 16 - 4) == 2 && pcs->idr_flag)) {
                ctx->mini_gop_activity_array[L2_10_INDEX] = false;
            }
        } else if ((enc_ctx->pre_assignment_buffer_count - 16) >= 2 &&
                   !((enc_ctx->pre_assignment_buffer_count - 16) == 2 && pcs->idr_flag)) {
            ctx->mini_gop_activity_array[L2_8_INDEX] = false;
        }
    } else if (enc_ctx->pre_assignment_buffer_count >= 8 &&
               !(enc_ctx->pre_assignment_buffer_count == 8 && pcs->idr_flag)) {
        ctx->mini_gop_activity_array[L4_0_INDEX] = false;

        if ((enc_ctx->pre_assignment_buffer_count - 8) >= 4 &&
            !((enc_ctx->pre_assignment_buffer_count - 8) == 4 && pcs->idr_flag)) {
            ctx->mini_gop_activity_array[L3_2_INDEX] = false;

            if ((enc_ctx->pre_assignment_buffer_count - 8 - 4) >= 2 &&
                !((enc_ctx->pre_assignment_buffer_count - 8 - 4) == 2 && pcs->idr_flag)) {
                ctx->mini_gop_activity_array[L2_6_INDEX] = false;
            }
        } else if ((enc_ctx->pre_assignment_buffer_count - 8) >= 2 &&
                   !((enc_ctx->pre_assignment_buffer_count - 8) == 2 && pcs->idr_flag)) {
            ctx->mini_gop_activity_array[L2_4_INDEX] = false;
        }
    } else if (enc_ctx->pre_assignment_buffer_count >= 4 &&
               !(enc_ctx->pre_assignment_buffer_count == 4 && pcs->idr_flag)) {
        ctx->mini_gop_activity_array[L3_0_INDEX] = false;

        if ((enc_ctx->pre_assignment_buffer_count - 4) >= 2 &&
            !((enc_ctx->pre_assignment_buffer_count - 4) == 2 && pcs->idr_flag)) {
            ctx->mini_gop_activity_array[L2_2_INDEX] = false;
        }
    } else if ((enc_ctx->pre_assignment_buffer_count) >= 2 &&
               !((enc_ctx->pre_assignment_buffer_count) == 2 && pcs->idr_flag)) {
        ctx->mini_gop_activity_array[L2_0_INDEX] = false;
    }

    // 6L vs. 5L
    if (scs->enable_dg && ctx->mini_gop_activity_array[L6_INDEX] == false) {
        PictureParentControlSet* start_pcs = (PictureParentControlSet*)enc_ctx->pre_assignment_buffer[0]->object_ptr;
        PictureParentControlSet* mid_pcs =
            (PictureParentControlSet*)enc_ctx
                ->pre_assignment_buffer[((1 << scs->static_config.hierarchical_levels) >> 1) - 1]
                ->object_ptr;
        PictureParentControlSet* end_pcs = (PictureParentControlSet*)enc_ctx
                                               ->pre_assignment_buffer[enc_ctx->pre_assignment_buffer_count - 1]
                                               ->object_ptr;
        eval_sub_mini_gop(ctx, enc_ctx, L6_INDEX, L5_0_INDEX, L5_1_INDEX, start_pcs, mid_pcs, end_pcs);
    }
    ctx->list0_only = 0;
    if (scs->list0_only_base) {
        ctx->list0_only = 1;
    }
}

/***************************************************************************************************
* Generates block picture map
*
*
***************************************************************************************************/
static EbErrorType generate_picture_window_split(PictureDecisionContext* pd_ctx, EncodeContext* enc_ctx) {
    pd_ctx->total_number_of_mini_gops = 0;
    // Loop over all mini GOPs
    for (uint32_t gopindex = 0; gopindex < MINI_GOP_MAX_COUNT; gopindex += pd_ctx->mini_gop_activity_array[gopindex]
             ? 1
             : mini_gop_offset[svt_aom_get_mini_gop_stats(gopindex)->hierarchical_levels - MIN_HIERARCHICAL_LEVEL]) {
        // Only for a valid mini GOP
        if (svt_aom_get_mini_gop_stats(gopindex)->end_index < enc_ctx->pre_assignment_buffer_count &&
            !pd_ctx->mini_gop_activity_array[gopindex]) {
            pd_ctx->mini_gop_start_index[pd_ctx->total_number_of_mini_gops] =
                svt_aom_get_mini_gop_stats(gopindex)->start_index;
            pd_ctx->mini_gop_end_index[pd_ctx->total_number_of_mini_gops] =
                svt_aom_get_mini_gop_stats(gopindex)->end_index;
            pd_ctx->mini_gop_length[pd_ctx->total_number_of_mini_gops] = svt_aom_get_mini_gop_stats(gopindex)->length;
            pd_ctx->mini_gop_hierarchical_levels[pd_ctx->total_number_of_mini_gops] =
                svt_aom_get_mini_gop_stats(gopindex)->hierarchical_levels;
            pd_ctx->mini_gop_intra_count[pd_ctx->total_number_of_mini_gops] = 0;
            pd_ctx->mini_gop_idr_count[pd_ctx->total_number_of_mini_gops]   = 0;
            pd_ctx->total_number_of_mini_gops++;
        }
    }
    // Only in presence of at least 1 valid mini GOP
    if (pd_ctx->total_number_of_mini_gops != 0) {
        pd_ctx->mini_gop_intra_count[pd_ctx->total_number_of_mini_gops - 1] =
            enc_ctx->pre_assignment_buffer_intra_count;
        pd_ctx->mini_gop_idr_count[pd_ctx->total_number_of_mini_gops - 1] = enc_ctx->pre_assignment_buffer_idr_count;
    }
    return EB_ErrorNone;
}

/***************************************************************************************************
* Handles an incomplete picture window map
*
*
***************************************************************************************************/
static EbErrorType handle_incomplete_picture_window_map(uint32_t hierarchical_level, PictureDecisionContext* pd_ctx,
                                                        EncodeContext* enc_ctx) {
    EbErrorType return_error = EB_ErrorNone;
    if (pd_ctx->total_number_of_mini_gops == 0) {
        hierarchical_level = MIN(MIN_HIERARCHICAL_LEVEL, hierarchical_level);
        pd_ctx->mini_gop_start_index[pd_ctx->total_number_of_mini_gops] = 0;
        pd_ctx->mini_gop_end_index[pd_ctx->total_number_of_mini_gops]   = enc_ctx->pre_assignment_buffer_count - 1;
        pd_ctx->mini_gop_length[pd_ctx->total_number_of_mini_gops]      = enc_ctx->pre_assignment_buffer_count -
            pd_ctx->mini_gop_start_index[pd_ctx->total_number_of_mini_gops];
        pd_ctx->mini_gop_hierarchical_levels[pd_ctx->total_number_of_mini_gops] = hierarchical_level;

        pd_ctx->total_number_of_mini_gops++;
    } else if (pd_ctx->mini_gop_end_index[pd_ctx->total_number_of_mini_gops - 1] <
               enc_ctx->pre_assignment_buffer_count - 1) {
        pd_ctx->mini_gop_start_index[pd_ctx->total_number_of_mini_gops] =
            pd_ctx->mini_gop_end_index[pd_ctx->total_number_of_mini_gops - 1] + 1;
        pd_ctx->mini_gop_end_index[pd_ctx->total_number_of_mini_gops] = enc_ctx->pre_assignment_buffer_count - 1;
        pd_ctx->mini_gop_length[pd_ctx->total_number_of_mini_gops]    = enc_ctx->pre_assignment_buffer_count -
            pd_ctx->mini_gop_start_index[pd_ctx->total_number_of_mini_gops];
        pd_ctx->mini_gop_hierarchical_levels[pd_ctx->total_number_of_mini_gops] = MIN_HIERARCHICAL_LEVEL;
        pd_ctx->mini_gop_intra_count[pd_ctx->total_number_of_mini_gops - 1]     = 0;
        pd_ctx->mini_gop_idr_count[pd_ctx->total_number_of_mini_gops - 1]       = 0;

        pd_ctx->total_number_of_mini_gops++;
    }

    pd_ctx->mini_gop_intra_count[pd_ctx->total_number_of_mini_gops - 1] = enc_ctx->pre_assignment_buffer_intra_count;
    pd_ctx->mini_gop_idr_count[pd_ctx->total_number_of_mini_gops - 1]   = enc_ctx->pre_assignment_buffer_idr_count;

    return return_error;
}

/*
   This function tells if a picture is part of a short
   mg in RA configuration
*/
uint8_t is_pic_cutting_short_ra_mg(PictureDecisionContext* pd_ctx, PictureParentControlSet* pcs, uint32_t mg_idx) {
    //if the size < complete MG or if there is usage of closed GOP
    if ((pd_ctx->mini_gop_length[mg_idx] < pcs->pred_struct_ptr->pred_struct_entry_count ||
         pd_ctx->mini_gop_idr_count[mg_idx] > 0) &&
        pcs->pred_struct_ptr->pred_type == RANDOM_ACCESS && pcs->idr_flag == false && pcs->cra_flag == false) {
        return 1;
    } else {
        return 0;
    }
}

/***************************************************************************************************
* Gets the pred struct for each frame in the mini-gop(s) that we have available
***************************************************************************************************/
static void get_pred_struct_for_all_frames(PictureDecisionContext* ctx, EncodeContext* enc_ctx) {
    // Loop over all mini GOPs
    for (unsigned int mini_gop_index = 0; mini_gop_index < ctx->total_number_of_mini_gops; ++mini_gop_index) {
        // Loop over picture within the mini GOP
        for (unsigned int pic_idx = ctx->mini_gop_start_index[mini_gop_index];
             pic_idx <= ctx->mini_gop_end_index[mini_gop_index];
             pic_idx++) {
            PictureParentControlSet* pcs =
                (PictureParentControlSet*)enc_ctx->pre_assignment_buffer[pic_idx]->object_ptr;
            SequenceControlSet* scs = pcs->scs;
#if DEBUG_STARTUP_MG_SIZE
            if (pcs->idr_flag || pcs->cra_flag) {
                SVT_LOG("Frame %d, key-frame\n", (int)pcs->picture_number);
            }
            if (pic_idx == ctx->mini_gop_start_index[mini_gop_index]) {
                SVT_LOG("mGOP start %d, mGOP length %d, startup mini-GOP %d\n",
                        (int)pcs->picture_number,
                        ctx->mini_gop_length[mini_gop_index],
                        ctx->enable_startup_mg);
            }
            if (pic_idx == ctx->mini_gop_end_index[mini_gop_index]) {
                SVT_LOG(
                    "mGOP end %d, mGOP length %d\n", (int)pcs->picture_number, ctx->mini_gop_length[mini_gop_index]);
            }
#endif
            pcs->pred_structure      = scs->static_config.pred_structure;
            pcs->hierarchical_levels = pcs->idr_flag ? scs->static_config.hierarchical_levels
                                                     : (uint8_t)ctx->mini_gop_hierarchical_levels[mini_gop_index];
            pcs->pred_struct_ptr     = svt_aom_get_prediction_structure(
                enc_ctx->prediction_structure_group_ptr, pcs->pred_structure, pcs->hierarchical_levels);

            if (scs->static_config.startup_mg_size != 0) {
                if (pcs->idr_flag || pcs->cra_flag) {
                    ctx->enable_startup_mg = true;
                } else if (ctx->enable_startup_mg) {
                    ctx->enable_startup_mg = false;
                }
            }
            if (pcs->idr_flag && pcs->picture_number == 0) {
                ctx->is_startup_gop = true;
            } else if (pcs->idr_flag || pcs->cra_flag) {
                ctx->is_startup_gop = false;
            }
            pcs->is_startup_gop = ctx->is_startup_gop;
        }
    }
}

void svt_aom_get_gm_needed_resolutions(uint8_t ds_lvl, bool* gm_need_full, bool* gm_need_quart, bool* gm_need_sixteen) {
    *gm_need_full    = (ds_lvl == GM_FULL) || (ds_lvl == GM_ADAPT_0);
    *gm_need_quart   = (ds_lvl == GM_DOWN) || (ds_lvl == GM_ADAPT_0) || (ds_lvl == GM_ADAPT_1);
    *gm_need_sixteen = (ds_lvl == GM_DOWN16) || (ds_lvl == GM_ADAPT_1);
}

bool svt_aom_is_pic_skipped(PictureParentControlSet* pcs) {
    if (!pcs->is_ref && pcs->scs->rc_stat_gen_pass_mode && !pcs->first_frame_in_minigop) {
        return true;
    }
    return false;
}

static void prune_sframe_refs(PictureDecisionContext* ctx, PictureParentControlSet* ppcs,
                              MvReferenceFrame ref_frame_arr[], uint8_t* tot_ref_frames) {
    if (ctx->sframe_poc > 0 && ppcs->picture_number < ctx->sframe_poc && ppcs->scs->mfmv_enabled) {
#if DEBUG_SFRAME
        fprintf(stderr, "frame[%u] ref before prune:\t", ppcs->picture_number);
        for (uint8_t i = 0; i < *tot_ref_frames; i++) {
            fprintf(stderr, "%u\t", ref_frame_arr[i]);
        }
#endif // DEBUG_SFRAME
        // check every reference frames, if it's in ref_list0 and direct to S-Frame, remove it from array
        uint32_t sframe_poc = ctx->sframe_poc %
            ((uint64_t)1 << (ppcs->scs->seq_header.order_hint_info.order_hint_bits));
        uint8_t ref_idx = 0;
        while (ref_idx < *tot_ref_frames) {
            MvReferenceFrame rf[2];
            av1_set_ref_frame(rf, ref_frame_arr[ref_idx]);
            if ((rf[0] < BWDREF_FRAME && ppcs->ref_order_hint[rf[0]] == sframe_poc) ||
                (rf[1] < BWDREF_FRAME && ppcs->ref_order_hint[rf[1]] == sframe_poc)) {
                (*tot_ref_frames)--;
                for (uint8_t i = ref_idx; i < *tot_ref_frames; i++) {
                    ref_frame_arr[i] = ref_frame_arr[i + 1];
                }
                ppcs->sframe_ref_pruned = true;
                // ref_idx not increase to prevent skipping next item
                continue;
            }
            ref_idx++;
        };
        // only prune ref_list0 will not make the array zero item, but still add assertion here
        assert(*tot_ref_frames > 0);
#if DEBUG_SFRAME
        fprintf(stderr, "\nframe[%u] ref after prune:\t", ppcs->picture_number);
        for (uint8_t i = 0; i < *tot_ref_frames; i++) {
            fprintf(stderr, "%u\t", ref_frame_arr[i]);
        }
        fprintf(stderr, "\n");
#endif // DEBUG_SFRAME
    }
}

//set the ref frame types used for this picture,
static void set_all_ref_frame_type(PictureDecisionContext* ctx, PictureParentControlSet* ppcs,
                                   MvReferenceFrame ref_frame_arr[], uint8_t* tot_ref_frames) {
    MvReferenceFrame rf[2];
    *tot_ref_frames = 0;

    //SVT_LOG("POC %i  totRef L0:%i   totRef L1: %i\n", ppcs->picture_number, ppcs->ref_list0_count, ppcs->ref_list1_count);

    //single ref - List0
    for (uint8_t ref_idx0 = 0; ref_idx0 < ppcs->ref_list0_count_try; ++ref_idx0) {
        rf[0]                              = svt_get_ref_frame_type(REF_LIST_0, ref_idx0);
        ref_frame_arr[(*tot_ref_frames)++] = rf[0];
    }

    //single ref - List1
    for (uint8_t ref_idx1 = 0; ref_idx1 < ppcs->ref_list1_count_try; ++ref_idx1) {
        rf[1]                              = svt_get_ref_frame_type(REF_LIST_1, ref_idx1);
        ref_frame_arr[(*tot_ref_frames)++] = rf[1];
    }

    //compound Bi-Dir
    for (uint8_t ref_idx0 = 0; ref_idx0 < ppcs->ref_list0_count_try; ++ref_idx0) {
        for (uint8_t ref_idx1 = 0; ref_idx1 < ppcs->ref_list1_count_try; ++ref_idx1) {
            rf[0]                              = svt_get_ref_frame_type(REF_LIST_0, ref_idx0);
            rf[1]                              = svt_get_ref_frame_type(REF_LIST_1, ref_idx1);
            ref_frame_arr[(*tot_ref_frames)++] = av1_ref_frame_type(rf);
        }
    }
    if (ppcs->slice_type == B_SLICE) {
        //compound Uni-Dir
        if (ppcs->ref_list0_count_try > 1) {
            rf[0]                              = LAST_FRAME;
            rf[1]                              = LAST2_FRAME;
            ref_frame_arr[(*tot_ref_frames)++] = av1_ref_frame_type(rf);
            if (ppcs->ref_list0_count_try > 2) {
                rf[1]                              = LAST3_FRAME;
                ref_frame_arr[(*tot_ref_frames)++] = av1_ref_frame_type(rf);
                if (ppcs->ref_list0_count_try > 3) {
                    rf[1]                              = GOLDEN_FRAME;
                    ref_frame_arr[(*tot_ref_frames)++] = av1_ref_frame_type(rf);
                }
            }
        }
        if (ppcs->ref_list1_count_try > 2) {
            rf[0]                              = BWDREF_FRAME;
            rf[1]                              = ALTREF_FRAME;
            ref_frame_arr[(*tot_ref_frames)++] = av1_ref_frame_type(rf);
        }
    }

    // The S-Frame feature in RA mode refreshes all reference frames at the S-Frame position (ARF).
    // However, in decode order, the remaining frames in this mini-GOP reference the S-Frame through
    // [LAST, LAST2, LAST3, GOLD]. When MFMV is enabled, the reference MVs to the S-Frame are duplicated
    // and have reversed direction. Prune the S-Frame reference types from ref_list0 to avoid conflicts.
    prune_sframe_refs(ctx, ppcs, ref_frame_arr, tot_ref_frames);
}

static void prune_refs(Av1RpsNode* av1_rps, unsigned ref_list0_count, unsigned ref_list1_count) {
    if (ref_list0_count < 4) {
        av1_rps->ref_dpb_index[GOLD] = av1_rps->ref_dpb_index[LAST];
        av1_rps->ref_poc_array[GOLD] = av1_rps->ref_poc_array[LAST];
    }
    if (ref_list0_count < 3) {
        av1_rps->ref_dpb_index[LAST3] = av1_rps->ref_dpb_index[LAST];
        av1_rps->ref_poc_array[LAST3] = av1_rps->ref_poc_array[LAST];
    }
    if (ref_list0_count < 2) {
        av1_rps->ref_dpb_index[LAST2] = av1_rps->ref_dpb_index[LAST];
        av1_rps->ref_poc_array[LAST2] = av1_rps->ref_poc_array[LAST];
    }

    // If not list 1 refs are used, set the refs to the list 0 ref
    if (ref_list1_count < 1) {
        av1_rps->ref_dpb_index[BWD] = av1_rps->ref_dpb_index[LAST];
        av1_rps->ref_poc_array[BWD] = av1_rps->ref_poc_array[LAST];
    }
    if (ref_list1_count < 3) {
        av1_rps->ref_dpb_index[ALT] = av1_rps->ref_dpb_index[BWD];
        av1_rps->ref_poc_array[ALT] = av1_rps->ref_poc_array[BWD];
    }
    if (ref_list1_count < 2) {
        av1_rps->ref_dpb_index[ALT2] = av1_rps->ref_dpb_index[BWD];
        av1_rps->ref_poc_array[ALT2] = av1_rps->ref_poc_array[BWD];
    }
}

// Set the show_frame and show_existing_frame for current picture if it's:
// 1)Low delay P, 2)Low delay b and 3)I frames of RA
// For b frames of RA, need to set it manually based on picture_index
static bool set_frame_display_params(PictureParentControlSet* pcs, PictureDecisionContext* pd_ctx,
                                     uint32_t mini_gop_index) {
    FrameHeader* frm_hdr = &pcs->frm_hdr;

    if (pcs->pred_struct_ptr->pred_type == LOW_DELAY || pcs->is_overlay) {
        frm_hdr->show_frame    = true;
        pcs->has_show_existing = false;
    } else if (pcs->pred_struct_ptr->pred_type == RANDOM_ACCESS) {
        //Decide on Show Mecanism
        if (pcs->slice_type == I_SLICE) {
            //3 cases for I slice:  1:Key Frame treated above.  2: broken MiniGop due to sc or intra refresh  3: complete miniGop due to sc or intra refresh
            if (pd_ctx->mini_gop_length[mini_gop_index] < pcs->pred_struct_ptr->pred_struct_entry_count) {
                //Scene Change that breaks the mini gop and switch to LDP (if I scene change happens to be aligned with a complete miniGop, then we do not break the pred structure)
                frm_hdr->show_frame    = true;
                pcs->has_show_existing = false;
            } else {
                frm_hdr->show_frame    = false;
                pcs->has_show_existing = false;
            }
        } else {
            if (pd_ctx->mini_gop_length[mini_gop_index] != pcs->pred_struct_ptr->pred_struct_entry_count) {
                SVT_LOG("Error in GOP indexing3\n");
            }
            // Handle b frame of Random Access out
            return false;
        }
    }
    return true;
}

static void set_key_frame_rps(PictureParentControlSet* pcs, PictureDecisionContext* pd_ctx) {
    FrameHeader* frm_hdr = &pcs->frm_hdr;
    pd_ctx->lay0_toggle  = 0;
    pd_ctx->lay1_toggle  = 0;

    frm_hdr->show_frame    = true;
    pcs->has_show_existing = false;
    return;
}

// returns the distance to the nearest S-Frame, and dist_to_next_s will be filled if current is an S-Frame
// dist_to_next_s is for there being more than one S-Frame inserted within one miniGOP size
static int32_t get_dist_to_s(SvtAv1SFramePositions const* sframe_posi, uint64_t picture_num, int32_t* dist_to_next_s) {
    *dist_to_next_s = -1;
    for (uint32_t i = 0; i < sframe_posi->sframe_num; i++) {
        if (sframe_posi->sframe_posis[i] >= picture_num) {
            if (sframe_posi->sframe_posis[i] == picture_num) {
                *dist_to_next_s = (i < sframe_posi->sframe_num - 1)
                    ? (int32_t)(sframe_posi->sframe_posis[i + 1] - picture_num)
                    : -1;
            }
            return (int32_t)(sframe_posi->sframe_posis[i] - picture_num);
        }
    }
    return -1; // all s-frame spots are expired
}

static uint8_t get_sframe_qp(SvtAv1SFramePositions const* sframe_posi, uint64_t picture_num) {
    if (sframe_posi->sframe_qps == NULL) {
        return 0;
    }
    if (sframe_posi->sframe_posis == NULL) {
        // always return first QP if not use flexible S-Frame position list
        return sframe_posi->sframe_qps[0];
    }
    for (uint32_t i = 0; i < sframe_posi->sframe_num; i++) {
        if (sframe_posi->sframe_posis[i] == picture_num) {
            return sframe_posi->sframe_qps[i];
        }
    }
    return 0; // not find the picture
}

static int8_t get_sframe_qp_offset(SvtAv1SFramePositions const* sframe_posi, uint64_t picture_num) {
    if (sframe_posi->sframe_qp_offsets == NULL) {
        return 0;
    }
    if (sframe_posi->sframe_posis == NULL) {
        // always return first QP offset if not use flexible S-Frame position list
        return sframe_posi->sframe_qp_offsets[0];
    }
    for (uint32_t i = 0; i < sframe_posi->sframe_num; i++) {
        if (sframe_posi->sframe_posis[i] == picture_num) {
            return sframe_posi->sframe_qp_offsets[i];
        }
    }
    return 0; // not find the picture
}

static void setup_sframe_qp(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs       = ppcs->scs;
    uint64_t            pic_num   = scs->static_config.sframe_mode == SFRAME_DEC_POSI_BASE ? ppcs->decode_order
                                                                                           : ppcs->picture_number;
    uint8_t             sframe_qp = scs->static_config.sframe_qp > 0 ? scs->static_config.sframe_qp
                                                                     : get_sframe_qp(&scs->static_config.sframe_posi, pic_num);
    if (sframe_qp > 0) {
        ppcs->picture_qp = (uint8_t)CLIP3(
            (int8_t)scs->static_config.min_qp_allowed, (int8_t)scs->static_config.max_qp_allowed, (int8_t)sframe_qp);
        ppcs->qp_on_the_fly = true;
    }
    int8_t sframe_qp_offset = scs->static_config.sframe_qp_offset != 0
        ? scs->static_config.sframe_qp_offset
        : get_sframe_qp_offset(&scs->static_config.sframe_posi, pic_num);
    if (sframe_qp_offset != 0) {
        ppcs->sframe_qp_offset = sframe_qp_offset;
    }
}

// Adjust the S-frame position offset for S-frame decode order mode.
// In low-delay mode, the decode order is the same as the display order,
// so the offset adjustment is unnecessary.
static int32_t sframe_position_offset(SequenceControlSet* scs) {
    return (scs->static_config.sframe_mode == SFRAME_DEC_POSI_BASE &&
            scs->static_config.pred_structure == RANDOM_ACCESS)
        ? 1
        : 0;
}

// Decide whether to make an inter frame into an S-Frame
static void set_sframe_type(PictureParentControlSet* ppcs, EncodeContext* enc_ctx, PictureDecisionContext* pd_ctx) {
    FrameHeader*       frm_hdr     = &ppcs->frm_hdr;
    const int          sframe_dist = enc_ctx->sf_cfg.sframe_dist;
    const EbSFrameMode sframe_mode = enc_ctx->sf_cfg.sframe_mode;

    const int      is_arf           = ppcs->temporal_layer_index == 0 ? true : false;
    const uint64_t frames_since_key = ppcs->picture_number - pd_ctx->key_poc;
    if (sframe_mode == SFRAME_STRICT_BASE) {
        // SFRAME_STRICT_ARF: insert sframe if it matches altref frame.
        if (is_arf && (frames_since_key % sframe_dist) == 0) {
            frm_hdr->frame_type = S_FRAME;
        }
    } else if (sframe_mode == SFRAME_NEAREST_BASE) {
        // SFRAME_NEAREST_ARF: if sframe will be inserted at the next available altref frame
        if (ppcs->scs->static_config.pred_structure == RANDOM_ACCESS) {
            // frames in PD are in decode order, when ARF position is in this miniGop range,
            // the ARF should be the next S-Frame
            if (is_arf && (frames_since_key % sframe_dist) < pd_ctx->mg_size) {
                frm_hdr->frame_type = S_FRAME;
            }
        } else {
            if ((frames_since_key % sframe_dist) == 0) {
                pd_ctx->sframe_due = 1;
            }
            if (pd_ctx->sframe_due && is_arf) {
                frm_hdr->frame_type = S_FRAME;
                pd_ctx->sframe_due  = 0;
            }
        }
    } else {
        // SFRAME_FLEXIBLE_ARF: if the considered frame is not an altref frame, modify the mini-GOP structure to promote it to an altref frame
        if (is_arf) {
            // SFRAME_DEC_POSI_BASE: adjust the frame before insert position to be ARF, and set the next ARF as S-Frame
            int32_t sframe_offset = sframe_position_offset(ppcs->scs);
            // set this ARF to S-Frame if it is decided by previous processing
            if (pd_ctx->next_arf_is_s) {
                frm_hdr->frame_type   = S_FRAME;
                pd_ctx->next_arf_is_s = false; // reset flag of next ARF setting to S-Frame
            }

            uint32_t next_mg_size = 1 << pd_ctx->sframe_hier_lvls;
            if (ppcs->scs->static_config.sframe_posi.sframe_posis) {
                // When the user specifies the positions of S-Frames, the encoder retrieves the distances to the next two S-Frames
                // to assist in deciding the mini-GOP structure.
                int32_t dist_to_next_s = 0;
                int32_t dist_to_s      = get_dist_to_s(
                    &ppcs->scs->static_config.sframe_posi, ppcs->picture_number + sframe_offset, &dist_to_next_s);
                if (dist_to_s == 0) {
                    if (sframe_offset) {
                        pd_ctx->next_arf_is_s = true; // delay setting SFRAME
                    } else {
                        frm_hdr->frame_type = S_FRAME;
                    }

                    // After inserting a new S-Frame, reset sframe_hier_lvls and use it for the next mini-GOP evaluation.
                    pd_ctx->sframe_hier_lvls = ppcs->scs->static_config.hierarchical_levels;
                    next_mg_size             = 1 << pd_ctx->sframe_hier_lvls;
                    dist_to_s                = dist_to_next_s;
                }
                if (dist_to_s > 0 && dist_to_s < (int32_t)next_mg_size) {
                    for (int32_t lvl = 0; lvl < pd_ctx->sframe_hier_lvls; lvl++) {
                        if (dist_to_s < (1 << (lvl + 1))) {
                            pd_ctx->sframe_hier_lvls = lvl;
                            break;
                        }
                    }
                    assert(pd_ctx->sframe_hier_lvls >= 0 &&
                           pd_ctx->sframe_hier_lvls <= (int32_t)ppcs->scs->static_config.hierarchical_levels);
                }
            } else {
                if (((frames_since_key + sframe_offset) % sframe_dist) == 0) {
                    if (sframe_offset) {
                        pd_ctx->next_arf_is_s = true; // delay setting SFRAME
                    } else {
                        frm_hdr->frame_type = S_FRAME;
                    }

                    // After inserting a new S-Frame, reset sframe_hier_lvls and use it for the next mini-GOP evaluation.
                    pd_ctx->sframe_hier_lvls = ppcs->scs->static_config.hierarchical_levels;
                    next_mg_size             = 1 << pd_ctx->sframe_hier_lvls;
                }
                // check the next key frame position for if the distance of next sframe being available
                if (sframe_mode != SFRAME_DEC_POSI_BASE || ppcs->scs->static_config.intra_period_length <= 0 ||
                    (frames_since_key + next_mg_size <= (uint64_t)ppcs->scs->static_config.intra_period_length)) {
                    // modify hierarchical level of next miniGOP
                    uint32_t gap_arf = (frames_since_key + sframe_offset + next_mg_size) % sframe_dist;
                    if (gap_arf != 0 && gap_arf < next_mg_size) {
                        // Downgrade the next mini-GOP if it contains the upcoming S-Frame.
                        int32_t arf_dist = next_mg_size - gap_arf;
                        for (int32_t lvl = 0; lvl < pd_ctx->sframe_hier_lvls; lvl++) {
                            if (arf_dist < (1 << (lvl + 1))) {
                                pd_ctx->sframe_hier_lvls = lvl;
                                break;
                            }
                        }
                        assert(pd_ctx->sframe_hier_lvls >= 0 &&
                               pd_ctx->sframe_hier_lvls <= (int32_t)ppcs->scs->static_config.hierarchical_levels);
                    }
                }
            }
            pd_ctx->sframe_last_arf = frames_since_key;
        }
    }

    if (frm_hdr->frame_type == S_FRAME) {
        setup_sframe_qp(ppcs);
    }

    ppcs->sframe_ref_pruned = false;
#if DEBUG_SFRAME
    if (frm_hdr->frame_type == S_FRAME) {
        fprintf(stderr, "\nFrame %d - set sframe\n", (int)ppcs->picture_number);
    }
#endif
    return;
}

// Determine the size of the first mini-GOP after inserting a key frame
static void decide_sframe_mg(PictureParentControlSet* ppcs, EncodeContext* enc_ctx, PictureDecisionContext* pd_ctx) {
    SequenceControlSet* scs           = ppcs->scs;
    int32_t             sframe_dist   = enc_ctx->sf_cfg.sframe_dist;
    int32_t             sframe_offset = sframe_position_offset(scs);
    // reset next_arf_sframe when key frame inserted
    pd_ctx->next_arf_is_s = false;
    // reset sframe_hier_lvls when key frame inserted
    pd_ctx->sframe_hier_lvls = scs->static_config.hierarchical_levels;

    int32_t next_mg_size = 1 << pd_ctx->sframe_hier_lvls;
    if (scs->static_config.sframe_posi.sframe_posis) {
        int32_t dist_to_next_s = 0;
        int32_t dist_to_s      = get_dist_to_s(
            &ppcs->scs->static_config.sframe_posi, ppcs->picture_number + sframe_offset, &dist_to_next_s);
        if (dist_to_s > 0) {
            sframe_dist = (uint32_t)dist_to_s;
        } else if (dist_to_s == 0 && dist_to_next_s > 0) {
            sframe_dist = (uint32_t)dist_to_next_s;
        } else {
            return;
        }
    }
    if (sframe_dist < next_mg_size) {
        // If the S-Frame falls within the next mini-GOP, downgrade the next mini-GOP.
        for (int32_t lvl = 0; lvl < pd_ctx->sframe_hier_lvls; lvl++) {
            if (sframe_dist < (1 << (lvl + 1))) {
                pd_ctx->sframe_hier_lvls = lvl;
                break;
            }
        }
        assert(pd_ctx->sframe_hier_lvls >= 0 &&
               pd_ctx->sframe_hier_lvls <= (int32_t)scs->static_config.hierarchical_levels);
    }
    return;
}

// Update RPS info for S-Frame
static void set_sframe_rps(PictureParentControlSet* ppcs, EncodeContext* enc_ctx, PictureDecisionContext* pd_ctx) {
    ppcs->frm_hdr.error_resilient_mode      = 1;
    ppcs->av1_ref_signal.refresh_frame_mask = 0xFF;

    pd_ctx->lay0_toggle = 0;
    pd_ctx->lay1_toggle = 0;
    // Bookmark latest switch frame poc to prevent following frames referencing frames before the switch frame
    pd_ctx->sframe_poc = ppcs->picture_number;
    // Reset pred_struct_position
    enc_ctx->elapsed_non_cra_count = 0;
    return;
}

/*************************************************
* AV1 Reference Picture Signalling:
* Stateless derivation of RPS info to be stored in
* Picture Header
*
* This function uses the picture index from the just
* collected miniGop to derive the RPS(refIndexes+refresh)
* the miniGop is always 4L but could be complete (8 pictures)
or non-complete (less than 8 pictures).
* We get to this function if the picture is:
* 1) first Key frame
* 2) part of a complete RA MiniGop where the last frame could be a regular I for open GOP
* 3) part of complete LDP MiniGop where the last frame could be Key frame for closed GOP
* 4) part of non-complete LDP MiniGop where the last frame is a regularI+SceneChange.
This miniGOP has P frames with predStruct=LDP, and the last frame=I with pred struct=RA.
* 5) part of non-complete LDP MiniGop at the end of the stream.This miniGOP has P frames with
predStruct=LDP, and the last frame=I with pred struct=RA.
*
*Note: the  SceneChange I has pred_type = RANDOM_ACCESS. if SChange is aligned on the miniGop,
we do not break the GOP.
*************************************************/
/*
 * Return true if a picture is used as a reference, false otherwise.
 *
 * Whether a picture is used as a reference depends on its position in the hierarchical structure, and on the referencing_scheme used.
 * referencing_scheme = 0 means that no top-layer pictures will be used as a reference
 * referencing_scheme = 1 means that all top-layer pictures may be used as a reference
 * referencing_scheme = 2 means that some top-layer pictures will be used as a reference (depending on their position in the MG)
 *
 * Interal pictures (non-top-layer pictures) are always used as a reference.  Overlay pictures are never used as a reference.
 */
bool svt_aom_is_pic_used_as_ref(uint32_t hierarchical_levels, uint32_t temporal_layer, uint32_t picture_index,
                                uint32_t referencing_scheme, bool is_overlay) {
    if (is_overlay) {
        return false;
    }

    // Frames below top layer are always used as ref
    if (temporal_layer < hierarchical_levels) {
        return true;
    }

    switch (hierarchical_levels) {
    case 0:
    case 1:
        return true;
    case 2:
        return referencing_scheme == 0 ? false : referencing_scheme == 1 ? true : (picture_index == 0);
    case 3:
        return referencing_scheme == 0 ? false : referencing_scheme == 1 ? true : (picture_index == 0);
    case 4:
        return referencing_scheme == 0 ? false
            : referencing_scheme == 1  ? true
                                       : (picture_index == 0 || picture_index == 8);
    case 5:
        return false;
    default:
        assert(0 && "Invalid hierarchical structure\n");
        break;
    }

    return true;
}

static void set_ref_list_counts(PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    if (pcs->slice_type == I_SLICE) {
        pcs->ref_list0_count = 0;
        pcs->ref_list1_count = 0;
        return;
    }

    Av1RpsNode*           av1_rps   = &pcs->av1_ref_signal;
    const MrpCtrls* const mrp_ctrls = &pcs->scs->mrp_ctrls;
    const bool            is_base   = pcs->temporal_layer_index == 0;
    const bool            is_sc     = pcs->sc_class1;

    // Get list0 count
    uint8_t list0_count   = 1;
    bool    breakout_flag = false;
    // When have duplicate refs in same list or get invalid ref, cap the count
    for (REF_FRAME_MINUS1 i = LAST2; i <= GOLD; i++) {
        if (breakout_flag) {
            break;
        }
        for (REF_FRAME_MINUS1 j = LAST; j < i; j++) {
            /*
            TODO: [PW] Add a check so that if we try accessing a top-layer pic when top-layer pics
            are not allowed (e.g. ref scheme 0) then we breakout.  A check to ensure that the picture
            being referenced is actually the picture intended to be referenced would also be useful
            for debugging.

            For example, if top-layer refs are disallowed and prev. MG is 4L and current MG is 5L
            then pics may try to access layer 3 pics from previous MG.  However, those pics won't
            be from the previous MG as expected, since they would not have been added to the DPB.
            Note that the reference in the DPB will be valid, but the actual picture being referenced
            will be different than expected.
            */
            if (av1_rps->ref_poc_array[i] == av1_rps->ref_poc_array[j]) {
                breakout_flag = true;
                break;
            }
        }
        // if no matching reference were found, increase the count
        if (!breakout_flag) {
            list0_count++;
        }
    }
    pcs->ref_list0_count = MIN(
        list0_count,
        is_sc ? (is_base ? mrp_ctrls->sc_base_ref_list0_count : mrp_ctrls->sc_non_base_ref_list0_count)
              : (is_base ? mrp_ctrls->base_ref_list0_count : mrp_ctrls->non_base_ref_list0_count));
    assert(pcs->ref_list0_count);

    if (svt_aom_is_incomp_mg_frame(pcs) || pcs->is_overlay) {
        pcs->ref_list1_count = 0;
        return;
    }

    // Get list1 count
    uint8_t list1_count = 0;
    breakout_flag       = false;
    // When have duplicate refs in both lists or get invalid ref, cap the count
    for (REF_FRAME_MINUS1 i = BWD; i <= ALT; i++) {
        if (breakout_flag) {
            break;
        }
        // BWD and LAST are allowed to have matching references, as in base layer
        for (REF_FRAME_MINUS1 j = (i == BWD) ? LAST2 : LAST; j < i; j++) {
            if (j <= GOLD && j + 1 > pcs->ref_list0_count) {
                continue;
            }
            // in S-Frame of RA mode, since the ref_list0 will be pruned in set_all_ref_frame_type(),
            // the rest frames in S-Frame miniGop should not remove the duplicated ref_list1, which
            // causes no ref frames in ref_list1, skip the following check only for S-Frame miniGOP
            if (pcs->scs->static_config.pred_structure == RANDOM_ACCESS && pcs->picture_number < ctx->sframe_poc &&
                j <= GOLD && av1_rps->ref_poc_array[j] == ctx->sframe_poc) {
                continue;
            }
            /*
            TODO: [PW] Add a check so that if we try accessing a top-layer pic when top-layer pics
            are not allowed (e.g. ref scheme 0) then we breakout.  A check to ensure that the picture
            being referenced is actually the picture intended to be referenced would also be useful
            for debugging.

            For example, if top-layer refs are disallowed and prev. MG is 4L and current MG is 5L
            then pics may try to access layer 3 pics from previous MG.  However, those pics won't
            be from the previous MG as expected, since they would not have been added to the DPB.
            Note that the reference in the DPB will be valid, but the actual picture being referenced
            will be different than expected.
            */
            if (av1_rps->ref_poc_array[i] == av1_rps->ref_poc_array[j]) {
                breakout_flag = 1;
                break;
            }
        }
        // if no matching reference were found, increase the count
        if (!breakout_flag) {
            list1_count++;
        }
    }
    pcs->ref_list1_count = MIN(
        list1_count,
        is_sc ? (is_base ? mrp_ctrls->sc_base_ref_list1_count : mrp_ctrls->sc_non_base_ref_list1_count)
              : (is_base ? mrp_ctrls->base_ref_list1_count : mrp_ctrls->non_base_ref_list1_count));
    // Old assert fails when M13 uses non-zero mrp
    assert(!(pcs->ref_list1_count == 0 && pcs->scs->static_config.pred_structure == RANDOM_ACCESS));
}

static INLINE void update_ref_poc_array(uint8_t* ref_dpb_idx, uint64_t* ref_poc_array, DpbEntry* dpb) {
    ref_poc_array[LAST]  = dpb[ref_dpb_idx[LAST]].picture_number;
    ref_poc_array[LAST2] = dpb[ref_dpb_idx[LAST2]].picture_number;
    ref_poc_array[LAST3] = dpb[ref_dpb_idx[LAST3]].picture_number;
    ref_poc_array[GOLD]  = dpb[ref_dpb_idx[GOLD]].picture_number;
    ref_poc_array[BWD]   = dpb[ref_dpb_idx[BWD]].picture_number;
    ref_poc_array[ALT2]  = dpb[ref_dpb_idx[ALT2]].picture_number;
    ref_poc_array[ALT]   = dpb[ref_dpb_idx[ALT]].picture_number;
}

static void av1_generate_rps_info(PictureParentControlSet* pcs, EncodeContext* enc_ctx, PictureDecisionContext* ctx,
                                  uint32_t pic_idx, uint32_t mg_idx) {
    Av1RpsNode*         av1_rps             = &pcs->av1_ref_signal;
    FrameHeader*        frm_hdr             = &pcs->frm_hdr;
    SequenceControlSet* scs                 = pcs->scs;
    const uint8_t       hierarchical_levels = pcs->hierarchical_levels;
    const uint8_t       temporal_layer      = pcs->temporal_layer_index;
    const uint8_t       more_5L_refs        = pcs->scs->mrp_ctrls.more_5L_refs;
    if (scs->allintra) {
        pcs->is_ref = false;
    } else {
        pcs->is_ref = scs->use_flat_ipp
            ? true
            : svt_aom_is_pic_used_as_ref(
                  hierarchical_levels, temporal_layer, pic_idx, scs->mrp_ctrls.referencing_scheme, pcs->is_overlay);
    }

    //Set frame type
    if (pcs->slice_type == I_SLICE) {
        frm_hdr->frame_type                    = pcs->idr_flag ? KEY_FRAME : INTRA_ONLY_FRAME;
        pcs->av1_ref_signal.refresh_frame_mask = 0xFF;
#if DEBUG_SFRAME
        fprintf(stderr, "\nFrame %d - key frame\n", (int)pcs->picture_number);
#endif
        if (frm_hdr->frame_type == KEY_FRAME) {
            set_key_frame_rps(pcs, ctx);
            set_ref_list_counts(pcs, ctx);
            if (IS_SFRAME_FLEXIBLE_INSERT(scs->static_config.sframe_mode)) {
                decide_sframe_mg(pcs, enc_ctx, ctx);
            }
            return;
        }
    } else {
        frm_hdr->frame_type = INTER_FRAME;

        // test s-frame on base layer inter frames
        if (enc_ctx->sf_cfg.sframe_dist > 0 || scs->static_config.sframe_posi.sframe_posis) {
            set_sframe_type(pcs, enc_ctx, ctx);
        }
    }

    uint8_t*  ref_dpb_index = av1_rps->ref_dpb_index;
    uint64_t* ref_poc_array = av1_rps->ref_poc_array;

    if (scs->use_flat_ipp) {
        const uint8_t max_refs = scs->mrp_ctrls.flat_max_refs;
        assert(max_refs <= 4);
        uint8_t lay0_toggle = ctx->lay0_toggle;

        // Use up to 4 previous frames as refs
        const uint8_t pic0_idx = QUEUE_GET_PREVIOUS_SPOT(lay0_toggle, max_refs); // newest pic
        const uint8_t pic1_idx = QUEUE_GET_PREVIOUS_SPOT(pic0_idx, max_refs);
        const uint8_t pic2_idx = QUEUE_GET_PREVIOUS_SPOT(pic1_idx, max_refs);
        const uint8_t pic3_idx = QUEUE_GET_PREVIOUS_SPOT(pic2_idx, max_refs);

        // Only use the previous frames as ref
        ref_dpb_index[LAST]  = pic0_idx;
        ref_dpb_index[LAST2] = pic1_idx;
        ref_dpb_index[LAST3] = pic2_idx;
        ref_dpb_index[GOLD]  = pic3_idx;

        ref_dpb_index[BWD]  = ref_dpb_index[LAST];
        ref_dpb_index[ALT2] = ref_dpb_index[LAST];
        ref_dpb_index[ALT]  = ref_dpb_index[LAST];

        // Only max_refs DPB entries should be used, so fill in remaining entries to remove old pics (free up ref buffers)
        av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle | (0xf0);
        for (int i = 3; i >= max_refs; i--) {
            av1_rps->refresh_frame_mask |= 1 << i;
        }
        //Layer0 toggle 0->1->2->3
        ctx->lay0_toggle = circ_inc(max_refs, 1, ctx->lay0_toggle);

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);
        set_ref_list_counts(pcs, ctx);
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);
        set_frame_display_params(pcs, ctx, mg_idx);
    } else if (scs->static_config.pred_structure == LOW_DELAY &&
               scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CQP_OR_CRF) {
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle; // lay1 toggle is for all non-base pics in LD

        // For LD, the prediction structure is generally the previous 3 non-base frames + the previous 3 base frames + 1 long-term ref
        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = scs->mrp_ctrls.ld_reduce_ref_buffs ? 0
            : lay0_toggle == 0                                       ? 2
            : lay0_toggle == 1                                       ? 0
                                                                     : 1; //the newest L0 picture in the DPB

        const uint8_t  lay1_offset   = scs->mrp_ctrls.ld_reduce_ref_buffs == 0 ? LAY1_OFF : 1;
        const uint8_t  lay1_0_idx    = lay1_toggle == 0 ? lay1_offset + 0
                : lay1_toggle == 1                      ? lay1_offset + 1
                                                        : lay1_offset + 2; //the oldest L1/2 picture in the DPB
        const uint8_t  lay1_1_idx    = lay1_toggle == 0 ? lay1_offset + 1
                : lay1_toggle == 1                      ? lay1_offset + 2
                                                        : lay1_offset + 0; //the middle L1/2 picture in the DPB
        const uint8_t  lay1_2_idx    = scs->mrp_ctrls.ld_reduce_ref_buffs == 2 ? 1
                : lay1_toggle == 0                                             ? lay1_offset + 2
                : lay1_toggle == 1                                             ? lay1_offset + 0
                                   : lay1_offset + 1; //the newest L1/2 picture in the DPB
        const uint8_t  long_base_idx = 7;
        const uint16_t long_base_pic = 128;

        assert(!pcs->is_overlay && "overlays not supported in LD");

        const MrpCtrls* const mrp_ctrls       = &pcs->scs->mrp_ctrls;
        const bool            is_base         = temporal_layer == 0;
        const bool            is_sc           = pcs->sc_class1;
        uint8_t               ref_list1_count = is_sc
                          ? (is_base ? mrp_ctrls->sc_base_ref_list1_count : mrp_ctrls->sc_non_base_ref_list1_count)
                          : (is_base ? mrp_ctrls->base_ref_list1_count : mrp_ctrls->non_base_ref_list1_count);

        const uint8_t lay1_pic_idx = (hierarchical_levels == 0) ? 0 : ((1 << (hierarchical_levels - 1)) - 1);
        // When list1 is not used, the pics after the layer 1 pic should use the layer 1 pic as ref instead of previous base
        ref_dpb_index[LAST]  = (pic_idx > lay1_pic_idx) && !is_base && ref_list1_count == 0 ? lay1_2_idx : base2_idx;
        ref_dpb_index[LAST2] = lay1_1_idx;
        ref_dpb_index[LAST3] = long_base_idx;
        ref_dpb_index[GOLD]  = base0_idx;

        ref_dpb_index[BWD]  = lay1_2_idx;
        ref_dpb_index[ALT2] = lay1_0_idx;
        ref_dpb_index[ALT]  = base1_idx;

        if (temporal_layer == 0) {
            if (scs->mrp_ctrls.ld_reduce_ref_buffs == 2) {
                // Only 2 DPB entries should be used, so fill in remaining entries to remove old pics (free up ref buffers)
                av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle | (0xfc);
            } else if (scs->mrp_ctrls.ld_reduce_ref_buffs == 1) {
                // Only 4 DPB entries should be used, so fill in remaining entries to remove old pics (free up ref buffers)
                av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle | (0xf0);
            } else {
                av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
                //Layer0 toggle 0->1->2
                ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            }
        } else {
            if (pcs->is_ref) {
                if (scs->mrp_ctrls.ld_reduce_ref_buffs == 2) {
                    av1_rps->refresh_frame_mask = 1 << 1;
                } else {
                    av1_rps->refresh_frame_mask = 1 << (lay1_offset + ctx->lay1_toggle);
                    //Layer1 toggle 0->1->2
                    ctx->lay1_toggle = circ_inc(3, 1, ctx->lay1_toggle);
                }
            } else {
                av1_rps->refresh_frame_mask = 0;
            }
        }

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        // to make sure the long base reference is in base layer
        if (/*scs->static_config.pred_structure == LOW_DELAY &&*/ (pcs->picture_number - ctx->last_long_base_pic) >=
                long_base_pic &&
            pcs->temporal_layer_index == 0) {
            av1_rps->refresh_frame_mask |= (1 << long_base_idx);
            ctx->last_long_base_pic = pcs->picture_number;
        }
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);
        set_frame_display_params(pcs, ctx, mg_idx);
        //frm_hdr->show_frame = true;
        //pcs->has_show_existing = false;
    } else if (scs->static_config.pred_structure == LOW_DELAY &&
               scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR) {
        assert(!pcs->is_overlay && "overlays not supported in LD");
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle;

        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = scs->mrp_ctrls.ld_reduce_ref_buffs == 2 ? 0
            : lay0_toggle == 0                                            ? 2
            : lay0_toggle == 1                                            ? 0
                                                                          : 1; //the newest L0 picture in the DPB

        const uint8_t  lay1_0_idx = lay1_toggle == 0 ? LAY1_OFF + 0 : LAY1_OFF + 1; //the oldest L1 picture in the DPB
        const uint8_t  lay1_1_idx = scs->mrp_ctrls.ld_reduce_ref_buffs == 2 ? 1
             : scs->mrp_ctrls.ld_reduce_ref_buffs == 1                      ? LAY1_OFF
             : lay1_toggle == 0                                             ? LAY1_OFF + 1
                                : LAY1_OFF + 0; //the newest L1 picture in the DPB
        const uint8_t  lay2_idx   = LAY2_OFF; //the newest L2 picture in the DPB
        const uint8_t  long_base_idx = 7;
        const uint16_t long_base_pic = 128;

        switch (temporal_layer) {
        case 0:
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = base0_idx;
            ref_dpb_index[LAST3] = long_base_idx;
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = ref_dpb_index[LAST];
            ref_dpb_index[ALT2] = ref_dpb_index[LAST];
            ref_dpb_index[ALT]  = ref_dpb_index[LAST];

            if (scs->mrp_ctrls.ld_reduce_ref_buffs == 2) {
                // Only 2 DPB entries should be used, so fill in remaining entries to remove old pics (free up ref buffers)
                av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle | (0xfc);
            } else if (scs->mrp_ctrls.ld_reduce_ref_buffs == 1) {
                // Only 5 DPB entries should be used, so fill in remaining entries to remove old pics (free up ref buffers)
                av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle | (0xf0);
                //Layer0 toggle 0->1->2
                ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            } else {
                av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
                //Layer0 toggle 0->1->2
                ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            }
            break;

        case 1: // Phoenix
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = lay1_1_idx;
            ref_dpb_index[LAST3] = base1_idx;
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = ref_dpb_index[LAST];
            ref_dpb_index[ALT2] = ref_dpb_index[LAST];
            ref_dpb_index[ALT]  = ref_dpb_index[LAST];

            if (scs->mrp_ctrls.ld_reduce_ref_buffs == 2) {
                av1_rps->refresh_frame_mask = 1 << 1;
            } else if (scs->mrp_ctrls.ld_reduce_ref_buffs == 1) {
                av1_rps->refresh_frame_mask = 1 << LAY1_OFF;
            } else {
                av1_rps->refresh_frame_mask = 1 << (LAY1_OFF + ctx->lay1_toggle);
                // Layer1 toggle 3->4
                ctx->lay1_toggle = 1 - ctx->lay1_toggle;
            }
            break;

        case 2:
            if (pic_idx == 0) {
                ref_dpb_index[LAST]  = base2_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = ref_dpb_index[LAST];
                ref_dpb_index[ALT2] = ref_dpb_index[LAST];
                ref_dpb_index[ALT]  = ref_dpb_index[LAST];
            } else if (pic_idx == 2) {
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = base2_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = ref_dpb_index[LAST];
                ref_dpb_index[ALT2] = ref_dpb_index[LAST];
                ref_dpb_index[ALT]  = ref_dpb_index[LAST];
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            assert(IMPLIES(scs->mrp_ctrls.ld_reduce_ref_buffs, !pcs->is_ref && scs->mrp_ctrls.referencing_scheme == 0));
            av1_rps->refresh_frame_mask = (pcs->is_ref) ? 1 << (lay2_idx) : 0;
            // This check should be redundant, but is added to avoid hangs if settings are not set correctly
            if (scs->mrp_ctrls.ld_reduce_ref_buffs) {
                av1_rps->refresh_frame_mask = 0;
            }
            break;
        default:
            SVT_ERROR("unexpected picture mini Gop number\n");
            break;
        }

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        // to make sure the long base reference is in base layer
        if (scs->static_config.pred_structure == LOW_DELAY &&
            (pcs->picture_number - ctx->last_long_base_pic) >= long_base_pic && pcs->temporal_layer_index == 0) {
            av1_rps->refresh_frame_mask |= (1 << long_base_idx);
            ctx->last_long_base_pic = pcs->picture_number;
        }
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);
        set_frame_display_params(pcs, ctx, mg_idx);
    } else if (hierarchical_levels == 0) {
        const uint8_t base0_idx = (ctx->lay0_toggle + 8 - 1) % 8; // the newest L0 picture in the DPB
        const uint8_t base1_idx = (ctx->lay0_toggle + 8 - 2) % 8; // the 2nd-newest L0 picture in the DPB
        const uint8_t base2_idx = (ctx->lay0_toggle + 8 - 3) % 8; // the 3rd-newest L0 picture in the DPB
        const uint8_t base3_idx = (ctx->lay0_toggle + 8 - 4) % 8; // the 4th-newest L0 picture in the DPB
        const uint8_t base4_idx = (ctx->lay0_toggle + 8 - 5) % 8; // the 5th-newest L0 picture in the DPB
        const uint8_t base5_idx = (ctx->lay0_toggle + 8 - 6) % 8; // the 6th-newest L0 picture in the DPB
        const uint8_t base7_idx = (ctx->lay0_toggle + 8 - 7) % 8; // the oldest L0 picture in the DPB

        // {1, 3, 5, 7},   // GOP Index 0 - Ref List 0
        // { 2, 4, 6, 0 }  // GOP Index 0 - Ref List 1
        ref_dpb_index[LAST]  = base0_idx;
        ref_dpb_index[LAST2] = base2_idx;
        ref_dpb_index[LAST3] = base4_idx;
        ref_dpb_index[GOLD]  = base7_idx;

        ref_dpb_index[BWD]  = base1_idx;
        ref_dpb_index[ALT2] = base3_idx;
        ref_dpb_index[ALT]  = base5_idx;

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);

        av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;

        // Flat mode, output all frames
        set_frame_display_params(pcs, ctx, mg_idx);
        frm_hdr->show_frame    = true;
        pcs->has_show_existing = false;
        ctx->lay0_toggle       = (1 + ctx->lay0_toggle) % 8;
    } else if (hierarchical_levels == 1) {
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle;
        /* The default toggling assumes that the toggle is updated in decode order for an RA configuration.
        For low-delay configurations, the decode order is the display order, so instead of having the base
        toggle updated before all other pictures, it is now updated last.  Hence, we need to adjust the toggle
        for low-delay configurations to ensure that all indices will still correspond to the proper reference
        (i.e. newest base, middle base, oldest base, etc.). Lay 1 pics in RA will typically be decoded second
        (right after base) so all higher level pics will assume that layer 1 was toggled before them.  For low-
        delay, the first half of the higher level pics will be before the layer 1 toggle, while the second half
        will come after the toggle.  Hence, the layer 1 toggle only needs to be updated for the first half of
        the pictures. */
        if (pcs->pred_struct_ptr->pred_type != RANDOM_ACCESS && temporal_layer) {
            assert(IMPLIES(scs->static_config.pred_structure == RANDOM_ACCESS, ctx->cut_short_ra_mg));
            lay0_toggle = circ_inc(3, 1, lay0_toggle);
            // No layer 1 toggling needed because there's only one non-base frame
        }

        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = lay0_toggle == 0 ? 2 : lay0_toggle == 1 ? 0 : 1; //the newest L0 picture in the DPB

        //const uint8_t  lay1_0_idx = lay1_toggle == 0 ? LAY1_OFF + 0 : LAY1_OFF + 1; //the oldest L1 picture in the DPB
        const uint8_t lay1_1_idx = lay1_toggle == 0 ? LAY1_OFF + 1 : LAY1_OFF + 0; //the newest L1 picture in the DPB
        //const uint8_t  lay2_idx = LAY2_OFF; //the newest L2 picture in the DPB

        switch (temporal_layer) {
        case 0:
            //{ 2, 6, 0, 0},  // GOP Index 0 - Ref List 0
            //{ 2, 4, 0, 0 } // GOP Index 0 - Ref List 1
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = base0_idx;
            ref_dpb_index[LAST3] = ref_dpb_index[LAST];
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = base1_idx;
            ref_dpb_index[ALT]  = ref_dpb_index[BWD];

            av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
            //Layer0 toggle 0->1->2
            ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            break;
        case 1:
            if (pcs->is_overlay) {
                // update RPS for the overlay frame.
                //{ 0, 0, 0, 0}         // GOP Index 1 - Ref List 0
                //{ 0, 0, 0, 0 }       // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base2_idx;
                ref_dpb_index[LAST2] = base2_idx;
                ref_dpb_index[LAST3] = base2_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base2_idx;
                assert(!pcs->is_ref);
            } else {
                //{ 1, 2, 3,  0},   // GOP Index 4 - Ref List 0
                //{-1,  0, 0,  0}     // GOP Index 4 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = base0_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = ref_dpb_index[BWD];
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];

                //Layer1 toggle 3->4
                ctx->lay1_toggle = 1 - ctx->lay1_toggle;
            }
            av1_rps->refresh_frame_mask = pcs->is_ref ? 1 << (LAY1_OFF + ctx->lay1_toggle) : 0;
            break;
        default:
            SVT_ERROR("unexpected picture mini Gop number\n");
            break;
        }
        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);

        if (!set_frame_display_params(pcs, ctx, mg_idx)) {
            if (temporal_layer < hierarchical_levels) {
                frm_hdr->show_frame    = false;
                pcs->has_show_existing = false;
            } else {
                frm_hdr->show_frame    = true;
                pcs->has_show_existing = true;

                if (pic_idx == 0) {
                    frm_hdr->show_existing_frame = base2_idx;
                } else {
                    SVT_LOG("Error in GOP indexing for hierarchical level %d\n", pcs->hierarchical_levels);
                }
            }
        }
    } else if (hierarchical_levels == 2) {
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle;
        /* The default toggling assumes that the toggle is updated in decode order for an RA configuration.
        For low-delay configurations, the decode order is the display order, so instead of having the base
        toggle updated before all other pictures, it is now updated last.  Hence, we need to adjust the toggle
        for low-delay configurations to ensure that all indices will still correspond to the proper reference
        (i.e. newest base, middle base, oldest base, etc.). Lay 1 pics in RA will typically be decoded second
        (right after base) so all higher level pics will assume that layer 1 was toggled before them.  For low-
        delay, the first half of the higher level pics will be before the layer 1 toggle, while the second half
        will come after the toggle.  Hence, the layer 1 toggle only needs to be updated for the first half of
        the pictures. */
        if (pcs->pred_struct_ptr->pred_type != RANDOM_ACCESS && temporal_layer) {
            assert(IMPLIES(scs->static_config.pred_structure == RANDOM_ACCESS, ctx->cut_short_ra_mg));
            lay0_toggle = circ_inc(3, 1, lay0_toggle);
            if (pic_idx == 0) {
                lay1_toggle = 1 - lay1_toggle;
            }
        }

        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = lay0_toggle == 0 ? 2 : lay0_toggle == 1 ? 0 : 1; //the newest L0 picture in the DPB

        const uint8_t  lay1_0_idx = lay1_toggle == 0 ? LAY1_OFF + 0 : LAY1_OFF + 1; //the oldest L1 picture in the DPB
        const uint8_t  lay1_1_idx = lay1_toggle == 0 ? LAY1_OFF + 1 : LAY1_OFF + 0; //the newest L1 picture in the DPB
        const uint8_t  lay2_idx   = LAY2_OFF; //the newest L2 picture in the DPB
        const uint8_t  long_base_idx = 7;
        const uint16_t long_base_pic = 128;

        switch (temporal_layer) {
        case 0:
            //{4, 12, 0, 0}, // GOP Index 0 - Ref List 0
            //{ 4, 8, 0, 0 } // GOP Index 0 - Ref List 1
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = base0_idx;
            if (scs->static_config.pred_structure == LOW_DELAY) {
                ref_dpb_index[LAST3] = long_base_idx;
            } else {
                ref_dpb_index[LAST3] = ref_dpb_index[LAST];
            }
            ref_dpb_index[GOLD] = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = base1_idx;
            ref_dpb_index[ALT]  = ref_dpb_index[BWD];

            av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
            //Layer0 toggle 0->1->2
            ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            break;

        case 1: // Phoenix
            //{ 2, 4, 6, 0}   // GOP Index 2 - Ref List 0
            //{-2, 0, 0, 0}   // GOP Index 2 - Ref List 1
            ref_dpb_index[LAST]  = base1_idx;
            ref_dpb_index[LAST2] = lay1_1_idx;
            ref_dpb_index[LAST3] = base0_idx;
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = ref_dpb_index[BWD];
            ref_dpb_index[ALT]  = ref_dpb_index[BWD];

            av1_rps->refresh_frame_mask = 1 << (LAY1_OFF + ctx->lay1_toggle);
            //Layer1 toggle 3->4
            ctx->lay1_toggle = 1 - ctx->lay1_toggle;
            break;

        case 2:
            if (pcs->is_overlay) {
                // update RPS for the overlay frame.
                //{ 0, 0, 0, 0}         // GOP Index 1 - Ref List 0
                //{ 0, 0, 0, 0 }       // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base2_idx;
                ref_dpb_index[LAST2] = base2_idx;
                ref_dpb_index[LAST3] = base2_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 0) {
                //{ 1, 3, 5, 0}      // GOP Index 1 - Ref List 0
                //{ -1, -3, 0, 0}    // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay1_0_idx;
                ref_dpb_index[LAST3] = base0_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = lay1_1_idx;
                ref_dpb_index[ALT2] = base2_idx;
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else if (pic_idx == 2) {
                // { 1, 3, 2, 0},     // GOP Index 3 - Ref List 0
                // { -1,0, 0, 0}      // GOP Index 3 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay2_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = ref_dpb_index[BWD];
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = (pcs->is_ref) ? 1 << (lay2_idx) : 0;
            break;
        default:
            SVT_ERROR("unexpected picture mini Gop number\n");
            break;
        }

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        // to make sure the long base reference is in base layer
        if (scs->static_config.pred_structure == LOW_DELAY &&
            (pcs->picture_number - ctx->last_long_base_pic) >= long_base_pic && pcs->temporal_layer_index == 0) {
            av1_rps->refresh_frame_mask |= (1 << long_base_idx);
            ctx->last_long_base_pic = pcs->picture_number;
        }
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);

        if (!set_frame_display_params(pcs, ctx, mg_idx)) {
            if (temporal_layer < hierarchical_levels) {
                frm_hdr->show_frame    = false;
                pcs->has_show_existing = false;
            } else {
                frm_hdr->show_frame    = true;
                pcs->has_show_existing = true;

                if (pic_idx == 0) {
                    frm_hdr->show_existing_frame = lay1_1_idx;
                } else if (pic_idx == 2) {
                    frm_hdr->show_existing_frame = base2_idx;
                } else {
                    SVT_LOG("Error in GOP indexing for hierarchical level %d\n", pcs->hierarchical_levels);
                }
            }
        }
    } else if (hierarchical_levels == 3) {
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle;
        /* The default toggling assumes that the toggle is updated in decode order for an RA configuration.
        For low-delay configurations, the decode order is the display order, so instead of having the base
        toggle updated before all other pictures, it is now updated last.  Hence, we need to adjust the toggle
        for low-delay configurations to ensure that all indices will still correspond to the proper reference
        (i.e. newest base, middle base, oldest base, etc.). Lay 1 pics in RA will typically be decoded second
        (right after base) so all higher level pics will assume that layer 1 was toggled before them.  For low-
        delay, the first half of the higher level pics will be before the layer 1 toggle, while the second half
        will come after the toggle.  Hence, the layer 1 toggle only needs to be updated for the first half of
        the pictures. */
        if (pcs->pred_struct_ptr->pred_type != RANDOM_ACCESS && temporal_layer) {
            assert(IMPLIES(scs->static_config.pred_structure == RANDOM_ACCESS, ctx->cut_short_ra_mg));
            lay0_toggle = circ_inc(3, 1, lay0_toggle);
            if (pic_idx < 3) {
                lay1_toggle = 1 - lay1_toggle;
            }
        }

        //pic_idx has this order:
        //         0     2    4      6
        //            1          5
        //                 3
        //                              7(could be an I)

        //DPB: Loc7|Loc6|Loc5|Loc4|Loc3|Loc2|Loc1|Loc0
        //Layer 0 : circular move 0-1-2
        //Layer 1 : circular move 3-4
        //Layer 2 : circular move 5-6
        //Layer 3 : 7
        //pic_num
        //         1     3    5      7    9     11     13      15
        //            2          6           10            14
        //                 4                        12
        //
        //base0:0                   base1:8                          base2:16
        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = lay0_toggle == 0 ? 2 : lay0_toggle == 1 ? 0 : 1; //the newest L0 picture in the DPB

        const uint8_t lay1_0_idx = lay1_toggle == 0 ? LAY1_OFF + 0 : LAY1_OFF + 1; //the oldest L1 picture in the DPB
        const uint8_t lay1_1_idx = lay1_toggle == 0 ? LAY1_OFF + 1 : LAY1_OFF + 0; //the newest L1 picture in the DPB
        const uint8_t lay2_idx   = LAY2_OFF; //the newest L2 picture in the DPB
        const uint8_t lay3_idx   = LAY3_OFF; //the newest L3 picture in the DPB

        switch (temporal_layer) {
        case 0:
            //{8, 24, 0, 0},  // GOP Index 0 - Ref List 0
            //{ 8, 16, 0, 0 } // GOP Index 0 - Ref List 1
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = base0_idx;
            ref_dpb_index[LAST3] = ref_dpb_index[LAST];
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = base1_idx;
            ref_dpb_index[ALT]  = ref_dpb_index[BWD];

            av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
            //Layer0 toggle 0->1->2
            ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            break;
        case 1:
            //{ 4, 8, 12,  0},   // GOP Index 4 - Ref List 0
            //{-4,  0, 0,  0}     // GOP Index 4 - Ref List 1
            ref_dpb_index[LAST]  = base1_idx;
            ref_dpb_index[LAST2] = lay1_1_idx;
            ref_dpb_index[LAST3] = base0_idx;
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = ref_dpb_index[BWD];
            ref_dpb_index[ALT]  = ref_dpb_index[BWD];

            av1_rps->refresh_frame_mask = 1 << (LAY1_OFF + ctx->lay1_toggle);
            //Layer1 toggle 3->4
            ctx->lay1_toggle = 1 - ctx->lay1_toggle;
            break;
        case 2:
            if (pic_idx == 1) {
                //{  2,  4,  6,  10}    // GOP Index 2 - Ref List 0
                //{ -2, -6,  0,   0}    // GOP Index 2 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base0_idx;

                ref_dpb_index[BWD]  = lay1_1_idx;
                ref_dpb_index[ALT2] = base2_idx;
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else if (pic_idx == 5) {
                //{ 2, 4, 6, 10}    // GOP Index 6 - Ref List 0
                //{ -2,  0, 0,  0 } // GOP Index 6 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = ref_dpb_index[BWD];
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            }

            av1_rps->refresh_frame_mask = 1 << (lay2_idx);
            break;
        case 3:
            if (pcs->is_overlay) {
                // update RPS for the overlay frame.
                //{ 0, 0, 0, 0}        // GOP Index 1 - Ref List 0
                //{ 0, 0, 0, 0 }       // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base2_idx;
                ref_dpb_index[LAST2] = base2_idx;
                ref_dpb_index[LAST3] = base2_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 0) {
                //{1, 5, 8, 0},     // GOP Index 1 - Ref List 0
                //{ -1, -3, -7, 0 } // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay1_0_idx;
                ref_dpb_index[LAST3] = lay3_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = lay2_idx;
                ref_dpb_index[ALT2] = lay1_1_idx;
                ref_dpb_index[ALT]  = base2_idx;
            } else if (pic_idx == 2) {
                //{1, 3, 2, 0}, // GOP Index 3 - Ref List 0
                //{ -1,  -5, 0,  0 } // GOP Index 3 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay3_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = lay1_1_idx;
                ref_dpb_index[ALT2] = base2_idx;
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else if (pic_idx == 4) {
                //{1, 5, 4, 0},    // GOP Index 5 - Ref List 0
                //{ -1, -3, 0, 0 } // GOP Index 5 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay3_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = lay2_idx;
                ref_dpb_index[ALT2] = base2_idx;
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else if (pic_idx == 6) {
                //{1, 3, 6, 0},   // GOP Index 7 - Ref List 0
                //{ -1, 0, 0, 0 } // GOP Index 7 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = lay3_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = ref_dpb_index[BWD];
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = (pcs->is_ref) ? 1 << (lay3_idx) : 0;
            break;

        default:
            SVT_ERROR("unexpected picture mini Gop number\n");
            break;
        }

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);

        if (!set_frame_display_params(pcs, ctx, mg_idx)) {
            if (temporal_layer < hierarchical_levels) {
                frm_hdr->show_frame    = false;
                pcs->has_show_existing = false;
            } else {
                frm_hdr->show_frame    = true;
                pcs->has_show_existing = true;

                if (pic_idx == 0) {
                    frm_hdr->show_existing_frame = lay2_idx;
                } else if (pic_idx == 2) {
                    frm_hdr->show_existing_frame = lay1_1_idx;
                } else if (pic_idx == 4) {
                    frm_hdr->show_existing_frame = lay2_idx;
                } else if (pic_idx == 6) {
                    frm_hdr->show_existing_frame = base2_idx;
                } else {
                    SVT_LOG("Error in GOP indexing for hierarchical level %d\n", pcs->hierarchical_levels);
                }
            }
        }
    } else if (hierarchical_levels == 4) {
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle;
        /* The default toggling assumes that the toggle is updated in decode order for an RA configuration.
        For low-delay configurations, the decode order is the display order, so instead of having the base
        toggle updated before all other pictures, it is now updated last.  Hence, we need to adjust the toggle
        for low-delay configurations to ensure that all indices will still correspond to the proper reference
        (i.e. newest base, middle base, oldest base, etc.). Lay 1 pics in RA will typically be decoded second
        (right after base) so all higher level pics will assume that layer 1 was toggled before them.  For low-
        delay, the first half of the higher level pics will be before the layer 1 toggle, while the second half
        will come after the toggle.  Hence, the layer 1 toggle only needs to be updated for the first half of
        the pictures. */
        if (pcs->pred_struct_ptr->pred_type != RANDOM_ACCESS && temporal_layer) {
            assert(IMPLIES(scs->static_config.pred_structure == RANDOM_ACCESS, ctx->cut_short_ra_mg));
            lay0_toggle = circ_inc(3, 1, lay0_toggle);
            if (pic_idx < 7) {
                lay1_toggle = 1 - lay1_toggle;
            }
        }
        //pic_idx has this order:
        //         0     2    4      6    8     10     12      14
        //            1          5           9            13
        //                 3                        11
        //                              7
        //                                                          15(could be an I)

        //DPB: Loc7|Loc6|Loc5|Loc4|Loc3|Loc2|Loc1|Loc0
        //Layer 0 : circular move 0-1-2
        //Layer 1 : circular move 3-4
        //Layer 2 : DPB Location 5
        //Layer 3 : DPB Location 6
        //Layer 4 : DPB Location 7
        //pic_num                  for poc 17
        //         1     3    5      7    9     11     13      15         17    19     21    23   25     27    29    31
        //            2          6           10            14                18           22          26          30
        //                 4                        12:L2_0                         20:L2_1                 28
        //                              8:L1_0                                                       24:L1_1
        //base0:0                                               base1:16                                           base2:32
        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = lay0_toggle == 0 ? 2 : lay0_toggle == 1 ? 0 : 1; //the newest L0 picture in the DPB

        const uint8_t lay1_0_idx = lay1_toggle == 0 ? LAY1_OFF + 0 : LAY1_OFF + 1; //the oldest L1 picture in the DPB
        const uint8_t lay1_1_idx = lay1_toggle == 0 ? LAY1_OFF + 1 : LAY1_OFF + 0; //the newest L1 picture in the DPB
        const uint8_t lay2_idx   = LAY2_OFF; //the newest L2 picture in the DPB
        const uint8_t lay3_idx   = LAY3_OFF; //the newest L3 picture in the DPB
        const uint8_t lay4_idx   = LAY4_OFF; //the newest L4 picture in the DPB

        switch (temporal_layer) {
        case 0:

            //{16, 48, 0, 0},      // GOP Index 0 - Ref List 0
            //{16, 32, 0, 0}       // GOP Index 0 - Ref List 1
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = base0_idx;
            ref_dpb_index[LAST3] = more_5L_refs ? lay1_1_idx : ref_dpb_index[LAST]; //48:p24
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = base1_idx;
            ref_dpb_index[ALT]  = more_5L_refs ? lay1_0_idx : ref_dpb_index[BWD]; //48:p8

            av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
            //Layer0 toggle 0->1->2
            ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            break;

        case 1:
            //{  8, 16, 24, 0},   // GOP Index 8 - Ref List 0
            //{ -8, 0, 0, 0}      // GOP Index 8 - Ref List 1
            ref_dpb_index[LAST]  = base1_idx;
            ref_dpb_index[LAST2] = lay1_1_idx;
            ref_dpb_index[LAST3] = base0_idx;
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = lay2_idx;
            ref_dpb_index[ALT]  = ref_dpb_index[BWD]; //40:-30

            av1_rps->refresh_frame_mask = 1 << (LAY1_OFF + ctx->lay1_toggle);
            //Layer1 toggle 3->4
            ctx->lay1_toggle = 1 - ctx->lay1_toggle;
            break;

        case 2:
            if (pic_idx == 3) {
                //{  4,   8,  12,  20 },  // GOP Index 4 - Ref List 0
                //{ -4, -12,  0,  0 }     // GOP Index 4 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base0_idx;

                ref_dpb_index[BWD]  = lay1_1_idx;
                ref_dpb_index[ALT2] = base2_idx;
                ref_dpb_index[ALT]  = more_5L_refs ? lay3_idx : ref_dpb_index[BWD]; //36:+30
            } else if (pic_idx == 11) {
                //{ 4, 8, 12, 0},       // GOP Index 12 - Ref List 0
                //{ -4,  0, 0,  0 }     // GOP Index 12 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay3_idx;

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = lay4_idx;
                ref_dpb_index[ALT]  = more_5L_refs ? lay1_0_idx : ref_dpb_index[BWD]; //44:+24
            }

            av1_rps->refresh_frame_mask = 1 << (LAY2_OFF);
            break;

        case 3:
            if (pic_idx == 1) {
                //{ 2, 4, 10, 18},        // GOP Index 2 - Ref List 0
                //{ -2, -6, -14,  0 }   // GOP Index 2 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base0_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 5) {
                //{ 2, 4, 6, 14},        // GOP Index 6 - Ref List 0
                //{ -2, -10,  0,  0 }   // GOP Index 6 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay1_1_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = more_5L_refs ? lay4_idx : ref_dpb_index[BWD]; // 38:p35
            } else if (pic_idx == 9) {
                //{ 2, 4, 10, 18},       // GOP Index 10 - Ref List 0
                //{ -2, -6,  0,  0 }    // GOP Index 10 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 13) {
                //{ 2, 4, 6, 14},    // GOP Index 14 - Ref List 0
                //{ -2, 0,  0, 0 }   // GOP Index 14 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = lay4_idx;
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = 1 << (lay3_idx);
            break;

        case 4:
            if (pcs->is_overlay) {
                // update RPS for the overlay frame.
                //{ 0, 0, 0, 0}  // GOP Index 1 - Ref List 0
                //{ 0, 0, 0, 0 } // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base2_idx;
                ref_dpb_index[LAST2] = base2_idx;
                ref_dpb_index[LAST3] = base2_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 0) {
                //{ 1, 9, 8, 17},   // GOP Index 1 - Ref List 0
                //{ -1, -3, -7, 0 } // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay1_0_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = base0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = lay1_1_idx;
            } else if (pic_idx == 2) {
                //{ 1, 3, 2, 11},  // GOP Index 3 - Ref List 0
                //{ -1, -5, -13, 0 }   // GOP Index 3 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 4) {
                //{ 1, 5, 4, 13},  // GOP Index 5 - Ref List 0
                //{ -1, -3, -11, 0 }   // GOP Index 5 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 6) {
                //{ 1, 3, 6, 7},  // GOP Index 7 - Ref List 0
                //{ -1, -9, 0, 0 }   // GOP Index 7 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay1_1_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = more_5L_refs ? lay1_0_idx : ref_dpb_index[BWD]; //39:p24
            } else if (pic_idx == 8) {
                //{ 1, 9, 8, 17},  // GOP Index 9 - Ref List 0
                //{ -1, -3, -7, 0 }   // GOP Index 9 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 10) {
                //{ 1, 3, 2, 11},  // GOP Index 11 - Ref List 0
                //{ -1, -5, 0, 0 }   // GOP Index 11 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 12) {
                //{ 1, 5, 4, 13},  // GOP Index 13 - Ref List 0
                //{ -1, -3, 0, 0 }   // GOP Index 13 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 14) {
                //{ 1, 3, 6, 7},  // GOP Index 15 - Ref List 0
                //{ -1, 0, 0, 0 }   // GOP Index 15 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay4_idx;
                ref_dpb_index[GOLD]  = lay1_1_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base1_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = (pcs->is_ref) ? 1 << (lay4_idx) : 0;
            break;

        default:
            SVT_ERROR("unexpected picture mini Gop number\n");
            break;
        }

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);

        if (!set_frame_display_params(pcs, ctx, mg_idx)) {
            if (temporal_layer < hierarchical_levels) {
                frm_hdr->show_frame    = false;
                pcs->has_show_existing = false;
            } else {
                frm_hdr->show_frame    = true;
                pcs->has_show_existing = true;

                if (pic_idx == 0) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 2) {
                    frm_hdr->show_existing_frame = lay2_idx;
                } else if (pic_idx == 4) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 6) {
                    frm_hdr->show_existing_frame = lay1_1_idx;
                } else if (pic_idx == 8) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 10) {
                    frm_hdr->show_existing_frame = lay2_idx;
                } else if (pic_idx == 12) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 14) {
                    frm_hdr->show_existing_frame = base2_idx;
                } else {
                    SVT_LOG("Error in GOP indexing for hierarchical level %d\n", pcs->hierarchical_levels);
                }
            }
        }
    } else if (hierarchical_levels == 5) {
        uint8_t lay0_toggle = ctx->lay0_toggle;
        uint8_t lay1_toggle = ctx->lay1_toggle;
        /* The default toggling assumes that the toggle is updated in decode order for an RA configuration.
        For low-delay configurations, the decode order is the display order, so instead of having the base
        toggle updated before all other pictures, it is now updated last.  Hence, we need to adjust the toggle
        for low-delay configurations to ensure that all indices will still correspond to the proper reference
        (i.e. newest base, middle base, oldest base, etc.). Lay 1 pics in RA will typically be decoded second
        (right after base) so all higher level pics will assume that layer 1 was toggled before them.  For low-
        delay, the first half of the higher level pics will be before the layer 1 toggle, while the second half
        will come after the toggle.  Hence, the layer 1 toggle only needs to be updated for the first half of
        the pictures. */
        if (pcs->pred_struct_ptr->pred_type != RANDOM_ACCESS && temporal_layer) {
            assert(IMPLIES(scs->static_config.pred_structure == RANDOM_ACCESS, ctx->cut_short_ra_mg));
            lay0_toggle = circ_inc(3, 1, lay0_toggle);
            if (pic_idx < 15) {
                lay1_toggle = 1 - lay1_toggle;
            }
        }

        //DPB: Loc7|Loc6|Loc5|Loc4|Loc3|Loc2|Loc1|Loc0
        //Layer 0 : circular move 0-1-2
        //Layer 1 : circular move 3-4
        //Layer 2 : DPB Location 5
        //Layer 3 : DPB Location 6
        //Layer 4 : DPB Location 7
        const uint8_t base0_idx = lay0_toggle == 0 ? 0 : lay0_toggle == 1 ? 1 : 2; //the oldest L0 picture in the DPB
        const uint8_t base1_idx = lay0_toggle == 0 ? 1 : lay0_toggle == 1 ? 2 : 0; //the middle L0 picture in the DPB
        const uint8_t base2_idx = lay0_toggle == 0 ? 2 : lay0_toggle == 1 ? 0 : 1; //the newest L0 picture in the DPB

        const uint8_t lay1_0_idx = lay1_toggle == 0 ? LAY1_OFF + 0 : LAY1_OFF + 1; //the oldest L1 picture in the DPB
        const uint8_t lay1_1_idx = lay1_toggle == 0 ? LAY1_OFF + 1 : LAY1_OFF + 0; //the newest L1 picture in the DPB
        const uint8_t lay2_idx   = LAY2_OFF; //the newest L2 picture in the DPB
        const uint8_t lay3_idx   = LAY3_OFF; //the newest L3 picture in the DPB
        const uint8_t lay4_idx   = LAY4_OFF; //the newest L4 picture in the DPB

        switch (temporal_layer) {
        case 0:
            //{32, 64, 96, 0}, // GOP Index 0 - Ref List 0
            //{ 32, 48, 0, 0 } // GOP Index 0 - Ref List 1
            ref_dpb_index[LAST]  = base2_idx;
            ref_dpb_index[LAST2] = base1_idx;
            ref_dpb_index[LAST3] = base0_idx;
            ref_dpb_index[GOLD]  = ref_dpb_index[LAST];
            ref_dpb_index[BWD]   = base2_idx;
            ref_dpb_index[ALT2]  = lay1_1_idx;
            ref_dpb_index[ALT]   = ref_dpb_index[BWD];

            av1_rps->refresh_frame_mask = 1 << ctx->lay0_toggle;
            //Layer0 toggle 0->1->2
            ctx->lay0_toggle = circ_inc(3, 1, ctx->lay0_toggle);
            break;

        case 1:
            //{16, 32, 48, 64}, // GOP Index 16 - Ref List 0
            //{-16, 24, 20, 0} // GOP Index 16 - Ref List 1
            ref_dpb_index[LAST]  = base1_idx;
            ref_dpb_index[LAST2] = lay1_1_idx;
            ref_dpb_index[LAST3] = base0_idx;
            ref_dpb_index[GOLD]  = lay1_0_idx;

            ref_dpb_index[BWD]  = base2_idx;
            ref_dpb_index[ALT2] = lay2_idx;
            ref_dpb_index[ALT]  = lay3_idx;

            av1_rps->refresh_frame_mask = 1 << (LAY1_OFF + ctx->lay1_toggle);
            //Layer1 toggle 2->3
            ctx->lay1_toggle = 1 - ctx->lay1_toggle;
            break;
        case 2:
            if (pic_idx == 7) {
                //{8, 16, 24, 0}, // GOP Index 8 - Ref List 0
                //{-8, -24, 12, 0 } // GOP Index 8 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = lay1_1_idx;
                ref_dpb_index[ALT2] = base2_idx;
                ref_dpb_index[ALT]  = lay3_idx;
            } else if (pic_idx == 23) {
                //{8, 16, 24, 0}    // GOP Index 24 - Ref List 0
                //{-8, 10, 40, 0} // GOP Index 24 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = lay4_idx;
                ref_dpb_index[ALT]  = lay1_0_idx;
            }

            av1_rps->refresh_frame_mask = 1 << (LAY2_OFF);
            break;

        case 3:
            if (pic_idx == 3) {
                //{4, 8, 20, 36}, // GOP Index 4 - Ref List 0
                //{-4, -12, -28, 0} // GOP Index 4 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base0_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 11) {
                //{1, 3, 11, 27}, // GOP Index 12 - Ref List 0
                //{-4, -20, 5, 0} // GOP Index 12 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay1_1_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 19) {
                //{4, 8, 20, 0}, // GOP Index 20 - Ref List 0
                //{-4, -12, 0, 0} // GOP Index 20 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 27) {
                //{4, 8, 12, 28}, // GOP Index 28 - Ref List 0
                //{-4, 60, 0, 0} // GOP Index 28 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;

                ref_dpb_index[BWD]  = base2_idx;
                ref_dpb_index[ALT2] = base0_idx;
                ref_dpb_index[ALT]  = ref_dpb_index[BWD];
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = 1 << (LAY3_OFF);
            break;

        case 4:
            if (pic_idx == 1) {
                //{2, 4, 18, -30}, // GOP Index 2 - Ref List 0
                //{-2, -6, -14, 0} // GOP Index 2 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = lay1_1_idx;
            } else if (pic_idx == 5) {
                //{2, 4, 6, 22}, // GOP Index 6 - Ref List 0
                //{-2, -10, -26, 0} // GOP Index 6 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 9) {
                //{2, 4, 10, 26}, // GOP Index 10 - Ref List 0
                //{-2, -6, -22, 0} // GOP Index 10 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 13) {
                //{2, 4, 6, 14}, // GOP Index 14 - Ref List 0
                //{-2, -18, 0, 0} // GOP Index 14 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = lay2_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay1_1_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 17) {
                //{2, 4, 18,  34}, // GOP Index 18 - Ref List 0
                //{-2, -6, -14, 0} // GOP Index 18 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 21) {
                //{2, 4, 6, 22}, // GOP Index 22 - Ref List 0
                //{-2, -10, 0, 0} // GOP Index 22 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 25) {
                //{2, 4, 10, 26}, // GOP Index 26 - Ref List 0
                //{-2, -6, 0, 0} // GOP Index 26 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = ref_dpb_index[BWD];
            } else if (pic_idx == 29) {
                //{2, 4, 6, 14}, // GOP Index 30 - Ref List 0
                //{-2, 30, 62, 0} // GOP Index 30 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay4_idx;
                ref_dpb_index[LAST3] = lay2_idx;
                ref_dpb_index[GOLD]  = lay1_1_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base1_idx;
                ref_dpb_index[ALT]   = base0_idx;
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = 1 << (LAY4_OFF);
            break;

        case 5:
            if (pcs->is_overlay) {
                // update RPS for the overlay frame.
                //{ 0, 0, 0, 0}  // GOP Index 1 - Ref List 0
                //{ 0, 0, 0, 0 } // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base2_idx;
                ref_dpb_index[LAST2] = base2_idx;
                ref_dpb_index[LAST3] = base2_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 0) {
                //{1, 17, -15, -31}, // GOP Index 1 - Ref List 0
                //{-1, -3, -7, 0} // GOP Index 1 - Ref List 1
                ref_dpb_index[LAST]  = base1_idx;
                ref_dpb_index[LAST2] = lay1_0_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay3_idx;
                ref_dpb_index[ALT]   = lay2_idx;
            } else if (pic_idx == 2) {
                //{1, 3, 19, -29}, // GOP Index 3 - Ref List 0
                //{-1, -5, -13, 0} // GOP Index 3 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = lay1_1_idx;
            } else if (pic_idx == 4) {
                //{1, 5, 21, 0}, // GOP Index 5 - Ref List 0
                //{-1, -3, -11, 0} // GOP Index 5 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = lay1_1_idx;
            } else if (pic_idx == 6) {
                //{1, 3, 7, 0}, // GOP Index 7 - Ref List 0
                //{-1, -9, -25, 0} // GOP Index 7 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 8) {
                //{1, 9, 25, 0}, // GOP Index 9 - Ref List 0
                //{-1, -3, -7, 0} // GOP Index 9 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = ref_dpb_index[LAST];
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay3_idx;
                ref_dpb_index[ALT]   = lay1_1_idx;
            } else if (pic_idx == 10) {
                //{1, 3, 11, 27}, // GOP Index 11 - Ref List 0
                //{-1, -5, -21, 0} // GOP Index 11 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 12) {
                //{1, 5, 13, 29}, // GOP Index 13 - Ref List 0
                //{-1, -3, -19, 0} // GOP Index 13 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 14) {
                //{1, 3, 7, 31}, // GOP Index 15 - Ref List 0
                //{ -1, -17, 15, 0 } // GOP Index 15 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay2_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay1_1_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base1_idx;
            } else if (pic_idx == 16) {
                //{1, 17, 33, -15}, // GOP Index 17 - Ref List 0
                //{-1, -3, -7, 0} // GOP Index 17 - Ref List 1
                ref_dpb_index[LAST]  = lay1_1_idx;
                ref_dpb_index[LAST2] = base1_idx;
                ref_dpb_index[LAST3] = lay1_0_idx;
                ref_dpb_index[GOLD]  = base2_idx;
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay3_idx;
                ref_dpb_index[ALT]   = lay2_idx;
            } else if (pic_idx == 18) {
                //{1, 3, 19, 35}, // GOP Index 19 - Ref List 0
                //{-1, -5, -13, 0} // GOP Index 19 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 20) {
                //{1, 5, 21, 37}, // GOP Index 21 - Ref List 0
                //{-1, -3, -11, 0} // GOP Index 21 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay2_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 22) {
                //{1, 3, 7, 23}, // GOP Index 23 - Ref List 0
                //{-1, -9, 55, 0} // GOP Index 23 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay2_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base0_idx;
            } else if (pic_idx == 24) {
                //{1, 9, 25, 41}, // GOP Index 25 - Ref List 0
                //{-1, -3, -7, 0} // GOP Index 25 - Ref List 1
                ref_dpb_index[LAST]  = lay2_idx;
                ref_dpb_index[LAST2] = lay1_1_idx;
                ref_dpb_index[LAST3] = base1_idx;
                ref_dpb_index[GOLD]  = lay1_0_idx;
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = lay3_idx;
                ref_dpb_index[ALT]   = base2_idx;
            } else if (pic_idx == 26) {
                //{1, 3, 11, 27}, // GOP Index 27 - Ref List 0
                //{-1, -5, 59, 0} // GOP Index 27 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay3_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base0_idx;
            } else if (pic_idx == 28) {
                //{1, 5, 13, 29}, // GOP Index 29 - Ref List 0
                //{-1, -3, 61, 0} // GOP Index 29 - Ref List 1
                ref_dpb_index[LAST]  = lay3_idx;
                ref_dpb_index[LAST2] = lay2_idx;
                ref_dpb_index[LAST3] = lay1_1_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = lay4_idx;
                ref_dpb_index[ALT2]  = base2_idx;
                ref_dpb_index[ALT]   = base0_idx;
            } else if (pic_idx == 30) {
                //{1, 3, 7, 31}, // GOP Index 31 - Ref List 0
                //{ -1, 15, 63, 0 } // GOP Index 31 - Ref List 1
                ref_dpb_index[LAST]  = lay4_idx;
                ref_dpb_index[LAST2] = lay3_idx;
                ref_dpb_index[LAST3] = lay2_idx;
                ref_dpb_index[GOLD]  = base1_idx;
                ref_dpb_index[BWD]   = base2_idx;
                ref_dpb_index[ALT2]  = lay1_1_idx;
                ref_dpb_index[ALT]   = base0_idx;
            } else {
                SVT_LOG("Error in GOp indexing\n");
            }

            av1_rps->refresh_frame_mask = 0;
            break;

        default:
            SVT_ERROR("unexpected picture mini Gop number\n");
            break;
        }

        update_ref_poc_array(ref_dpb_index, ref_poc_array, ctx->dpb);

        set_ref_list_counts(pcs, ctx);
        prune_refs(av1_rps, pcs->ref_list0_count, pcs->ref_list1_count);

        if (!set_frame_display_params(pcs, ctx, mg_idx)) {
            if (temporal_layer < hierarchical_levels) {
                frm_hdr->show_frame    = false;
                pcs->has_show_existing = false;
            } else {
                frm_hdr->show_frame    = true;
                pcs->has_show_existing = true;

                if (pic_idx == 0) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 2) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 4) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 6) {
                    frm_hdr->show_existing_frame = lay2_idx;
                } else if (pic_idx == 8) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 10) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 12) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 14) {
                    frm_hdr->show_existing_frame = lay1_1_idx;
                } else if (pic_idx == 16) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 18) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 20) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 22) {
                    frm_hdr->show_existing_frame = lay2_idx;
                } else if (pic_idx == 24) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 26) {
                    frm_hdr->show_existing_frame = lay3_idx;
                } else if (pic_idx == 28) {
                    frm_hdr->show_existing_frame = lay4_idx;
                } else if (pic_idx == 30) {
                    frm_hdr->show_existing_frame = base2_idx;
                } else {
                    SVT_LOG("Error in GOP indexing for hierarchical level %d\n", pcs->hierarchical_levels);
                }
            }
        }
    } else {
        SVT_ERROR("Not supported GOP structure!");
        exit(0);
    }

    if (frm_hdr->frame_type == S_FRAME) {
        set_sframe_rps(pcs, enc_ctx, ctx);
    }

    // This should already be the case
    if (pcs->is_overlay) {
        av1_rps->refresh_frame_mask = 0;
    }
}

/***************************************************************************************************
// Perform Required Picture Analysis Processing for the Overlay frame
***************************************************************************************************/
void perform_simple_picture_analysis_for_overlay(PictureParentControlSet* pcs) {
    EbPictureBufferDesc* input_padded_pic;
    EbPictureBufferDesc* input_pic;
    EbPaReferenceObject* pa_ref_obj_;

    SequenceControlSet* scs = pcs->scs;
    input_pic               = pcs->enhanced_pic;
    pa_ref_obj_             = (EbPaReferenceObject*)pcs->pa_ref_pic_wrapper->object_ptr;
    input_padded_pic        = (EbPictureBufferDesc*)pa_ref_obj_->input_padded_pic;

    // Pad pictures to multiple min cu size
    svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(scs, input_pic);

    // Pre processing operations performed on the input picture
    svt_aom_picture_pre_processing_operations(pcs, scs);

    if (input_pic->color_format >= EB_YUV422) {
        // Jing: Do the conversion of 422/444=>420 here since it's multi-threaded kernel
        //       Reuse the Y, only add cb/cr in the newly created buffer desc
        //       NOTE: since denoise may change the src, so this part is after svt_aom_picture_pre_processing_operations()
        pcs->chroma_downsampled_pic->buffer_y = input_pic->buffer_y;
        svt_aom_down_sample_chroma(input_pic, pcs->chroma_downsampled_pic);
    } else {
        pcs->chroma_downsampled_pic = input_pic;
    }

    // R2R FIX: copying input_pic to input_padded_pic for motion_estimate_sb needs it
    {
        uint8_t* pa = input_padded_pic->buffer_y + input_padded_pic->org_x +
            input_padded_pic->org_y * input_padded_pic->stride_y;
        uint8_t* in = input_pic->buffer_y + input_pic->org_x + input_pic->org_y * input_pic->stride_y;
        for (uint32_t row = 0; row < input_pic->height; row++) {
            svt_memcpy(pa + row * input_padded_pic->stride_y,
                       in + row * input_pic->stride_y,
                       sizeof(uint8_t) * input_pic->width);
        }
    }

    // Pad input picture to complete border SBs
    svt_aom_pad_picture_to_multiple_of_sb_dimensions(input_padded_pic);
    // 1/4 & 1/16 input picture downsampling through filtering
    svt_aom_downsample_filtering_input_picture(pcs,
                                               input_padded_pic,
                                               (EbPictureBufferDesc*)pa_ref_obj_->quarter_downsampled_picture_ptr,
                                               (EbPictureBufferDesc*)pa_ref_obj_->sixteenth_downsampled_picture_ptr);

    // Gathering statistics of input picture, including Variance Calculation, Histogram Bins
    svt_aom_gathering_picture_statistics(scs, pcs, input_padded_pic, pa_ref_obj_->sixteenth_downsampled_picture_ptr);

    pcs->sc_class0 = pcs->alt_ref_ppcs_ptr->sc_class0;
    pcs->sc_class1 = pcs->alt_ref_ppcs_ptr->sc_class1;
    pcs->sc_class2 = pcs->alt_ref_ppcs_ptr->sc_class2;
    pcs->sc_class3 = pcs->alt_ref_ppcs_ptr->sc_class3;
    pcs->sc_class4 = pcs->alt_ref_ppcs_ptr->sc_class4;
}

/***************************************************************************************************
 * Initialize the overlay frame
***************************************************************************************************/
void initialize_overlay_frame(PictureParentControlSet* pcs) {
    pcs->scene_change_flag              = false;
    pcs->cra_flag                       = false;
    pcs->idr_flag                       = false;
    pcs->last_idr_picture               = pcs->alt_ref_ppcs_ptr->last_idr_picture;
    pcs->pred_structure                 = pcs->alt_ref_ppcs_ptr->pred_structure;
    pcs->pred_struct_ptr                = pcs->alt_ref_ppcs_ptr->pred_struct_ptr;
    pcs->pred_struct_index              = pcs->alt_ref_ppcs_ptr->pred_struct_index;
    pcs->pic_idx_in_mg                  = pcs->alt_ref_ppcs_ptr->pic_idx_in_mg;
    pcs->hierarchical_levels            = pcs->alt_ref_ppcs_ptr->hierarchical_levels;
    pcs->hierarchical_layers_diff       = 0;
    pcs->init_pred_struct_position_flag = false;
    pcs->pre_assignment_buffer_count    = pcs->alt_ref_ppcs_ptr->pre_assignment_buffer_count;
    pcs->slice_type                     = B_SLICE;
    // set the overlay frame as non reference frame with max temporal layer index
    pcs->temporal_layer_index = (uint8_t)pcs->hierarchical_levels;
    pcs->is_highest_layer     = true;
    pcs->ref_list0_count      = 1;
    pcs->ref_list1_count      = 0;

    perform_simple_picture_analysis_for_overlay(pcs);
}

/*
  ret number of past picture(not including current) in mg buffer.

*/
static int32_t avail_past_pictures(PictureParentControlSet** buf, uint32_t buf_size, uint64_t input_pic) {
    //buffer has at least curr picture
    int32_t tot_past = 0;
    for (uint32_t pic = 0; pic < buf_size; pic++) {
        if (buf[pic]->picture_number < input_pic) {
            tot_past++;
        }
    }
    return tot_past;
}

/*
  searches a picture in a given pcs buffer
*/
int32_t search_this_pic(PictureParentControlSet** buf, uint32_t buf_size, uint64_t input_pic) {
    int32_t index = -1;
    for (uint32_t pic = 0; pic < buf_size; pic++) {
        if (buf[pic]->picture_number == input_pic) {
            index = (int32_t)pic;
            break;
        }
    }
    return index;
}

/*
  Tells if an Intra picture should be delayed to get next mini-gop
*/
bool svt_aom_is_delayed_intra(PictureParentControlSet* pcs) {
    if ((pcs->idr_flag || pcs->cra_flag) && pcs->pred_structure == RANDOM_ACCESS) {
        if (pcs->scs->static_config.intra_period_length == 0 || pcs->end_of_sequence_flag) {
            return 0;
        } else if (pcs->idr_flag ||
                   (pcs->cra_flag &&
                    pcs->pre_assignment_buffer_count < pcs->pred_struct_ptr->pred_struct_entry_count)) {
            return 1;
        } else {
            return 0;
        }
    } else {
        return 0;
    }
}

void first_pass_frame_end_one_pass(PictureParentControlSet* pcs);

/* modulate_ref_pics()
 For INTRA, the modulation uses the noise level, and towards increasing the number of ref_pics
 For BASE and L1, the modulation uses the filt_INTRA-to-unfilterd_INTRA distortion range, and towards decreasing the number of ref_pics
*/
static int ref_pics_modulation(PictureParentControlSet* pcs, int32_t noise_levels_log1p_fp16) {
    int offset = 0;

    if (pcs->slice_type == I_SLICE) {
        // Adjust number of filtering frames based on noise and quantization factor.
        // Basically, we would like to use more frames to filter low-noise frame such
        // that the filtered frame can provide better predictions for more frames.
        // Also, when the quantization factor is small enough (lossless compression),
        // we will not change the number of frames for key frame filtering, which is
        // to avoid visual quality drop.
        if (noise_levels_log1p_fp16 < 26572 /*FLOAT2FP(log1p(0.5), 16, int32_t)*/) {
            offset = 6;
        } else if (noise_levels_log1p_fp16 < 45426 /*FLOAT2FP(log1p(1.0), 16, int32_t)*/) {
            offset = 4;
        } else if (noise_levels_log1p_fp16 < 71998 /*FLOAT2FP(log1p(2.0), 16, int32_t)*/) {
            offset = 2;
        }
    } else if (pcs->temporal_layer_index == 0) {
        int ratio = noise_levels_log1p_fp16 ? (pcs->filt_to_unfilt_diff * 100) / noise_levels_log1p_fp16 : 0;
        switch (pcs->tf_ctrls.modulate_pics) {
        case 0:
            offset = 0;
            break;
        case 1:
            if (ratio < 100) {
                offset = 5;
            } else {
                offset = TF_MAX_EXTENSION;
            }
            break;
        case 2:
            if (ratio < 50) {
                offset = 3;
            } else if (ratio < 100) {
                offset = 5;
            } else {
                offset = TF_MAX_EXTENSION;
            }
            break;
        case 3:
            if (ratio < 50) {
                offset = 3;
            } else if (ratio < 100) {
                offset = 4;
            } else {
                offset = 5;
            }
            break;
        case 4:
            if (ratio < 50) {
                offset = 0;
            } else if (ratio < 100) {
                offset = 1;
            } else {
                offset = 2;
            }
            break;
        default:
            break;
        }
    } else {
        int ratio = noise_levels_log1p_fp16 ? (pcs->filt_to_unfilt_diff * 100) / noise_levels_log1p_fp16 : 0;
        switch (pcs->tf_ctrls.modulate_pics) {
        case 0:
            offset = 0;
            break;
        case 1:
            if (ratio < 25) {
                offset = 0;
            } else {
                offset = 1;
            }
            break;
        case 2:
            if (ratio < 50) {
                offset = 0;
            } else {
                offset = 1;
            }

            break;
        case 3:
            if (ratio < 75) {
                offset = 0;
            } else {
                offset = 1;
            }
            break;
        default:
            break;
        }
    }
    // Modulate offset using qp
    if (pcs->tf_ctrls.qp_opt) {
        uint32_t q_weight, q_weight_denom;
        svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.tf_ref_qp_based_th_scaling,
                                                &q_weight,
                                                &q_weight_denom,
                                                pcs->scs->static_config.qp);
        offset = DIVIDE_AND_ROUND(offset * q_weight, q_weight_denom);
    }
    return offset;
}

static EbErrorType derive_tf_window_params(SequenceControlSet* scs, EncodeContext* enc_ctx,
                                           PictureParentControlSet* pcs, PictureDecisionContext* pd_ctx) {
    PictureParentControlSet* centre_pcs          = pcs;
    EbPictureBufferDesc*     central_picture_ptr = centre_pcs->enhanced_pic;

    // chroma subsampling
    uint32_t ss_x                    = centre_pcs->scs->subsampling_x;
    uint32_t ss_y                    = centre_pcs->scs->subsampling_y;
    int32_t* noise_levels_log1p_fp16 = &(centre_pcs->noise_levels_log1p_fp16[0]);
    int32_t  noise_level_fp16;

    uint8_t do_noise_est = pcs->tf_ctrls.use_intra_for_noise_est ? 0 : 1;
    if (centre_pcs->slice_type == I_SLICE) {
        do_noise_est = 1;
    }
    // allocate 16 bit buffer
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    uint32_t encoder_bit_depth = centre_pcs->scs->static_config.encoder_bit_depth;
    bool     is_highbd         = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;
    if (is_highbd) {
        EB_MALLOC_ARRAY(centre_pcs->altref_buffer_highbd[C_Y], central_picture_ptr->luma_size);
        if (pcs->tf_ctrls.chroma_lvl) {
            EB_MALLOC_ARRAY(centre_pcs->altref_buffer_highbd[C_U], central_picture_ptr->chroma_size);
            EB_MALLOC_ARRAY(centre_pcs->altref_buffer_highbd[C_V], central_picture_ptr->chroma_size);
        }

        // pack byte buffers to 16 bit buffer
        svt_aom_pack_highbd_pic(central_picture_ptr, centre_pcs->altref_buffer_highbd, ss_x, ss_y, true);
        // Estimate source noise level
        uint16_t* altref_buffer_highbd_start[COLOR_CHANNELS];
        altref_buffer_highbd_start[C_Y] = centre_pcs->altref_buffer_highbd[C_Y] +
            central_picture_ptr->org_y * central_picture_ptr->stride_y + central_picture_ptr->org_x;
        if (pcs->tf_ctrls.chroma_lvl) {
            altref_buffer_highbd_start[C_U] = centre_pcs->altref_buffer_highbd[C_U] +
                (central_picture_ptr->org_y >> ss_y) * central_picture_ptr->stride_bit_inc_cb +
                (central_picture_ptr->org_x >> ss_x);

            altref_buffer_highbd_start[C_V] = centre_pcs->altref_buffer_highbd[C_V] +
                (central_picture_ptr->org_y >> ss_y) * central_picture_ptr->stride_bit_inc_cr +
                (central_picture_ptr->org_x >> ss_x);
        } else {
            altref_buffer_highbd_start[C_U] = NOT_USED_VALUE;
            altref_buffer_highbd_start[C_V] = NOT_USED_VALUE;
        }

        if (do_noise_est) {
            noise_level_fp16             = svt_estimate_noise_highbd_fp16(altref_buffer_highbd_start[C_Y], // Y only
                                                              central_picture_ptr->width,
                                                              central_picture_ptr->height,
                                                              central_picture_ptr->stride_y,
                                                              encoder_bit_depth);
            noise_levels_log1p_fp16[C_Y] = svt_aom_noise_log1p_fp16(noise_level_fp16);
        }
        if (pcs->tf_ctrls.chroma_lvl) {
            noise_level_fp16             = svt_estimate_noise_highbd_fp16(altref_buffer_highbd_start[C_U], // U only
                                                              (central_picture_ptr->width >> 1),
                                                              (central_picture_ptr->height >> 1),
                                                              central_picture_ptr->stride_cb,
                                                              encoder_bit_depth);
            noise_levels_log1p_fp16[C_U] = svt_aom_noise_log1p_fp16(noise_level_fp16);

            noise_level_fp16             = svt_estimate_noise_highbd_fp16(altref_buffer_highbd_start[C_V], // V only
                                                              (central_picture_ptr->width >> 1),
                                                              (central_picture_ptr->height >> 1),
                                                              central_picture_ptr->stride_cb,
                                                              encoder_bit_depth);
            noise_levels_log1p_fp16[C_V] = svt_aom_noise_log1p_fp16(noise_level_fp16);
        }
    } else
#endif
    {
        EbByte buffer_y = central_picture_ptr->buffer_y + central_picture_ptr->org_y * central_picture_ptr->stride_y +
            central_picture_ptr->org_x;
        EbByte buffer_u = central_picture_ptr->buffer_cb +
            (central_picture_ptr->org_y >> ss_y) * central_picture_ptr->stride_cb +
            (central_picture_ptr->org_x >> ss_x);
        EbByte buffer_v = central_picture_ptr->buffer_cr +
            (central_picture_ptr->org_y >> ss_x) * central_picture_ptr->stride_cr +
            (central_picture_ptr->org_x >> ss_x);

        if (do_noise_est) {
            noise_level_fp16             = svt_estimate_noise_fp16(buffer_y, // Y
                                                       central_picture_ptr->width,
                                                       central_picture_ptr->height,
                                                       central_picture_ptr->stride_y);
            noise_levels_log1p_fp16[C_Y] = svt_aom_noise_log1p_fp16(noise_level_fp16);
        }
        if (pcs->tf_ctrls.chroma_lvl) {
            noise_level_fp16             = svt_estimate_noise_fp16(buffer_u, // U
                                                       (central_picture_ptr->width >> ss_x),
                                                       (central_picture_ptr->height >> ss_y),
                                                       central_picture_ptr->stride_cb);
            noise_levels_log1p_fp16[C_U] = svt_aom_noise_log1p_fp16(noise_level_fp16);

            noise_level_fp16             = svt_estimate_noise_fp16(buffer_v, // V
                                                       (central_picture_ptr->width >> ss_x),
                                                       (central_picture_ptr->height >> ss_y),
                                                       central_picture_ptr->stride_cr);
            noise_levels_log1p_fp16[C_V] = svt_aom_noise_log1p_fp16(noise_level_fp16);
        }
    }
    if (do_noise_est) {
        pd_ctx->last_i_noise_levels_log1p_fp16[0] = noise_levels_log1p_fp16[0];
    } else {
        noise_levels_log1p_fp16[0] = pd_ctx->last_i_noise_levels_log1p_fp16[0];
    }
    // Set is_noise_level for the tf off case
    pcs->is_noise_level = (pd_ctx->last_i_noise_levels_log1p_fp16[0] >= VQ_NOISE_LVL_TH);
    // Adjust the number of filtering frames
    int offset = pcs->tf_ctrls.modulate_pics ? ref_pics_modulation(pcs, noise_levels_log1p_fp16[0]) : 0;
    if (scs->static_config.pred_structure != RANDOM_ACCESS) {
        int num_past_pics   = pcs->tf_ctrls.num_past_pics + (pcs->tf_ctrls.modulate_pics ? offset : 0);
        num_past_pics       = MIN(pcs->tf_ctrls.max_num_past_pics, num_past_pics);
        int num_future_pics = pcs->tf_ctrls.num_future_pics + (pcs->tf_ctrls.modulate_pics ? offset : 0);
        num_future_pics     = MIN(pcs->tf_ctrls.max_num_future_pics, num_future_pics);
        //initilize list
        for (int pic_itr = 0; pic_itr < ALTREF_MAX_NFRAMES; pic_itr++) {
            pcs->temp_filt_pcs_list[pic_itr] = NULL;
        }

        //get previous
        for (int pic_itr = 0; pic_itr < num_past_pics; pic_itr++) {
            int32_t idx = search_this_pic(
                pd_ctx->tf_pic_array, pd_ctx->tf_pic_arr_cnt, pcs->picture_number - num_past_pics + pic_itr);
            if (idx >= 0) {
                pcs->temp_filt_pcs_list[pic_itr] = pd_ctx->tf_pic_array[idx];
            }
        }

        //get central
        pcs->temp_filt_pcs_list[num_past_pics] = pcs;

        int actual_past_pics   = num_past_pics;
        int actual_future_pics = 0;
        int pic_i;
        //search reord-queue to get the future pictures
        for (pic_i = 0; pic_i < num_future_pics; pic_i++) {
            int32_t q_index = QUEUE_GET_NEXT_SPOT(
                pcs->pic_decision_reorder_queue_idx, pic_i + 1, enc_ctx->picture_decision_reorder_queue_size);
            if (enc_ctx->picture_decision_reorder_queue[q_index]->ppcs_wrapper != NULL) {
                PictureParentControlSet* pcs_itr = (PictureParentControlSet*)enc_ctx
                                                       ->picture_decision_reorder_queue[q_index]
                                                       ->ppcs_wrapper->object_ptr;
                // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                if (pcs_itr->frame_width != pcs->frame_width || pcs_itr->frame_height != pcs->frame_height) {
                    break;
                }
                pcs->temp_filt_pcs_list[pic_i + num_past_pics + 1] = pcs_itr;
                actual_future_pics++;
            } else {
                break;
            }
        }

        //search in pre-ass if still short
        if (pic_i < num_future_pics) {
            for (int pic_i_future = pic_i; pic_i_future < num_future_pics; pic_i_future++) {
                for (uint32_t pic_i_pa = 0; pic_i_pa < enc_ctx->pre_assignment_buffer_count; pic_i_pa++) {
                    PictureParentControlSet* pcs_itr =
                        (PictureParentControlSet*)enc_ctx->pre_assignment_buffer[pic_i_pa]->object_ptr;
                    // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                    if (pcs_itr->picture_number == pcs->picture_number + pic_i_future + 1 &&
                        pcs_itr->frame_width == pcs->frame_width && pcs_itr->frame_height == pcs->frame_height) {
                        pcs->temp_filt_pcs_list[pic_i_future + num_past_pics + 1] = pcs_itr;
                        actual_future_pics++;
                        break; //exist the pre-ass loop, go search the next
                    }
                }
            }
        }
        pcs->past_altref_nframes   = actual_past_pics;
        pcs->future_altref_nframes = actual_future_pics;

        // adjust the temporal filtering pcs buffer to remove unused past pictures
        if (actual_past_pics != num_past_pics) {
            pic_i = 0;
            while (pcs->temp_filt_pcs_list[pic_i] != NULL) {
                pcs->temp_filt_pcs_list[pic_i] = pcs->temp_filt_pcs_list[pic_i + num_past_pics - actual_past_pics];
                pic_i++;
            }
        }
    } else {
        if (svt_aom_is_delayed_intra(pcs)) {
            //initilize list
            for (int pic_itr = 0; pic_itr < ALTREF_MAX_NFRAMES; pic_itr++) {
                pcs->temp_filt_pcs_list[pic_itr] = NULL;
            }

            pcs->temp_filt_pcs_list[0] = pcs;
            uint32_t num_future_pics   = pcs->tf_ctrls.num_future_pics + (pcs->tf_ctrls.modulate_pics ? offset : 0);
            num_future_pics            = MIN(pcs->tf_ctrls.max_num_future_pics, num_future_pics);
            // Update the key frame pred structure;
            int32_t idx = search_this_pic(pd_ctx->mg_pictures_array, pd_ctx->mg_size, pcs->picture_number + 1);

            if (centre_pcs->hierarchical_levels != pcs->temp_filt_pcs_list[0]->hierarchical_levels ||
                centre_pcs->hierarchical_levels != pd_ctx->mg_pictures_array[idx]->hierarchical_levels) {
                centre_pcs->hierarchical_levels = pcs->temp_filt_pcs_list[0]->hierarchical_levels =
                    pd_ctx->mg_pictures_array[idx]->hierarchical_levels;
            }
            num_future_pics = MIN((uint8_t)num_future_pics,
                                  svt_aom_tf_max_ref_per_struct(pcs->hierarchical_levels, 0, 1));
            uint32_t pic_i;
            for (pic_i = 0; pic_i < num_future_pics; pic_i++) {
                int32_t idx_1 = search_this_pic(
                    pd_ctx->mg_pictures_array, pd_ctx->mg_size, pcs->picture_number + pic_i + 1);
                if (idx_1 >= 0) {
                    // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                    if (pd_ctx->mg_pictures_array[idx_1]->frame_width != pcs->frame_width ||
                        pd_ctx->mg_pictures_array[idx_1]->frame_height != pcs->frame_height) {
                        break;
                    }
                    pcs->temp_filt_pcs_list[pic_i + 1]                        = pd_ctx->mg_pictures_array[idx_1];
                    uint8_t active_region_cnt                                 = 0;
                    pd_ctx->mg_pictures_array[idx_1]->tf_ahd_error_to_central = calc_ahd(
                        scs, pcs, pd_ctx->mg_pictures_array[idx_1], &active_region_cnt);
                    pd_ctx->mg_pictures_array[idx_1]->tf_active_region_present = active_region_cnt > 0;
                } else {
                    break;
                }
            }

            pcs->past_altref_nframes   = 0;
            pcs->future_altref_nframes = pic_i;
        } else {
            if (pcs->idr_flag) {
                //initilize list
                for (int pic_itr = 0; pic_itr < ALTREF_MAX_NFRAMES; pic_itr++) {
                    pcs->temp_filt_pcs_list[pic_itr] = NULL;
                }

                pcs->temp_filt_pcs_list[0] = pcs;
                uint32_t num_future_pics   = pcs->tf_ctrls.num_future_pics + (pcs->tf_ctrls.modulate_pics ? offset : 0);
                num_future_pics            = MIN(pcs->tf_ctrls.max_num_future_pics, num_future_pics);
                num_future_pics            = MIN((uint8_t)num_future_pics,
                                      svt_aom_tf_max_ref_per_struct(pcs->hierarchical_levels, 0, 1));
                uint32_t num_past_pics     = 0;
                uint32_t pic_i;
                //search reord-queue to get the future pictures
                for (pic_i = 0; pic_i < num_future_pics; pic_i++) {
                    int32_t q_index = QUEUE_GET_NEXT_SPOT(
                        pcs->pic_decision_reorder_queue_idx, pic_i + 1, enc_ctx->picture_decision_reorder_queue_size);
                    if (enc_ctx->picture_decision_reorder_queue[q_index]->ppcs_wrapper != NULL) {
                        PictureParentControlSet* pcs_itr = (PictureParentControlSet*)enc_ctx
                                                               ->picture_decision_reorder_queue[q_index]
                                                               ->ppcs_wrapper->object_ptr;
                        // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                        if (pcs_itr->frame_width != pcs->frame_width || pcs_itr->frame_height != pcs->frame_height) {
                            break;
                        }
                        pcs->temp_filt_pcs_list[pic_i + num_past_pics + 1] = pcs_itr;
                        uint8_t active_region_cnt                          = 0;
                        pcs_itr->tf_ahd_error_to_central  = calc_ahd(scs, pcs, pcs_itr, &active_region_cnt);
                        pcs_itr->tf_active_region_present = active_region_cnt > 0;
                    } else {
                        break;
                    }
                }

                pcs->past_altref_nframes   = 0;
                pcs->future_altref_nframes = pic_i;
            }

            else {
                int num_past_pics   = MAX(1, (int)pcs->tf_ctrls.num_past_pics + offset);
                int num_future_pics = MAX(1, (int)pcs->tf_ctrls.num_future_pics + offset);
                num_past_pics       = MIN(pcs->tf_ctrls.max_num_past_pics, num_past_pics);
                num_future_pics     = MIN(pcs->tf_ctrls.max_num_future_pics, num_future_pics);
                num_past_pics       = MIN(
                    num_past_pics,
                    svt_aom_tf_max_ref_per_struct(pcs->hierarchical_levels, pcs->temporal_layer_index ? 2 : 1, 0));
                num_future_pics = MIN(
                    num_future_pics,
                    svt_aom_tf_max_ref_per_struct(pcs->hierarchical_levels, pcs->temporal_layer_index ? 2 : 1, 1));

                // Initialize list
                for (int pic_itr = 0; pic_itr < ALTREF_MAX_NFRAMES; pic_itr++) {
                    pcs->temp_filt_pcs_list[pic_itr] = NULL;
                }
                // limit the number of pictures to make sure there are enough pictures in the buffer. i.e. Intra CRA case
                // limit the number of pictures to make sure there are enough pictures in the buffer. i.e. Intra CRA case
                num_past_pics = MIN(
                    num_past_pics,
                    avail_past_pictures(pd_ctx->mg_pictures_array, pd_ctx->mg_size, pcs->picture_number));
                // get previous+current pictures from the the pre-assign buffer
                for (int pic_itr = 0; pic_itr <= num_past_pics; pic_itr++) {
                    int32_t idx = search_this_pic(
                        pd_ctx->mg_pictures_array, pd_ctx->mg_size, pcs->picture_number - num_past_pics + pic_itr);
                    if (idx >= 0) {
                        // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                        if (pd_ctx->mg_pictures_array[idx]->frame_width != pcs->frame_width ||
                            pd_ctx->mg_pictures_array[idx]->frame_height != pcs->frame_height) {
                            break;
                        }
                        pcs->temp_filt_pcs_list[pic_itr]                        = pd_ctx->mg_pictures_array[idx];
                        uint8_t active_region_cnt                               = 0;
                        pd_ctx->mg_pictures_array[idx]->tf_ahd_error_to_central = calc_ahd(
                            scs, pcs, pd_ctx->mg_pictures_array[idx], &active_region_cnt);
                        pd_ctx->mg_pictures_array[idx]->tf_active_region_present = active_region_cnt > 0;
                    }
                }
                int actual_past_pics   = num_past_pics;
                int actual_future_pics = 0;
                int pic_i;
                //search reord-queue to get the future pictures
                for (pic_i = 0; pic_i < num_future_pics; pic_i++) {
                    int32_t q_index = QUEUE_GET_NEXT_SPOT(
                        pcs->pic_decision_reorder_queue_idx, pic_i + 1, enc_ctx->picture_decision_reorder_queue_size);
                    if (enc_ctx->picture_decision_reorder_queue[q_index]->ppcs_wrapper != NULL) {
                        PictureParentControlSet* pcs_itr = (PictureParentControlSet*)enc_ctx
                                                               ->picture_decision_reorder_queue[q_index]
                                                               ->ppcs_wrapper->object_ptr;
                        // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                        if (pcs_itr->frame_width != pcs->frame_width || pcs_itr->frame_height != pcs->frame_height) {
                            break;
                        }
                        pcs->temp_filt_pcs_list[pic_i + num_past_pics + 1] = pcs_itr;
                        uint8_t active_region_cnt                          = 0;
                        pcs_itr->tf_ahd_error_to_central  = calc_ahd(scs, pcs, pcs_itr, &active_region_cnt);
                        pcs_itr->tf_active_region_present = active_region_cnt > 0;
                        actual_future_pics++;
                    } else {
                        break;
                    }
                }

                //search in pre-ass if still short
                if (pic_i < num_future_pics) {
                    for (int pic_i_future = pic_i; pic_i_future < num_future_pics; pic_i_future++) {
                        for (uint32_t pic_i_pa = 0; pic_i_pa < enc_ctx->pre_assignment_buffer_count; pic_i_pa++) {
                            PictureParentControlSet* pcs_itr =
                                (PictureParentControlSet*)enc_ctx->pre_assignment_buffer[pic_i_pa]->object_ptr;
                            // if resolution has changed, and the pcs with new resolution should not be used in temporal filtering
                            if (pcs_itr->picture_number == pcs->picture_number + pic_i_future + 1 &&
                                pcs_itr->frame_width == pcs->frame_width &&
                                pcs_itr->frame_height == pcs->frame_height) {
                                pcs->temp_filt_pcs_list[pic_i_future + num_past_pics + 1] = pcs_itr;
                                uint8_t active_region_cnt                                 = 0;
                                pcs_itr->tf_ahd_error_to_central  = calc_ahd(scs, pcs, pcs_itr, &active_region_cnt);
                                pcs_itr->tf_active_region_present = active_region_cnt > 0;
                                actual_future_pics++;
                                break; //exist the pre-ass loop, go search the next
                            }
                        }
                    }
                }
                pcs->past_altref_nframes   = actual_past_pics;
                pcs->future_altref_nframes = actual_future_pics;

                // adjust the temporal filtering pcs buffer to remove unused past pictures
                if (actual_past_pics != num_past_pics) {
                    pic_i = 0;
                    while (pcs->temp_filt_pcs_list[pic_i] != NULL) {
                        pcs->temp_filt_pcs_list[pic_i] =
                            pcs->temp_filt_pcs_list[pic_i + num_past_pics - actual_past_pics];
                        pic_i++;
                    }
                }
            }
        }

        // Calc the avg_ahd_error
        centre_pcs->tf_avg_ahd_error = 0;
        if (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes) {
            uint64_t tot_luma = 0;
            int      tot_err  = 0;

            for (int i = 0; i < (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes + 1); i++) {
                if (i != centre_pcs->past_altref_nframes) {
                    tot_luma += pcs->temp_filt_pcs_list[i]->avg_luma;
                    tot_err += pcs->temp_filt_pcs_list[i]->tf_ahd_error_to_central;
                }
            }
            centre_pcs->tf_avg_luma = tot_luma / (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes);
            centre_pcs->tf_avg_ahd_error = tot_err /
                (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes);
        }
    }
    return EB_ErrorNone;
}

/*
store this input  picture to be used for TF-ing of upcoming base
increment live count of the required ressources to be used by TF of upcoming base.
will be released once TF is done
*/
static void low_delay_store_tf_pictures(SequenceControlSet* scs, PictureParentControlSet* pcs,
                                        PictureDecisionContext* ctx) {
    const uint32_t mg_size  = 1 << (IS_SFRAME_FLEXIBLE_INSERT(scs->static_config.sframe_mode)
                                        ? (uint32_t)ctx->sframe_hier_lvls
                                        : scs->static_config.hierarchical_levels);
    const uint32_t tot_past = scs->tf_params_per_type[1].max_num_past_pics;
    if (pcs->temporal_layer_index != 0 && pcs->pic_idx_in_mg + 1 + tot_past >= mg_size) {
        //store this picture to be used for TF-ing upcoming base
        ctx->tf_pic_array[ctx->tf_pic_arr_cnt++] = pcs;

        //increment live count of these ressources to be used by TF of upcoming base. will be released once TF is done.
        svt_object_inc_live_count(pcs->p_pcs_wrapper_ptr, 1);
        svt_object_inc_live_count(pcs->input_pic_wrapper, 1);
        svt_object_inc_live_count(pcs->pa_ref_pic_wrapper, 1);
        svt_object_inc_live_count(pcs->scs_wrapper, 1);
        if (pcs->y8b_wrapper) {
            svt_object_inc_live_count(pcs->y8b_wrapper, 1);
        }
    }
}

/*
 TF is done, release ressources and reset the tf picture buffer.
*/
static void low_delay_release_tf_pictures(PictureDecisionContext* ctx) {
    for (uint32_t pic_it = 0; pic_it < ctx->tf_pic_arr_cnt; pic_it++) {
        PictureParentControlSet* past_pcs = ctx->tf_pic_array[pic_it];

        svt_release_object(past_pcs->input_pic_wrapper);

        if (past_pcs->y8b_wrapper) {
            svt_release_object(past_pcs->y8b_wrapper);
        }

        svt_release_object(past_pcs->pa_ref_pic_wrapper);
        svt_release_object(past_pcs->scs_wrapper);
        //ppcs should be the last one to release
        svt_release_object(past_pcs->p_pcs_wrapper_ptr);
    }

    memset(ctx->tf_pic_array, 0, ctx->tf_pic_arr_cnt * sizeof(PictureParentControlSet*));
    ctx->tf_pic_arr_cnt = 0;
}

/*
  Performs Motion Compensated Temporal Filtering in ME process
*/
static void mctf_frame(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* pd_ctx) {
    if (scs->static_config.pred_structure != RANDOM_ACCESS && scs->tf_params_per_type[1].enabled) {
        low_delay_store_tf_pictures(scs, pcs, pd_ctx);
    }
    if (pcs->tf_ctrls.enabled) {
        derive_tf_window_params(scs, scs->enc_ctx, pcs, pd_ctx);
        pcs->temp_filt_prep_done = 0;
        pcs->tf_tot_horz_blks = pcs->tf_tot_vert_blks = 0;

        // Start Filtering in ME processes
        {
            int16_t seg_idx;

            // Initialize Segments
            pcs->tf_segments_column_count = scs->tf_segment_column_count;
            pcs->tf_segments_row_count    = scs->tf_segment_row_count;
            pcs->tf_segments_total_count  = (uint16_t)(pcs->tf_segments_column_count * pcs->tf_segments_row_count);
            pcs->temp_filt_seg_acc        = 0;
            for (seg_idx = 0; seg_idx < pcs->tf_segments_total_count; ++seg_idx) {
                EbObjectWrapper*        out_results_wrapper;
                PictureDecisionResults* out_results;

                svt_get_empty_object(pd_ctx->picture_decision_results_output_fifo_ptr, &out_results_wrapper);
                out_results                = (PictureDecisionResults*)out_results_wrapper->object_ptr;
                out_results->pcs_wrapper   = pcs->p_pcs_wrapper_ptr;
                out_results->segment_index = seg_idx;
                out_results->task_type     = 1;
                svt_post_full_object(out_results_wrapper);
            }

            svt_block_on_semaphore(pcs->temp_filt_done_semaphore);
        }

        if (pcs->tf_tot_horz_blks > pcs->tf_tot_vert_blks * 6 / 4) {
            pd_ctx->tf_motion_direction = 0;
        } else if (pcs->tf_tot_vert_blks > pcs->tf_tot_horz_blks * 6 / 4) {
            pd_ctx->tf_motion_direction = 1;
        } else {
            pd_ctx->tf_motion_direction = -1;
        }
    } else {
        pcs->do_tf = false; // set temporal filtering flag OFF for current picture
    }

    pcs->is_noise_level = (pd_ctx->last_i_noise_levels_log1p_fp16[0] >= VQ_NOISE_LVL_TH);

    if (scs->static_config.pred_structure != RANDOM_ACCESS && scs->tf_params_per_type[1].enabled &&
        pcs->temporal_layer_index == 0) {
        low_delay_release_tf_pictures(pd_ctx);
    }
}

bool get_similar_ref_brightness(PictureParentControlSet* pcs) {
    bool similar_brightness_refs = false;
    if (pcs->slice_type == B_SLICE && pcs->hierarchical_levels > 0 && pcs->ref_list1_count_try > 0) {
        EbPaReferenceObject* ref_obj_0 = (EbPaReferenceObject*)pcs->ref_pa_pic_ptr_array[0][0]->object_ptr;
        EbPaReferenceObject* ref_obj_1 = (EbPaReferenceObject*)pcs->ref_pa_pic_ptr_array[1][0]->object_ptr;
        if (ref_obj_0->avg_luma != INVALID_LUMA && ref_obj_1->avg_luma != INVALID_LUMA) {
            const int32_t luma_th = 5;
            if (ABS((int)ref_obj_0->avg_luma - (int)pcs->avg_luma) < luma_th &&
                ABS((int)ref_obj_1->avg_luma - (int)pcs->avg_luma) < luma_th) {
                similar_brightness_refs = true;
            }
        }
    }

    return similar_brightness_refs;
}

static void send_picture_out(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    EbObjectWrapper* me_wrapper;
    EbObjectWrapper* out_results_wrapper;

    //every picture enherits latest motion direction from TF
    pcs->tf_motion_direction = ctx->tf_motion_direction;
    MrpCtrls* mrp_ctrl       = &(scs->mrp_ctrls);

    pcs->similar_brightness_refs = get_similar_ref_brightness(pcs);
    if (scs->mrp_ctrls.safe_limit_nref == 2 && pcs->slice_type == B_SLICE && pcs->hierarchical_levels > 0 &&
        (pcs->temporal_layer_index >= pcs->hierarchical_levels - 1)) {
        if (pcs->similar_brightness_refs) {
            // TODO: The ref list counts should not be updated after set_all_ref_frame_type()
            pcs->ref_list0_count_try = MIN(pcs->ref_list0_count_try, 1);
            pcs->ref_list1_count_try = MIN(pcs->ref_list1_count_try, 1);
        }
    }
    //get a new ME data buffer
    if (pcs->me_data_wrapper == NULL) {
        svt_get_empty_object(ctx->me_fifo_ptr, &me_wrapper);
        pcs->me_data_wrapper = me_wrapper;
        pcs->pa_me_data      = (MotionEstimationData*)me_wrapper->object_ptr;
        me_update_param(pcs->pa_me_data, scs);
    }

    uint8_t ref_count_used_list0 = MAX(
        mrp_ctrl->sc_base_ref_list0_count,
        MAX(mrp_ctrl->base_ref_list0_count,
            MAX(mrp_ctrl->sc_non_base_ref_list0_count, mrp_ctrl->non_base_ref_list0_count)));

    uint8_t ref_count_used_list1 = MAX(
        mrp_ctrl->sc_base_ref_list1_count,
        MAX(mrp_ctrl->base_ref_list1_count,
            MAX(mrp_ctrl->sc_non_base_ref_list1_count, mrp_ctrl->non_base_ref_list1_count)));

    uint8_t max_ref_to_alloc, max_cand_to_alloc;

    svt_aom_get_max_allocated_me_refs(
        ref_count_used_list0, ref_count_used_list1, &max_ref_to_alloc, &max_cand_to_alloc);

    pcs->pa_me_data->max_cand = max_cand_to_alloc;
    pcs->pa_me_data->max_refs = max_ref_to_alloc;
    pcs->pa_me_data->max_l0   = ref_count_used_list0;

    //****************************************************
    // Picture resizing for super-res tool
    //****************************************************

    // Scale picture if super-res is used
    // Handle SUPERRES_FIXED and SUPERRES_RANDOM modes here.
    // SUPERRES_QTHRESH and SUPERRES_AUTO modes are handled in rate control process because these modes depend on qindex
    if (scs->static_config.pass == ENC_SINGLE_PASS) {
        if (scs->static_config.resize_mode > RESIZE_NONE || scs->static_config.superres_mode == SUPERRES_FIXED ||
            scs->static_config.superres_mode == SUPERRES_RANDOM) {
            svt_aom_init_resize_picture(scs, pcs);
        }
    }
    bool super_res_off = pcs->frame_superres_enabled == false && scs->static_config.resize_mode == RESIZE_NONE;
    svt_aom_set_gm_controls(pcs, svt_aom_derive_gm_level(pcs, super_res_off));
    pcs->me_processed_b64_count = 0;

    // NB: overlay frames should be non-ref
    // Before sending pics out to pic mgr, ensure that pic mgr can handle them
    if (pcs->is_ref) {
        svt_block_on_semaphore(scs->ref_buffer_available_semaphore);
    }

    for (uint32_t segment_index = 0; segment_index < pcs->me_segments_total_count; ++segment_index) {
        // Get Empty Results Object
        svt_get_empty_object(ctx->picture_decision_results_output_fifo_ptr, &out_results_wrapper);

        PictureDecisionResults* out_results = (PictureDecisionResults*)out_results_wrapper->object_ptr;
        out_results->pcs_wrapper            = pcs->p_pcs_wrapper_ptr;
        out_results->segment_index          = segment_index;
        out_results->task_type              = TASK_PAME;
        //Post the Full Results Object
        svt_post_full_object(out_results_wrapper);
    }
}

/***************************************************************************************************
* Store the pcs pointers in the gf group, set the gf_interval and gf_update_due
***************************************************************************************************/
void store_gf_group(PictureParentControlSet* pcs, PictureDecisionContext* ctx, uint32_t mg_size) {
    if (pcs->slice_type == I_SLICE || (!svt_aom_is_delayed_intra(pcs) && pcs->temporal_layer_index == 0) ||
        svt_aom_is_incomp_mg_frame(pcs)) {
        if (svt_aom_is_delayed_intra(pcs)) {
            pcs->gf_group[0] = pcs;
            svt_memcpy(&pcs->gf_group[1], ctx->mg_pictures_array, mg_size * sizeof(PictureParentControlSet*));
            pcs->gf_interval = 1 + mg_size;
        } else {
            if (svt_aom_is_incomp_mg_frame(pcs) && mg_size > 0 && ctx->mg_pictures_array[mg_size - 1]->idr_flag) {
                mg_size = MAX(0, (int)mg_size - 1);
            }
            svt_memcpy(&pcs->gf_group[0], ctx->mg_pictures_array, mg_size * sizeof(PictureParentControlSet*));
            pcs->gf_interval = mg_size;
        }

        if (pcs->slice_type == I_SLICE && pcs->end_of_sequence_flag) {
            pcs->gf_interval = 1;
            pcs->gf_group[0] = pcs;
        }

        for (int pic_i = 0; pic_i < pcs->gf_interval; ++pic_i) {
            if (pcs->gf_group[pic_i]->slice_type == I_SLICE ||
                (!svt_aom_is_delayed_intra(pcs) && pcs->gf_group[pic_i]->temporal_layer_index == 0) ||
                svt_aom_is_incomp_mg_frame(pcs->gf_group[pic_i])) {
                pcs->gf_group[pic_i]->gf_update_due = 1;
            } else {
                pcs->gf_group[pic_i]->gf_update_due = 0;
            }

            // For P picture that come after I, we need to set the gf_group pictures. It is used later in RC
            if (pcs->slice_type == I_SLICE && svt_aom_is_incomp_mg_frame(pcs->gf_group[pic_i]) &&
                pcs->picture_number < pcs->gf_group[pic_i]->picture_number) {
                pcs->gf_group[pic_i]->gf_interval = pcs->gf_interval - 1;
                svt_memcpy(&pcs->gf_group[pic_i]->gf_group[0],
                           &ctx->mg_pictures_array[1],
                           pcs->gf_group[pic_i]->gf_interval * sizeof(PictureParentControlSet*));
                pcs->gf_group[pic_i]->gf_update_due = 0;
            }
        }
    }
}

#if LAD_MG_PRINT
/* prints content of pre-assignment buffer */
void print_pre_ass_buffer(EncodeContext* ctx, PictureParentControlSet* pcs, uint8_t log) {
    if (log) {
        if (ctx->pre_assignment_buffer_intra_count > 0) {
            SVT_LOG(
                "PRE-ASSIGN INTRA   (%i pictures)  POC:%lld \n", ctx->pre_assignment_buffer_count, pcs->picture_number);
        }
        if (ctx->pre_assignment_buffer_count == (uint32_t)(1 << pcs->scs->static_config.hierarchical_levels)) {
            SVT_LOG("PRE-ASSIGN COMPLETE   (%i pictures)  POC:%lld \n",
                    ctx->pre_assignment_buffer_count,
                    pcs->picture_number);
        }
        if (ctx->pre_assignment_buffer_eos_flag == 1) {
            SVT_LOG(
                "PRE-ASSIGN EOS   (%i pictures)  POC:%lld \n", ctx->pre_assignment_buffer_count, pcs->picture_number);
        }
        if (pcs->pred_structure == LOW_DELAY) {
            SVT_LOG(
                "PRE-ASSIGN LD   (%i pictures)  POC:%lld \n", ctx->pre_assignment_buffer_count, pcs->picture_number);
        }

        SVT_LOG("\n Pre-Assign(%i):  ", ctx->pre_assignment_buffer_count);
        for (uint32_t pic = 0; pic < ctx->pre_assignment_buffer_count; pic++) {
            PictureParentControlSet* pcs = (PictureParentControlSet*)ctx->pre_assignment_buffer[pic]->object_ptr;
            SVT_LOG("%ld ", pcs->picture_number);
        }
        SVT_LOG("\n");
    }
}
#endif

static PaReferenceEntry* search_ref_in_ref_queue_pa(EncodeContext* enc_ctx, uint64_t ref_poc) {
    PaReferenceEntry* ref_entry_ptr = NULL;
    for (uint8_t i = 0; i < REF_FRAMES; i++) {
        ref_entry_ptr = enc_ctx->pd_dpb[i];

        if (ref_entry_ptr && ref_entry_ptr->picture_number == ref_poc) {
            return ref_entry_ptr;
        }
    }

    return NULL;
}

/*
 * Copy TF params: sps -> pcs
 */
static void copy_tf_params(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    // Map TF settings sps -> pcs
    if (scs->static_config.pred_structure != RANDOM_ACCESS) {
        if (pcs->slice_type != I_SLICE && pcs->temporal_layer_index == 0) {
            pcs->tf_ctrls = scs->tf_params_per_type[1];
        } else {
            pcs->tf_ctrls.enabled = 0;
        }
        // When the hierarchical level reaches zero during flexible S-Frame insertion, tf_pic_arr_cnt is reset to zero upon mini-GOP closure.
        // Add additional protection for TF handling.
        if (IS_SFRAME_FLEXIBLE_INSERT(scs->static_config.sframe_mode) && pcs->tf_ctrls.enabled &&
            ctx->tf_pic_arr_cnt == 0) {
            pcs->tf_ctrls.enabled = 0;
        }

        return;
    }
    // Don't perform TF for overlay pics or pics in the highest layer (relevant for 2L)
    if (pcs->is_overlay || pcs->temporal_layer_index == pcs->hierarchical_levels) {
        pcs->tf_ctrls.enabled = 0;
    } else if (svt_aom_is_delayed_intra(pcs)) {
        pcs->tf_ctrls = scs->tf_params_per_type[0];
    } else if (pcs->temporal_layer_index == 0) { // BASE
        pcs->tf_ctrls = scs->tf_params_per_type[1];
    } else if (pcs->temporal_layer_index == 1) { // L1
        pcs->tf_ctrls = scs->tf_params_per_type[2];
    } else {
        pcs->tf_ctrls.enabled = 0;
    }
}

void svt_aom_is_screen_content(PictureParentControlSet* pcs);
void svt_aom_is_screen_content_antialiasing_aware(PictureParentControlSet* pcs);

/*
* Update the list0 count try and the list1 count try based on the Enc-Mode, whether BASE or not, whether SC or not
*/
void update_count_try(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    MrpCtrls* mrp_ctrl = &scs->mrp_ctrls;
    if (pcs->sc_class1) {
        if (pcs->temporal_layer_index == 0) {
            pcs->ref_list0_count_try = MIN(pcs->ref_list0_count, mrp_ctrl->sc_base_ref_list0_count);
            pcs->ref_list1_count_try = MIN(pcs->ref_list1_count, mrp_ctrl->sc_base_ref_list1_count);
        } else {
            pcs->ref_list0_count_try = MIN(pcs->ref_list0_count, mrp_ctrl->sc_non_base_ref_list0_count);
            pcs->ref_list1_count_try = MIN(pcs->ref_list1_count, mrp_ctrl->sc_non_base_ref_list1_count);
        }
    } else {
        if (pcs->temporal_layer_index == 0) {
            pcs->ref_list0_count_try = MIN(pcs->ref_list0_count, mrp_ctrl->base_ref_list0_count);
            pcs->ref_list1_count_try = MIN(pcs->ref_list1_count, mrp_ctrl->base_ref_list1_count);
        } else {
            pcs->ref_list0_count_try = MIN(pcs->ref_list0_count, mrp_ctrl->non_base_ref_list0_count);
            pcs->ref_list1_count_try = MIN(pcs->ref_list1_count, mrp_ctrl->non_base_ref_list1_count);
        }
    }
}

/*
* Switch frame's pcs->dpb_order_hint[8] will be packed to uncompressed_header as ref_order_hint[8], ref to spec 5.9.2.
*/
static void update_sframe_ref_order_hint(PictureParentControlSet* ppcs, PictureDecisionContext* pd_ctx) {
    assert(sizeof(ppcs->dpb_order_hint) == sizeof(pd_ctx->ref_order_hint));
    if (ppcs->pred_structure == LOW_DELAY) {
        for (int32_t i = 0; i < REF_FRAMES; i++) {
            // dpd_order_hint should be updated with relative position of key frame
            ppcs->dpb_order_hint[i] = (uint32_t)(pd_ctx->ref_order_hint[i] - pd_ctx->key_poc);
        }
    } else {
        memcpy(ppcs->dpb_order_hint, pd_ctx->ref_order_hint, sizeof(ppcs->dpb_order_hint));
    }
    if (ppcs->av1_ref_signal.refresh_frame_mask != 0) {
        const uint32_t cur_order_hint = ppcs->picture_number %
            ((uint64_t)1 << (ppcs->scs->seq_header.order_hint_info.order_hint_bits));
        for (int32_t i = 0; i < REF_FRAMES; i++) {
            if ((ppcs->av1_ref_signal.refresh_frame_mask >> i) & 1) {
                pd_ctx->ref_order_hint[i] = cur_order_hint;
            }
        }
    }
}

/*****************************************************************
* Update the RC param queue
* Set the size of the previous Gop/param, Check if all the frames in gop are processed, if yes reset
* Increament the head index to assign a new spot in the queue for the new gop
*****************************************************************/
static void update_rc_param_queue(PictureParentControlSet* ppcs, EncodeContext* enc_cxt) {
    if (ppcs->idr_flag == true && ppcs->picture_number > 0) {
        svt_block_on_mutex(enc_cxt->rc_param_queue_mutex);
        // Set the size of the previous Gop/param
        enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->size =
            (int32_t)(ppcs->picture_number - enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->first_poc);
        // Check if all the frames in gop are processed, if yes reset
        if (enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->size ==
            enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->processed_frame_number) {
            enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->size                   = -1;
            enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->processed_frame_number = 0;
        }
        // Increament the head index to assign a new spot in the queue for the new gop
        enc_cxt->rc_param_queue_head_index = (enc_cxt->rc_param_queue_head_index == PARALLEL_GOP_MAX_NUMBER - 1)
            ? 0
            : enc_cxt->rc_param_queue_head_index + 1;
        svt_aom_assert_err(enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->size == -1,
                           "The head in rc paramqueue is not empty");
        enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index]->first_poc = ppcs->picture_number;
        svt_release_mutex(enc_cxt->rc_param_queue_mutex);
    }
    // Store the pointer to the right spot in the RC param queue under PCS
    ppcs->rate_control_param_ptr = enc_cxt->rc_param_queue[enc_cxt->rc_param_queue_head_index];
}

/****************************************************************************************
* set_layer_depth()
* Set the layer depth per frame based on frame type, temporal layer
****************************************************************************************/
static void set_layer_depth(PictureParentControlSet* ppcs) {
    // SequenceControlSet *scs = ppcs->scs;
    if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
        ppcs->layer_depth = 0;
    } else {
        ppcs->layer_depth = ppcs->temporal_layer_index + 1;
    }
}

/****************************************************************************************
* set_frame_update_type()
* Set the update type per frame based on frame type, temporal layer and prediction structure
* For Low delay, there is a special case where all non key frames are treated as LF_UPDATE.
* Every MAX_GF_INTERVAL frames, update type is set to GF_UPDATE
****************************************************************************************/
static void set_frame_update_type(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs = ppcs->scs;
    if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
        ppcs->update_type = SVT_AV1_KF_UPDATE;
    } else if (scs->max_temporal_layers > 0 && ppcs->pred_structure != LOW_DELAY) {
        if (ppcs->temporal_layer_index == 0) {
            ppcs->update_type = SVT_AV1_ARF_UPDATE;
        } else if (ppcs->temporal_layer_index == ppcs->hierarchical_levels) {
            ppcs->update_type = SVT_AV1_LF_UPDATE;
        } else {
            ppcs->update_type = SVT_AV1_INTNL_ARF_UPDATE;
        }
    } else if (ppcs->pred_structure == LOW_DELAY && (ppcs->frame_offset % MAX_GF_INTERVAL) == 0) {
        ppcs->update_type = SVT_AV1_GF_UPDATE;
    } else {
        ppcs->update_type = SVT_AV1_LF_UPDATE;
    }
}

static void set_gf_group_param(PictureParentControlSet* ppcs) {
    set_frame_update_type(ppcs);
    set_layer_depth(ppcs);
}

static void process_first_pass(SequenceControlSet* scs, EncodeContext* enc_ctx) {
    for (unsigned int window_index = 0; window_index < scs->scd_delay + 1; window_index++) {
        unsigned int entry_index = QUEUE_GET_NEXT_SPOT(enc_ctx->picture_decision_reorder_queue_head_index,
                                                       window_index,
                                                       enc_ctx->picture_decision_reorder_queue_size);
        PictureDecisionReorderEntry* first_pass_queue_entry = enc_ctx->picture_decision_reorder_queue[entry_index];
        if (first_pass_queue_entry->ppcs_wrapper == NULL) {
            break;
        }

        PictureParentControlSet* first_pass_pcs = (PictureParentControlSet*)
                                                      first_pass_queue_entry->ppcs_wrapper->object_ptr;
        if (!first_pass_pcs->first_pass_done) {
            first_pass_frame_end_one_pass(first_pass_pcs);
            first_pass_pcs->first_pass_done = 1;
        }
    }
}

// Check if have enough frames to do scene change detection or if the EOS has been reached
static void check_window_availability(SequenceControlSet* scs, EncodeContext* enc_ctx, PictureParentControlSet* pcs,
                                      PictureDecisionReorderEntry* queue_entry, bool* window_avail, bool* eos_reached) {
    *eos_reached  = ((PictureParentControlSet*)(queue_entry->ppcs_wrapper->object_ptr))->end_of_sequence_flag == true;
    *window_avail = true;

    unsigned int previous_entry_index = QUEUE_GET_PREVIOUS_SPOT(enc_ctx->picture_decision_reorder_queue_head_index,
                                                                enc_ctx->picture_decision_reorder_queue_size);
    memset(pcs->pd_window, 0, (2 + scs->scd_delay) * sizeof(PictureParentControlSet*));
    //for poc 0, ignore previous frame check
    if (queue_entry->picture_number > 0 &&
        enc_ctx->picture_decision_reorder_queue[previous_entry_index]->ppcs_wrapper == NULL) {
        *window_avail = false;
    } else {
        //TODO: risk of a race condition accessing prev(pcs0 is released, and pcs1 still doing sc).
        //Actually we don't need to keep prev, just keep previous copy of histograms.
        pcs->pd_window[0] = queue_entry->picture_number > 0
            ? (PictureParentControlSet*)enc_ctx->picture_decision_reorder_queue[previous_entry_index]
                  ->ppcs_wrapper->object_ptr
            : NULL;
        pcs->pd_window[1] = (PictureParentControlSet*)enc_ctx
                                ->picture_decision_reorder_queue[enc_ctx->picture_decision_reorder_queue_head_index]
                                ->ppcs_wrapper->object_ptr;
        for (unsigned int window_index = 0; window_index < scs->scd_delay; window_index++) {
            unsigned int entry_index = QUEUE_GET_NEXT_SPOT(enc_ctx->picture_decision_reorder_queue_head_index,
                                                           window_index + 1,
                                                           enc_ctx->picture_decision_reorder_queue_size);
            if (enc_ctx->picture_decision_reorder_queue[entry_index]->ppcs_wrapper == NULL) {
                *window_avail = false;
                break;
            } else if (((PictureParentControlSet*)(enc_ctx->picture_decision_reorder_queue[entry_index]
                                                       ->ppcs_wrapper->object_ptr))
                           ->end_of_sequence_flag == true) {
                *window_avail = false;
                *eos_reached  = true;
                break;
            } else {
                pcs->pd_window[2 + window_index] = (PictureParentControlSet*)enc_ctx
                                                       ->picture_decision_reorder_queue[entry_index]
                                                       ->ppcs_wrapper->object_ptr;
            }
        }
    }
}

// Perform scene change detection and update relevant signals
static void perform_scene_change_detection(SequenceControlSet* scs, PictureParentControlSet* pcs,
                                           PictureDecisionContext* ctx) {
    if (scs->static_config.scene_change_detection) {
        pcs->scene_change_flag = scene_transition_detector(ctx, scs, (PictureParentControlSet**)pcs->pd_window);

    } else {
        pcs->scene_change_flag = false;

        if (scs->vq_ctrls.sharpness_ctrls.scene_transition &&
            (ctx->transition_detected == -1 || ctx->transition_detected == 0)) {
            ctx->transition_detected = scene_transition_detector(ctx, scs, (PictureParentControlSet**)pcs->pd_window);
        }
    }

    pcs->cra_flag = (pcs->scene_change_flag == true) ? true : pcs->cra_flag;

    // Store scene change in context
    ctx->is_scene_change_detected = pcs->scene_change_flag;
}

// Copy current pic's histogram to temporary buffer to be used by next input pic (N + 1) for scene change detection
static void copy_histograms(PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    for (unsigned int region_in_picture_width_index = 0; region_in_picture_width_index < MAX_NUMBER_OF_REGIONS_IN_WIDTH;
         region_in_picture_width_index++) {
        for (unsigned int region_in_picture_height_index = 0;
             region_in_picture_height_index < MAX_NUMBER_OF_REGIONS_IN_HEIGHT;
             region_in_picture_height_index++) {
            svt_memcpy(&(ctx->prev_picture_histogram[region_in_picture_width_index][region_in_picture_height_index][0]),
                       &(pcs->picture_histogram[region_in_picture_width_index][region_in_picture_height_index][0]),
                       HISTOGRAM_NUMBER_OF_BINS * sizeof(uint32_t));

            ctx->prev_average_intensity_per_region[region_in_picture_width_index][region_in_picture_height_index] =
                pcs->average_intensity_per_region[region_in_picture_width_index][region_in_picture_height_index];
        }
    }
}

// Decide what mini-gop sizes to use and init the relevant fields
static void set_mini_gop_structure(SequenceControlSet* scs, EncodeContext* enc_ctx, PictureParentControlSet* pcs,
                                   PictureDecisionContext* ctx) {
    uint32_t next_mg_hierarchical_levels = scs->static_config.hierarchical_levels;
    // Overwrite next_mg_hierarchical_levels when an S-Frame needs to modify the mini-GOP size.
    if (ctx->sframe_hier_lvls != (int32_t)scs->static_config.hierarchical_levels) {
        next_mg_hierarchical_levels = ctx->sframe_hier_lvls;
    }
    if (ctx->enable_startup_mg) {
        next_mg_hierarchical_levels = scs->static_config.startup_mg_size;
    }
    // Initialize Picture Block Params
    ctx->mini_gop_start_index[0] = 0;
    ctx->mini_gop_end_index[0]   = enc_ctx->pre_assignment_buffer_count - 1;
    ctx->mini_gop_length[0]      = enc_ctx->pre_assignment_buffer_count;

    ctx->mini_gop_hierarchical_levels[0]           = next_mg_hierarchical_levels;
    ctx->mini_gop_intra_count[0]                   = enc_ctx->pre_assignment_buffer_intra_count;
    ctx->mini_gop_idr_count[0]                     = enc_ctx->pre_assignment_buffer_idr_count;
    ctx->total_number_of_mini_gops                 = 1;
    enc_ctx->previous_mini_gop_hierarchical_levels = (pcs->picture_number == 0)
        ? next_mg_hierarchical_levels
        : enc_ctx->previous_mini_gop_hierarchical_levels;
    enc_ctx->mini_gop_cnt_per_gop = (enc_ctx->pre_assignment_buffer_idr_count) ? 0 : enc_ctx->mini_gop_cnt_per_gop + 1;
    assert(IMPLIES(enc_ctx->pre_assignment_buffer_intra_count == enc_ctx->pre_assignment_buffer_count,
                   enc_ctx->pre_assignment_buffer_count == 1));
    // TODO: Why special case? Why no check on enc_ctx->pre_assignment_buffer_count > 1
    if (next_mg_hierarchical_levels == 1) {
        //minigop 2 case
        ctx->mini_gop_start_index[ctx->total_number_of_mini_gops] = 0;
        ctx->mini_gop_end_index[ctx->total_number_of_mini_gops]   = enc_ctx->pre_assignment_buffer_count - 1;
        ctx->mini_gop_length[ctx->total_number_of_mini_gops]      = enc_ctx->pre_assignment_buffer_count -
            ctx->mini_gop_start_index[ctx->total_number_of_mini_gops];
        ctx->mini_gop_hierarchical_levels[ctx->total_number_of_mini_gops] = 2;
    }
    // In RA, if the only picture is an I_SLICE, use default settings (set above). If treat the solo I_SLICE
    // as a regular MG, you will change the hierarchical_levels to the minimum.
    // For low-delay pred strucutres, pre_assignment_buffer_count will be 1, but no need to change the default
    // hierarchical levels.
    else if (enc_ctx->pre_assignment_buffer_count > 1 ||
             (!enc_ctx->pre_assignment_buffer_intra_count && scs->static_config.pred_structure == RANDOM_ACCESS)) {
        initialize_mini_gop_activity_array(scs, pcs, enc_ctx, ctx);

        generate_picture_window_split(ctx, enc_ctx);

        handle_incomplete_picture_window_map(next_mg_hierarchical_levels, ctx, enc_ctx);
    }

    get_pred_struct_for_all_frames(ctx, enc_ctx);
}

// Set whether the picture is to be considered as SC; for single-threaded mode we perform SC detection here
static void perform_sc_detection(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    if (pcs->slice_type == I_SLICE) {
        // If running multi-threaded mode, perform SC detection in svt_aom_picture_analysis_kernel, else in svt_aom_picture_decision_kernel
        if (scs->static_config.level_of_parallelism == 1) {
            switch (scs->static_config.screen_content_mode) {
            case 0:
                pcs->sc_class0 = pcs->sc_class1 = pcs->sc_class2 = pcs->sc_class3 = pcs->sc_class4 = 0;
                break;
            case 1:
                pcs->sc_class0 = pcs->sc_class1 = pcs->sc_class2 = pcs->sc_class3 = pcs->sc_class4 = 1;
                break;
            case 2:
                // SC Detection is OFF for 4K and higher
                if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
                    svt_aom_is_screen_content(pcs);
                }
                break;
            case 3:
                svt_aom_is_screen_content_antialiasing_aware(pcs);
                break;
            }
        }
        ctx->last_i_picture_sc_class0 = pcs->sc_class0;
        ctx->last_i_picture_sc_class1 = pcs->sc_class1;
        ctx->last_i_picture_sc_class2 = pcs->sc_class2;
        ctx->last_i_picture_sc_class3 = pcs->sc_class3;
        ctx->last_i_picture_sc_class4 = pcs->sc_class4;

    } else {
        pcs->sc_class0 = ctx->last_i_picture_sc_class0;
        pcs->sc_class1 = ctx->last_i_picture_sc_class1;
        pcs->sc_class2 = ctx->last_i_picture_sc_class2;
        pcs->sc_class3 = ctx->last_i_picture_sc_class3;
        pcs->sc_class4 = ctx->last_i_picture_sc_class4;
    }
}

// Update pred struct info and pic type for non-overlay pictures
static void update_pred_struct_and_pic_type(SequenceControlSet* scs, EncodeContext* enc_ctx,
                                            PictureParentControlSet* pcs, PictureDecisionContext* ctx,
                                            unsigned int mini_gop_index, bool pre_assignment_buffer_first_pass_flag,
                                            SliceType* picture_type, PredictionStructureEntry** pred_position_ptr) {
    (void)scs;
    // Keep track of the mini GOP size to which the input picture belongs - needed @ PictureManagerProcess()
    pcs->pre_assignment_buffer_count = ctx->mini_gop_length[mini_gop_index];

    // Update the Pred Structure if cutting short a Random Access period
    if (is_pic_cutting_short_ra_mg(ctx, pcs, mini_gop_index)) {
        // Correct the Pred Index before switching structures
        if (pre_assignment_buffer_first_pass_flag == true) {
            enc_ctx->pred_struct_position -= pcs->pred_struct_ptr->init_pic_index;
        }
        pcs->pred_struct_ptr = svt_aom_get_prediction_structure(
            enc_ctx->prediction_structure_group_ptr, LOW_DELAY, pcs->hierarchical_levels);
        *picture_type        = B_SLICE;
        ctx->cut_short_ra_mg = 1;
    } else {
        // Set the Picture Type
        *picture_type = (pcs->idr_flag || pcs->cra_flag) ? I_SLICE : B_SLICE;
    }
    // If mini GOP switch, reset position
    if (pcs->init_pred_struct_position_flag) {
        enc_ctx->pred_struct_position = pcs->pred_struct_ptr->init_pic_index;
    }

    // If Intra, reset position
    if (pcs->idr_flag == true) {
        enc_ctx->pred_struct_position = pcs->pred_struct_ptr->init_pic_index;
    } else if (pcs->cra_flag == true &&
               ctx->mini_gop_length[mini_gop_index] < pcs->pred_struct_ptr->pred_struct_entry_count) {
        enc_ctx->pred_struct_position = pcs->pred_struct_ptr->init_pic_index;
    } else if (enc_ctx->elapsed_non_cra_count == 0) {
        // If we are the picture directly after a CRA, we have to not use references that violate the CRA
        enc_ctx->pred_struct_position = pcs->pred_struct_ptr->init_pic_index + 1;
    }
    // Else, Increment the position normally
    else {
        ++enc_ctx->pred_struct_position;
    }
    // The poc number of the latest IDR picture is stored so that last_idr_picture (present in PCS) for the incoming pictures can be updated.
    // The last_idr_picture is used in reseting the poc (in entropy coding) whenever IDR is encountered.
    // Note IMP: This logic only works when display and decode order are the same. Currently for Random Access, IDR is inserted (similar to CRA) by using trailing P pictures (low delay fashion) and breaking prediction structure.
    // Note: When leading P pictures are implemented, this logic has to change..
    if (pcs->idr_flag == true) {
        enc_ctx->last_idr_picture = pcs->picture_number;
    } else {
        pcs->last_idr_picture = enc_ctx->last_idr_picture;
    }
    // Cycle the PredStructPosition if its overflowed
    enc_ctx->pred_struct_position = (enc_ctx->pred_struct_position == pcs->pred_struct_ptr->pred_struct_entry_count)
        ? enc_ctx->pred_struct_position - pcs->pred_struct_ptr->pred_struct_entry_count
        : enc_ctx->pred_struct_position;

    *pred_position_ptr = pcs->pred_struct_ptr->pred_struct_entry_ptr_array[enc_ctx->pred_struct_position];
}

static uint32_t get_pic_idx_in_mg(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* ctx,
                                  uint32_t pic_idx, uint32_t mini_gop_index) {
    uint32_t pic_idx_in_mg = 0;
    if (scs->static_config.pred_structure == RANDOM_ACCESS) {
        pic_idx_in_mg = pic_idx - ctx->mini_gop_start_index[mini_gop_index];
    } else {
        uint64_t distance_to_last_idr = pcs->picture_number - scs->enc_ctx->last_idr_picture;
        // For low delay P or low delay b case, get the the picture_index by mini_gop size
        if (scs->static_config.intra_period_length >= 0) {
            pic_idx_in_mg = (distance_to_last_idr == 0)
                ? 0
                : (uint32_t)(((distance_to_last_idr - 1) % (scs->static_config.intra_period_length + 1)) %
                             pcs->pred_struct_ptr->pred_struct_entry_count);
        } else {
            // intra-period=-1 case, no gop
            pic_idx_in_mg = (distance_to_last_idr == 0)
                ? 0
                : (uint32_t)((distance_to_last_idr - 1) % pcs->pred_struct_ptr->pred_struct_entry_count);
        }
        // In S-Frame flexible insertion mode, hierarchical levels are adjusted based on the S-Frame position.
        // Picture indices in the low-delay mini-GOP are calculated from the last saved ARF.
        if (IS_SFRAME_FLEXIBLE_INSERT(scs->static_config.sframe_mode)) {
            pic_idx_in_mg = distance_to_last_idr > ctx->sframe_last_arf
                ? (uint32_t)(distance_to_last_idr - ctx->sframe_last_arf - 1)
                : 0;
        }
        pcs->frame_offset = distance_to_last_idr;
    }

    return pic_idx_in_mg;
}

static void set_ref_frame_sign_bias(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    memset(pcs->av1_cm->ref_frame_sign_bias, 0, 8 * sizeof(int32_t));

    if (scs->seq_header.order_hint_info.enable_order_hint) {
        for (MvReferenceFrame ref_frame = LAST_FRAME; ref_frame <= ALTREF_FRAME; ++ref_frame) {
            pcs->av1_cm->ref_frame_sign_bias[ref_frame] = (get_relative_dist(&scs->seq_header.order_hint_info,
                                                                             pcs->ref_order_hint[ref_frame - 1],
                                                                             (int)pcs->cur_order_hint) <= 0)
                ? 0
                : 1;
        }
    }
}

// Derive settings used to encode the picture
// Both normative (e.g. frame header info) and non-normative (e.g. feature levels) are set here
static void init_pic_settings(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    FrameHeader* frm_hdr        = &pcs->frm_hdr;
    pcs->allow_comp_inter_inter = pcs->slice_type != I_SLICE;
    frm_hdr->reference_mode     = pcs->slice_type == I_SLICE ? (ReferenceMode)0xFF
            : svt_aom_is_incomp_mg_frame(pcs)                ? SINGLE_REFERENCE
                                                             : REFERENCE_MODE_SELECT;

    pcs->av1_cm->mi_cols = pcs->aligned_width >> MI_SIZE_LOG2;
    pcs->av1_cm->mi_rows = pcs->aligned_height >> MI_SIZE_LOG2;

    // Initialize the order hints
    const OrderHintInfo* const order_hint_info = &pcs->scs->seq_header.order_hint_info;
    uint32_t*                  ref_order_hint  = pcs->ref_order_hint;
    for (uint8_t i = 0; i < INTER_REFS_PER_FRAME; ++i) {
        ref_order_hint[i] = pcs->av1_ref_signal.ref_poc_array[i] % (uint64_t)(1 << (order_hint_info->order_hint_bits));
    }
    pcs->cur_order_hint = pcs->picture_number % (uint64_t)(1 << (order_hint_info->order_hint_bits));

    set_ref_frame_sign_bias(scs, pcs);

    // TODO: put this in EbMotionEstimationProcess?
    copy_tf_params(scs, pcs, ctx);
    // TODO: put this in EbMotionEstimationProcess?
    // ME Kernel Multi-Processes Signal(s) derivation
#if TUNE_STILL_IMAGE
    const bool rtc_tune = scs->static_config.rtc;
    const bool allintra = scs->allintra;

    allintra       ? svt_aom_sig_deriv_multi_processes_allintra(scs, pcs)
        : rtc_tune ? svt_aom_sig_deriv_multi_processes_rtc(scs, pcs)
                   : svt_aom_sig_deriv_multi_processes_default(scs, pcs);
#else
    svt_aom_sig_deriv_multi_processes(scs, pcs);
#endif

    update_count_try(scs, pcs);

    if (ctx->transition_detected == 1) {
        if (pcs->temporal_layer_index == 0) {
            pcs->transition_present  = 1;
            ctx->transition_detected = 0;
        }
    }

    if (ctx->list0_only && pcs->slice_type == B_SLICE && pcs->temporal_layer_index == 0) {
        pcs->ref_list1_count_try = 0;
    }
    assert(pcs->ref_list0_count_try <= pcs->ref_list0_count);
    assert(pcs->ref_list1_count_try <= pcs->ref_list1_count);

    // Setup the skip mode syntax, see: spec 5.9.22 - Skip mode params syntax
    svt_av1_setup_skip_mode_allowed(pcs);
    frm_hdr->skip_mode_params.skip_mode_flag = frm_hdr->skip_mode_params.skip_mode_allowed;

    //set the ref frame types used for this picture,
    set_all_ref_frame_type(ctx, pcs, pcs->ref_frame_type_arr, &pcs->tot_ref_frame_types);
}

// Create MG arrays with pics in decode order (ctx->mg_pictures_array) and dispaly order (ctx->mg_pictures_array_disp_order)
// Input is ctx->mg_pictures_array with all pics in the MG sorted by display order
static void store_mg_picture_arrays(PictureDecisionContext* ctx) {
    const unsigned int mg_size = ctx->mg_size;

    // mg_pictures_array arrives in display order, so copy into display order array
    svt_memcpy(ctx->mg_pictures_array_disp_order, ctx->mg_pictures_array, mg_size * sizeof(PictureParentControlSet*));

    // Sort MG pics into decode order
    PictureParentControlSet** mg_pics = &ctx->mg_pictures_array[0];
    for (unsigned int i = 0; i < mg_size - 1; ++i) {
        for (unsigned int j = i + 1; j < mg_size; ++j) {
            if (mg_pics[j]->decode_order < mg_pics[i]->decode_order) {
                PictureParentControlSet* temp = mg_pics[i];
                mg_pics[i]                    = ctx->mg_pictures_array[j];
                mg_pics[j]                    = temp;
            }
        }
    }
}

// return true if the frame is part of an incomplete MG (at the end of a GOP)
bool svt_aom_is_incomp_mg_frame(PictureParentControlSet* pcs) {
    return (pcs->pred_struct_ptr->pred_type == LOW_DELAY && pcs->scs->static_config.pred_structure == RANDOM_ACCESS);
}

static void assign_and_release_pa_refs(EncodeContext* enc_ctx, PictureParentControlSet* pcs,
                                       PictureDecisionContext* ctx) {
    const unsigned int mg_size = ctx->mg_size;
    for (uint32_t pic_i = 0; pic_i < mg_size; ++pic_i) {
        pcs = ctx->mg_pictures_array[pic_i];
        if (pcs->slice_type == B_SLICE) {
            for (REF_FRAME_MINUS1 ref = LAST; ref < ALT + 1; ref++) {
                // hardcode the reference for the overlay frame
                uint64_t ref_poc = pcs->is_overlay ? pcs->picture_number : pcs->av1_ref_signal.ref_poc_array[ref];

                uint8_t list_idx = get_list_idx(ref + 1);
                uint8_t ref_idx  = get_ref_frame_idx(ref + 1);
                svt_block_on_mutex(enc_ctx->pd_dpb_mutex);
                PaReferenceEntry* pa_ref_entry = search_ref_in_ref_queue_pa(enc_ctx, ref_poc);
                assert(pa_ref_entry != NULL);
                // Set the Reference Object
                pcs->ref_pa_pic_ptr_array[list_idx][ref_idx] = pa_ref_entry->input_object_ptr;
                pcs->ref_pic_poc_array[list_idx][ref_idx]    = ref_poc;
                // Increment the PA Reference's liveCount by the number of tiles in the input picture
                svt_object_inc_live_count(pa_ref_entry->input_object_ptr, 1);

                pcs->ref_y8b_array[list_idx][ref_idx] = pa_ref_entry->y8b_wrapper;

                if (pa_ref_entry->y8b_wrapper) {
                    //y8b follows longest life cycle of pa ref and input. so it needs to build on top of live count of pa ref
                    svt_object_inc_live_count(pa_ref_entry->y8b_wrapper, 1);
                }
                svt_release_mutex(enc_ctx->pd_dpb_mutex);
            }
        }

        uint8_t released_pics_idx = 0;

        // If the pic is added to DPB, add to ref list until all frames that use it have had a chance to reference it
        if (pcs->av1_ref_signal.refresh_frame_mask) {
            //assert(!pcs->is_overlay); // is this true?
            //Update the DPB
            for (uint8_t i = 0; i < REF_FRAMES; i++) {
                if ((pcs->av1_ref_signal.refresh_frame_mask >> i) & 1) {
                    svt_block_on_mutex(enc_ctx->pd_dpb_mutex);
                    // Get the current entry at that spot in the DPB
                    PaReferenceEntry* input_entry = enc_ctx->pd_dpb[i];

                    // If DPB entry is occupied, release the current entry
                    if (input_entry->is_valid) {
                        bool still_in_dpb = 0;
                        for (uint8_t j = 0; j < REF_FRAMES; j++) {
                            if (j == i) {
                                continue;
                            }
                            if (enc_ctx->pd_dpb[j]->is_valid &&
                                enc_ctx->pd_dpb[j]->picture_number == input_entry->picture_number) {
                                still_in_dpb = 1;
                            }
                        }
                        if (!still_in_dpb) {
                            pcs->released_pics[released_pics_idx++] = input_entry->decode_order;
                        }

                        // Release the entry at that DPB spot
                        // Release the nominal live_count value
                        svt_release_object(input_entry->input_object_ptr);

                        if (input_entry->y8b_wrapper) {
                            //y8b needs to get decremented at the same time of pa ref
                            svt_release_object(input_entry->y8b_wrapper);
                        }

                        input_entry->input_object_ptr = (EbObjectWrapper*)NULL;
                    }

                    // Update the list entry with the info of the new pic that is replacing the old pic in the DPB
                    // Place Picture in Picture Decision PA Reference Queue
                    input_entry->input_object_ptr = pcs->pa_ref_pic_wrapper;
                    input_entry->picture_number   = pcs->picture_number;
                    input_entry->is_valid         = true;
                    input_entry->decode_order     = pcs->decode_order;
                    input_entry->is_alt_ref       = pcs->is_alt_ref;
                    input_entry->y8b_wrapper      = pcs->y8b_wrapper;

                    svt_object_inc_live_count(input_entry->input_object_ptr, 1);

                    if (input_entry->y8b_wrapper) {
                        //y8b follows longest life cycle of pa ref and input. so it needs to build on top of live count of pa ref
                        svt_object_inc_live_count(input_entry->y8b_wrapper, 1);
                    }
                    svt_release_mutex(enc_ctx->pd_dpb_mutex);
                }
            }
        } else {
            assert(!pcs->is_ref);
        }
        pcs->released_pics_count = released_pics_idx;
    }
}

// Send pictures to TF and ME
static void process_pics(SequenceControlSet* scs, PictureDecisionContext* ctx) {
    PictureParentControlSet* pcs     = NULL; // init'd to quiet build warnings
    const unsigned int       mg_size = ctx->mg_size;
    // Process previous delayed Intra if we have one
    if (ctx->prev_delayed_intra) {
        pcs = ctx->prev_delayed_intra;
        store_gf_group(pcs, ctx, mg_size);
    } else {
        for (uint32_t pic_i = 0; pic_i < mg_size; ++pic_i) {
            pcs = ctx->mg_pictures_array_disp_order[pic_i];
            if (svt_aom_is_delayed_intra(pcs) == false) {
                store_gf_group(pcs, ctx, mg_size);
            }
        }
    }
    //Process previous delayed Intra if we have one
    if (ctx->prev_delayed_intra) {
        pcs                      = ctx->prev_delayed_intra;
        ctx->base_counter        = 0;
        ctx->gm_pp_last_detected = 0;
        pcs->filt_to_unfilt_diff = ctx->filt_to_unfilt_diff = (uint32_t)~0;
        mctf_frame(scs, pcs, ctx);
        ctx->filt_to_unfilt_diff = pcs->slice_type == I_SLICE ? pcs->filt_to_unfilt_diff : ctx->filt_to_unfilt_diff;
    }

    //Do TF loop in display order
    for (uint32_t pic_i = 0; pic_i < mg_size; ++pic_i) {
        pcs = ctx->mg_pictures_array_disp_order[pic_i];

        if (svt_aom_is_delayed_intra(pcs) == false) {
            if (pcs->slice_type == B_SLICE && pcs->temporal_layer_index == 0) {
                pcs->gm_pp_enabled = ctx->base_counter == 0 ? 1 : 0;
                ctx->base_counter  = 1 - ctx->base_counter;
            }

            pcs->filt_to_unfilt_diff = ctx->filt_to_unfilt_diff;
            mctf_frame(scs, pcs, ctx);
            ctx->filt_to_unfilt_diff = pcs->slice_type == I_SLICE ? pcs->filt_to_unfilt_diff : ctx->filt_to_unfilt_diff;
            ctx->gm_pp_last_detected = pcs->gm_pp_enabled ? pcs->gm_pp_detected : ctx->gm_pp_last_detected;
        }
    }

    if (ctx->prev_delayed_intra) {
        pcs                     = ctx->prev_delayed_intra;
        ctx->prev_delayed_intra = NULL;
        send_picture_out(scs, pcs, ctx);
    }

    //split MG into two for these two special cases
    uint8_t ldp_delayi_mg = 0;
    uint8_t ldp_i_eos_mg  = 0;

    // Special considerations for I_SLICE after incomplete MG pic
    if (ctx->mg_pictures_array[mg_size - 1]->slice_type == I_SLICE &&
        svt_aom_is_incomp_mg_frame(ctx->mg_pictures_array[0])) {
        if (svt_aom_is_delayed_intra(ctx->mg_pictures_array[mg_size - 1])) {
            ldp_delayi_mg = 1;
        } else if (ctx->mg_pictures_array[mg_size - 1]->slice_type == I_SLICE &&
                   ctx->mg_pictures_array[mg_size - 1]->end_of_sequence_flag) {
            ldp_i_eos_mg = 1;
        }
    }

    for (uint32_t pic_i = 0; pic_i < mg_size; ++pic_i) {
        pcs = ctx->mg_pictures_array[pic_i];
        if (svt_aom_is_delayed_intra(pcs)) {
            ctx->prev_delayed_intra = pcs;

            if (ldp_delayi_mg) {
                ctx->mg_progress_id++;
            }

            pcs->ext_mg_id   = ctx->mg_progress_id;
            pcs->ext_mg_size = 1;
        } else {
            pcs->ext_mg_id   = ctx->mg_progress_id;
            pcs->ext_mg_size = ldp_delayi_mg || ldp_i_eos_mg ? mg_size - 1 : mg_size;

            if (ldp_i_eos_mg && pcs->slice_type == I_SLICE) {
                ctx->mg_progress_id++;
                pcs->ext_mg_id   = ctx->mg_progress_id;
                pcs->ext_mg_size = 1;
            }
            pcs->gm_pp_detected = ctx->gm_pp_last_detected;
            send_picture_out(scs, pcs, ctx);
        }
    }

    ctx->mg_progress_id++;
}

// update the DPB stored in the PD context
static void update_dpb(PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    Av1RpsNode* av1_rps = &pcs->av1_ref_signal;
    if (av1_rps->refresh_frame_mask) {
        for (int i = 0; i < REF_FRAMES; i++) {
            if ((av1_rps->refresh_frame_mask >> i) & 1) {
                ctx->dpb[i].picture_number       = pcs->picture_number;
                ctx->dpb[i].decode_order         = pcs->decode_order;
                ctx->dpb[i].temporal_layer_index = pcs->temporal_layer_index;
            }
        }
    }
}

static uint32_t calc_ahd_pd(SequenceControlSet* scs, PictureParentControlSet* pcs, PictureDecisionContext* ctx) {
    // accumulative histogram (absolute) differences between the past and current frame
    uint32_t ahd = 0;
    uint32_t region_in_picture_width_index;
    uint32_t region_in_picture_height_index;
    // Loop over regions inside the picture
    for (region_in_picture_width_index = 0;
         region_in_picture_width_index < scs->picture_analysis_number_of_regions_per_width;
         region_in_picture_width_index++) { // loop over horizontal regions
        for (region_in_picture_height_index = 0;
             region_in_picture_height_index < scs->picture_analysis_number_of_regions_per_height;
             region_in_picture_height_index++) { // loop over vertical regions

            for (int bin = 0; bin < HISTOGRAM_NUMBER_OF_BINS; ++bin) {
                ahd += ABS(
                    (int32_t)
                        pcs->picture_histogram[region_in_picture_width_index][region_in_picture_height_index][bin] -
                    (int32_t)ctx
                        ->prev_picture_histogram[region_in_picture_width_index][region_in_picture_height_index][bin]);
            }
        }
    }
    return (ahd);
}

/* Picture Decision Kernel */

/***************************************************************************************************
*
* @brief
*  The Picture Decision process performs multi-picture level decisions, including setting of the prediction structure,
*  setting the picture type and performing scene change detection.
*
* @par Description:
*  Since the prior Picture Analysis process stage is multithreaded, inputs to the Picture Decision Process can arrive
*  out-of-display-order, so a reordering queue is used to enforce processing of pictures in display order. The algorithms
*  employed in the Picture Decision process are dependent on prior pictures’ statistics, so the order in which pictures are
*  processed must be strictly enforced. Additionally, the Picture Decision process uses the reorder queue to hold input pictures
*  until they can be started into the Motion Estimation process while following the proper prediction structure.
*
* @param[in] Pictures
*  The Picture Decision Process takes images spontaneously as they arive and perform multi-picture level decisions,
*  including setting of the picture structure, setting the picture type and scene change detection.
*
* @param[out] Picture Control Set
*  Picture Control Set with fully available Picture Analysis Reference List
*
* @remarks
*  For Low Delay Sequences, pictures are started into the encoder pipeline immediately.
*
*  For Random Access Sequences, pictures are held for up to a PredictionStructurePeriod
*    in order to determine if a Scene Change or Intra Frame is forthcoming. Either of
*    those events (and additionally a End of Sequence Flag) will change the expected
*    prediction structure.
*
*  Below is an example worksheet for how Intra Flags and Scene Change Flags interact
*    together to affect the prediction structure.
*
*  The base prediction structure for this example is a 3-Level Hierarchical Random Access,
*    Single Reference Prediction Structure:
*
*        b   b
*       / \ / \
*      /   b   \
*     /   / \   \
*    I-----------b
*
*  From this base structure, the following RPS positions are derived:
*
*    p   p       b   b       p   p
*     \   \     / \ / \     /   /
*      P   \   /   b   \   /   P
*       \   \ /   / \   \ /   /
*        ----I-----------b----
*
*    L L L   I  [ Normal ]   T T T
*    2 1 0   n               0 1 2
*            t
*            r
*            a
*
*  The RPS is composed of Leading Picture [L2-L0], Intra (CRA), Base/Normal Pictures,
*    and Trailing Pictures [T0-t2]. Generally speaking, Leading Pictures are useful
*    for handling scene changes without adding extraneous I-pictures and the Trailing
*    pictures are useful for terminating GOPs.
*
*  Here is a table of possible combinations of pictures needed to handle intra and
*    scene changes happening in quick succession.
*
*        Distance to scene change ------------>
*
*                  0              1                 2                3+
*   I
*   n
*   t   0        I   I           n/a               n/a              n/a
*   r
*   a              p              p
*                   \            /
*   P   1        I   I          I   I              n/a              n/a
*   e
*   r               p                               p
*   i                \                             /
*   o            p    \         p   p             /   p
*   d             \    \       /     \           /   /
*       2     I    -----I     I       I         I----    I          n/a
*   |
*   |            p   p           p   p            p   p            p   p
*   |             \   \         /     \          /     \          /   /
*   |              P   \       /   p   \        /   p   \        /   P
*   |               \   \     /     \   \      /   /     \      /   /
*   V   3+   I       ----I   I       ----I    I----       I    I----       I
*
*   The table is interpreted as follows:
*
*   If there are no SCs or Intras encountered for a PredPeriod, then the normal
*     prediction structure is applied.
*
*   If there is an intra in the PredPeriod, then one of the above combinations of
*     Leading and Trailing pictures is used.  If there is no scene change, the last
*     valid column consisting of Trailing Pictures only is used.  However, if there
*     is an upcoming scene change before the next intra, then one of the above patterns
*     is used. In the case of End of Sequence flags, only the last valid column of Trailing
*     Pictures is used. The intention here is that any combination of Intra Flag and Scene
*     Change flag can be coded.
***************************************************************************************************/
void* svt_aom_picture_decision_kernel(void* input_ptr) {
    EbThreadContext*        thread_ctx = (EbThreadContext*)input_ptr;
    PictureDecisionContext* ctx        = (PictureDecisionContext*)thread_ctx->priv;

    PictureParentControlSet* pcs;

    EncodeContext*          enc_ctx;
    SequenceControlSet*     scs;
    EbObjectWrapper*        in_results_wrapper_ptr;
    PictureAnalysisResults* in_results_ptr;

    PredictionStructureEntry* pred_position_ptr;

    PictureDecisionReorderEntry* queue_entry_ptr;

    unsigned int pic_idx;
    int64_t      current_input_poc = -1;

    for (;;) {
        // Get Input Full Object
        EB_GET_FULL_OBJECT(ctx->picture_analysis_results_input_fifo_ptr, &in_results_wrapper_ptr);

        in_results_ptr      = (PictureAnalysisResults*)in_results_wrapper_ptr->object_ptr;
        pcs                 = (PictureParentControlSet*)in_results_ptr->pcs_wrapper->object_ptr;
        scs                 = pcs->scs;
        enc_ctx             = scs->enc_ctx;
        const bool allintra = scs->allintra;
        // Input Picture Analysis Results into the Picture Decision Reordering Queue
        // Since the prior Picture Analysis processes stage is multithreaded, inputs to the Picture Decision Process
        // can arrive out-of-display-order, so a the Picture Decision Reordering Queue is used to enforce processing of
        // pictures in display order
        if (!pcs->is_overlay) {
            int queue_entry_index =
                (int)(pcs->picture_number -
                      enc_ctx->picture_decision_reorder_queue[enc_ctx->picture_decision_reorder_queue_head_index]
                          ->picture_number);
            queue_entry_index += enc_ctx->picture_decision_reorder_queue_head_index;
            queue_entry_index = (queue_entry_index > (int)(enc_ctx->picture_decision_reorder_queue_size - 1))
                ? queue_entry_index - (int)enc_ctx->picture_decision_reorder_queue_size
                : queue_entry_index;
            queue_entry_ptr   = enc_ctx->picture_decision_reorder_queue[queue_entry_index];
            if (queue_entry_ptr->ppcs_wrapper != NULL) {
                CHECK_REPORT_ERROR_NC(enc_ctx->app_callback_ptr, EB_ENC_PD_ERROR8);
            } else {
                queue_entry_ptr->ppcs_wrapper   = in_results_ptr->pcs_wrapper;
                queue_entry_ptr->picture_number = pcs->picture_number;
            }

            pcs->pic_decision_reorder_queue_idx = queue_entry_index;
            pcs->first_pass_done                = 0;
        }
        // Process the head of the Picture Decision Reordering Queue (Entry N)
        // The Picture Decision Reordering Queue should be parsed in the display order to be able to construct a pred structure
        queue_entry_ptr = enc_ctx->picture_decision_reorder_queue[enc_ctx->picture_decision_reorder_queue_head_index];

        while (queue_entry_ptr->ppcs_wrapper != NULL) {
            if (scs->lap_rc) {
                process_first_pass(scs, enc_ctx);
            }

            pcs = (PictureParentControlSet*)queue_entry_ptr->ppcs_wrapper->object_ptr;
            bool window_avail, eos_reached;
            check_window_availability(scs, enc_ctx, pcs, queue_entry_ptr, &window_avail, &eos_reached);

            if (!allintra) {
                pcs->ahd_error = (uint32_t)~0;
                if (window_avail == true && queue_entry_ptr->picture_number > 0 && scs->calc_hist) {
                    pcs->ahd_error = calc_ahd_pd(scs, pcs, ctx);
                }
                // If the relevant frames are available, perform scene change detection
                if (window_avail == true && queue_entry_ptr->picture_number > 0) {
                    perform_scene_change_detection(scs, pcs, ctx);
                }
            }

            // If the required lookahead frames aren't available, and we haven't reached EOS, must wait for more frames before continuing
            if (!window_avail && !eos_reached) {
                break;
            }

            // Place the PCS into the Pre-Assignment Buffer
            // The Pre-Assignment Buffer is used to store a whole pre-structure
            enc_ctx->pre_assignment_buffer[enc_ctx->pre_assignment_buffer_count] = queue_entry_ptr->ppcs_wrapper;

            // Set the POC Number
            pcs->picture_number                 = ++current_input_poc;
            pcs->pred_structure                 = scs->static_config.pred_structure;
            pcs->hierarchical_layers_diff       = 0;
            pcs->init_pred_struct_position_flag = false;
            pcs->tpl_group_size                 = 0;
            if (pcs->picture_number == 0) {
                ctx->prev_delayed_intra = NULL;
            }
            if (pcs->picture_number == 0) {
                ctx->sframe_hier_lvls = scs->static_config.hierarchical_levels;
            }

            release_prev_picture_from_reorder_queue(enc_ctx);

            // If the Intra period length is 0, then introduce an intra for every picture
            if (allintra) {
                pcs->idr_flag = true;
                pcs->cra_flag = false;
            }
            // If an #IntraPeriodLength has passed since the last Intra, then introduce a CRA or IDR based on Intra Refresh type
            else if (scs->static_config.intra_period_length != -1) {
                pcs->cra_flag = (scs->static_config.intra_refresh_type != SVT_AV1_FWDKF_REFRESH) ? pcs->cra_flag
                    : ((enc_ctx->intra_period_position == (uint32_t)scs->static_config.intra_period_length) ||
                       (pcs->scene_change_flag == true))
                    ? true
                    : pcs->cra_flag;

                pcs->idr_flag = (scs->static_config.intra_refresh_type != SVT_AV1_KF_REFRESH) ? pcs->idr_flag
                    : enc_ctx->intra_period_position == (uint32_t)scs->static_config.intra_period_length
                    ?

                    true
                    : pcs->idr_flag;
            }
            pcs->idr_flag = (scs->static_config.intra_refresh_type != SVT_AV1_KF_REFRESH)            ? pcs->idr_flag
                : (pcs->scene_change_flag == true || pcs->input_ptr->pic_type == EB_AV1_KEY_PICTURE) ? true
                                                                                                     : pcs->idr_flag;
            if (!allintra && pcs->picture_number > 0 && scs->static_config.sframe_posi.sframe_posis &&
                (pcs->cra_flag || pcs->idr_flag)) {
                // if this key frame position is set to an S-frame by sframe-posi, replace this I frame with B frame,
                // and then the S_FRAME will be set in set_sframe_type()
                int32_t dist_next_s = 0;
                if (get_dist_to_s(&scs->static_config.sframe_posi, pcs->picture_number, &dist_next_s) == 0) {
                    pcs->cra_flag = false;
                    pcs->idr_flag = false;
                }
            }
            enc_ctx->pre_assignment_buffer_eos_flag = (pcs->end_of_sequence_flag)
                ? (uint32_t)true
                : enc_ctx->pre_assignment_buffer_eos_flag;

            // Histogram data to be used at the next input (N + 1)
            // TODO: can this be moved to the end of perform_scene_change_detection? Histograms aren't needed if at EOS
            if (scs->calc_hist) {
                copy_histograms(pcs, ctx);
            }

            // Increment the Pre-Assignment Buffer Intra Count
            enc_ctx->pre_assignment_buffer_intra_count += (pcs->idr_flag || pcs->cra_flag);
            enc_ctx->pre_assignment_buffer_idr_count += pcs->idr_flag;
            enc_ctx->pre_assignment_buffer_count += 1;

            // Increment the Intra Period Position
            enc_ctx->intra_period_position = ((enc_ctx->intra_period_position ==
                                               (uint32_t)scs->static_config.intra_period_length) ||
                                              (pcs->scene_change_flag == true) ||
                                              pcs->input_ptr->pic_type == EB_AV1_KEY_PICTURE)
                ? 0
                : enc_ctx->intra_period_position + 1;

#if LAD_MG_PRINT
            print_pre_ass_buffer(enc_ctx, pcs, 1);
#endif

            uint32_t next_mg_hierarchical_levels = scs->static_config.hierarchical_levels;
            // Overwrite next_mg_hierarchical_levels when an S-Frame needs to modify the mini-GOP size.
            if (ctx->sframe_hier_lvls != (int32_t)scs->static_config.hierarchical_levels) {
                next_mg_hierarchical_levels = ctx->sframe_hier_lvls;
            }
            if (ctx->enable_startup_mg) {
                next_mg_hierarchical_levels = scs->static_config.startup_mg_size;
            }
            // Determine if Pictures can be released from the Pre-Assignment Buffer
            if ((enc_ctx->pre_assignment_buffer_intra_count > 0) ||
                (enc_ctx->pre_assignment_buffer_count == (uint32_t)(1 << next_mg_hierarchical_levels)) ||
                (enc_ctx->pre_assignment_buffer_eos_flag == true) || (pcs->pred_structure == LOW_DELAY)) {
#if LAD_MG_PRINT
                print_pre_ass_buffer(enc_ctx, pcs, 0);
#endif
                // Once there are enough frames in the pre-assignement buffer, we can setup the mini-gops
                set_mini_gop_structure(scs, enc_ctx, pcs, ctx);

                // Loop over Mini GOPs
                for (unsigned int mini_gop_index = 0; mini_gop_index < ctx->total_number_of_mini_gops;
                     ++mini_gop_index) {
                    bool pre_assignment_buffer_first_pass_flag = true;

                    // Get the 1st PCS in the mini-GOP
                    pcs = (PictureParentControlSet*)enc_ctx
                              ->pre_assignment_buffer[ctx->mini_gop_start_index[mini_gop_index]]
                              ->object_ptr;

                    // Derive the temporal layer difference between the current mini GOP and the previous mini GOP
                    pcs->hierarchical_layers_diff = (int32_t)enc_ctx->previous_mini_gop_hierarchical_levels -
                        (int32_t)pcs->hierarchical_levels;

                    // Set init_pred_struct_position_flag to true if mini-GOP switch
                    pcs->init_pred_struct_position_flag = enc_ctx->is_mini_gop_changed =
                        (pcs->hierarchical_layers_diff != 0);

                    // Keep track of the number of hierarchical levels of the latest implemented mini GOP
                    enc_ctx->previous_mini_gop_hierarchical_levels = ctx->mini_gop_hierarchical_levels[mini_gop_index];
                    ctx->cut_short_ra_mg                           = 0;
                    // 1st Loop over Pictures in the Pre-Assignment Buffer
                    // Setup the pred strucutre and picture types for all frames in the mini-GOP (including overlay pics)
                    for (pic_idx = ctx->mini_gop_start_index[mini_gop_index];
                         pic_idx <= ctx->mini_gop_end_index[mini_gop_index];
                         ++pic_idx) {
                        pcs = (PictureParentControlSet*)enc_ctx->pre_assignment_buffer[pic_idx]->object_ptr;
                        scs = pcs->scs;

                        update_pred_struct_and_pic_type(scs,
                                                        enc_ctx,
                                                        pcs,
                                                        ctx,
                                                        mini_gop_index,
                                                        pre_assignment_buffer_first_pass_flag,
                                                        &pcs->slice_type,
                                                        &pred_position_ptr);

                        if (scs->static_config.enable_overlays == true) {
                            // At this stage we know the prediction structure and the location of ALT_REF pictures.
                            // For every ALTREF picture, there is an overlay picture. They extra pictures are released
                            // is_alt_ref flag is set for non-slice base layer pictures
                            if (pred_position_ptr->temporal_layer_index == 0 && pcs->slice_type != I_SLICE) {
                                pcs->is_alt_ref         = 1;
                                pcs->frm_hdr.show_frame = 0;
                            }
                            // release the overlay PCS for non alt ref pictures. First picture does not have overlay PCS
                            else if (pcs->picture_number) {
                                svt_release_object(pcs->overlay_ppcs_ptr->input_pic_wrapper);
                                // release the pa_reference_picture
                                svt_release_object(pcs->overlay_ppcs_ptr->pa_ref_pic_wrapper);
                                svt_release_object(pcs->overlay_ppcs_ptr->scs_wrapper);
                                // release the parent pcs
                                // Note: this release will recycle ppcs to empty fifo if not live_count+1 in ResourceCoordination.
                                svt_release_object(pcs->overlay_ppcs_ptr->p_pcs_wrapper_ptr);
                                pcs->overlay_ppcs_ptr = NULL;
                            }
                        }

                        pcs->pic_idx_in_mg = get_pic_idx_in_mg(scs, pcs, ctx, pic_idx, mini_gop_index);

                        for (uint8_t loop_index = 0; loop_index <= pcs->is_alt_ref; loop_index++) {
                            // Init pred strucutre info - different for overlay/non-overlay
                            if (loop_index == 1) {
                                pcs = pcs->overlay_ppcs_ptr;
                                initialize_overlay_frame(pcs);
                            } else {
                                assert(!pcs->is_overlay);
                                pcs->pred_struct_index    = (uint8_t)enc_ctx->pred_struct_position;
                                pcs->temporal_layer_index = (uint8_t)pred_position_ptr->temporal_layer_index;
                                pcs->is_highest_layer     = (pcs->temporal_layer_index == pcs->hierarchical_levels);
                                switch (pcs->slice_type) {
                                case I_SLICE:

                                    // Reset Prediction Structure Position & Reference Struct Position
                                    if (pcs->picture_number == 0) {
                                        enc_ctx->intra_period_position = 0;
                                    }
                                    enc_ctx->elapsed_non_cra_count = 0;

                                    // I_SLICE cannot be CRA and IDR
                                    pcs->cra_flag = !pcs->idr_flag;

                                    if (pcs->idr_flag) {
                                        enc_ctx->elapsed_non_idr_count = 0; // Reset the pictures since last IDR counter
                                        ctx->key_poc = pcs->picture_number; // log latest key frame poc
                                    }
                                    break;
                                case B_SLICE:
                                    // Reset CRA and IDR Flag
                                    pcs->cra_flag = false;
                                    pcs->idr_flag = false;

                                    // Increment & Clip the elapsed Non-IDR Counter. This is clipped rather than allowed to free-run
                                    // inorder to avoid rollover issues.  This assumes that any the GOP period is less than MAX_ELAPSED_IDR_COUNT
                                    enc_ctx->elapsed_non_idr_count = MIN(enc_ctx->elapsed_non_idr_count + 1,
                                                                         MAX_ELAPSED_IDR_COUNT);
                                    enc_ctx->elapsed_non_cra_count = MIN(enc_ctx->elapsed_non_cra_count + 1,
                                                                         MAX_ELAPSED_IDR_COUNT);

                                    CHECK_REPORT_ERROR(
                                        (pcs->pred_struct_ptr->pred_struct_entry_count < MAX_ELAPSED_IDR_COUNT),
                                        enc_ctx->app_callback_ptr,
                                        EB_ENC_PD_ERROR1);

                                    break;
                                default:
                                    CHECK_REPORT_ERROR_NC(enc_ctx->app_callback_ptr, EB_ENC_PD_ERROR2);
                                    break;
                                }
                            }

                            CHECK_REPORT_ERROR((pcs->pred_struct_ptr->pred_struct_entry_count * REF_LIST_MAX_DEPTH <
                                                MAX_ELAPSED_IDR_COUNT),
                                               enc_ctx->app_callback_ptr,
                                               EB_ENC_PD_ERROR5);
                        }
                        pre_assignment_buffer_first_pass_flag = false;
                    }

                    // 2nd Loop over Pictures in the Pre-Assignment Buffer
                    // Init picture settings
                    // Add 1 to the loop for the overlay picture. If the last picture is alt ref, increase the loop by 1 to add the overlay picture
                    const uint32_t has_overlay = ((PictureParentControlSet*)enc_ctx
                                                      ->pre_assignment_buffer[ctx->mini_gop_end_index[mini_gop_index]]
                                                      ->object_ptr)
                                                     ->is_alt_ref
                        ? 1
                        : 0;
                    for (pic_idx = ctx->mini_gop_start_index[mini_gop_index];
                         pic_idx <= ctx->mini_gop_end_index[mini_gop_index] + has_overlay;
                         ++pic_idx) {
                        // Assign the overlay pcs. Since Overlay picture is not added to the picture_decision_pa_reference_queue, in the next stage, the loop finds the alt_ref picture. The reference for overlay frame is hardcoded later
                        if (has_overlay && pic_idx == ctx->mini_gop_end_index[mini_gop_index] + has_overlay) {
                            pcs = ((PictureParentControlSet*)enc_ctx
                                       ->pre_assignment_buffer[ctx->mini_gop_end_index[mini_gop_index]]
                                       ->object_ptr)
                                      ->overlay_ppcs_ptr;
                        } else {
                            pcs = (PictureParentControlSet*)enc_ctx->pre_assignment_buffer[pic_idx]->object_ptr;
                        }

                        pcs->picture_number_alt = enc_ctx->picture_number_alt++;

                        // Set the Decode Order
                        if ((ctx->mini_gop_idr_count[mini_gop_index] == 0) &&
                            (ctx->mini_gop_length[mini_gop_index] == pcs->pred_struct_ptr->pred_struct_entry_count) &&
                            (scs->static_config.pred_structure == RANDOM_ACCESS) && !pcs->is_overlay) {
                            pcs->decode_order = enc_ctx->decode_base_number +
                                pcs->pred_struct_ptr->pred_struct_entry_ptr_array[pcs->pred_struct_index]->decode_order;
                        } else {
                            pcs->decode_order = pcs->picture_number_alt;
                        }

                        perform_sc_detection(scs, pcs, ctx);
                        // Update the RC param queue
                        update_rc_param_queue(pcs, enc_ctx);
                        // Reset the PA Reference Lists
                        EB_MEMSET(
                            pcs->ref_pa_pic_ptr_array[REF_LIST_0], 0, REF_LIST_MAX_DEPTH * sizeof(EbObjectWrapper*));
                        EB_MEMSET(
                            pcs->ref_pa_pic_ptr_array[REF_LIST_1], 0, REF_LIST_MAX_DEPTH * sizeof(EbObjectWrapper*));
                        EB_MEMSET(pcs->ref_y8b_array[REF_LIST_0], 0, REF_LIST_MAX_DEPTH * sizeof(EbObjectWrapper*));
                        EB_MEMSET(pcs->ref_y8b_array[REF_LIST_1], 0, REF_LIST_MAX_DEPTH * sizeof(EbObjectWrapper*));
                        EB_MEMSET(pcs->ref_pic_poc_array[REF_LIST_0], 0, REF_LIST_MAX_DEPTH * sizeof(uint64_t));
                        EB_MEMSET(pcs->ref_pic_poc_array[REF_LIST_1], 0, REF_LIST_MAX_DEPTH * sizeof(uint64_t));

                        uint32_t pic_it                = pic_idx - ctx->mini_gop_start_index[mini_gop_index];
                        ctx->mg_pictures_array[pic_it] = pcs;
                        if (pic_idx == ctx->mini_gop_end_index[mini_gop_index] + has_overlay) {
                            // Increment the Decode Base Number
                            enc_ctx->decode_base_number += ctx->mini_gop_length[mini_gop_index] + has_overlay;
                        }
                    }

                    ctx->mg_size = ctx->mini_gop_end_index[mini_gop_index] + has_overlay -
                        ctx->mini_gop_start_index[mini_gop_index] + 1;

                    // Store pics in ctx->mg_pictures_array in decode order
                    // and pics in ctx->mg_pictures_array_disp_order in display order
                    store_mg_picture_arrays(ctx);

                    const unsigned int mg_size = ctx->mg_size;
                    for (uint32_t pic_i = 0; pic_i < mg_size; ++pic_i) {
                        // Loop over pics in decode order
                        pcs = ctx->mg_pictures_array[pic_i];
                        av1_generate_rps_info(pcs, enc_ctx, ctx, pcs->pic_idx_in_mg, mini_gop_index);

                        if (scs->static_config.sframe_dist != 0 || !pcs->is_not_scaled ||
                            scs->static_config.sframe_posi.sframe_posis) {
                            update_sframe_ref_order_hint(pcs, ctx);
                        }

                        update_dpb(pcs, ctx);

                        // Set picture settings, incl. normative frame header fields and feature levels in signal_derivation function
                        init_pic_settings(scs, pcs, ctx);
                    }

                    for (uint32_t pic_i = 0; pic_i < mg_size; ++pic_i) {
                        PictureParentControlSet* pcs_1 = ctx->mg_pictures_array_disp_order[pic_i];
                        pcs_1->first_frame_in_minigop  = !pic_i;
                        set_gf_group_param(pcs_1);
                        if (pcs_1->is_alt_ref) {
                            ctx->mg_pictures_array_disp_order[pic_i - 1]->has_show_existing = false;
                        }
                    }

                    // Loop over pics in MG and assign their PA reference buffers; release buffers when no longer needed
                    assign_and_release_pa_refs(enc_ctx, pcs, ctx);

                    // Send the pictures in the MG to TF and ME
                    process_pics(scs, ctx);
                } // End MINI GOPs loop
                // Reset the Pre-Assignment Buffer
                enc_ctx->pre_assignment_buffer_count       = 0;
                enc_ctx->pre_assignment_buffer_idr_count   = 0;
                enc_ctx->pre_assignment_buffer_intra_count = 0;
                enc_ctx->pre_assignment_buffer_eos_flag    = false;
            }
            // Increment the Picture Decision Reordering Queue Head Ptr
            enc_ctx->picture_decision_reorder_queue_head_index = (enc_ctx->picture_decision_reorder_queue_head_index ==
                                                                  enc_ctx->picture_decision_reorder_queue_size - 1)
                ? 0
                : enc_ctx->picture_decision_reorder_queue_head_index + 1;

            // Get the next entry from the Picture Decision Reordering Queue (Entry N+1)
            queue_entry_ptr =
                enc_ctx->picture_decision_reorder_queue[enc_ctx->picture_decision_reorder_queue_head_index];
        }

        if (scs->static_config.enable_overlays == true) {
            svt_release_object(((PictureParentControlSet*)in_results_ptr->pcs_wrapper->object_ptr)->scs_wrapper);
            // release ppcs, since live_count + 1 before post in ResourceCoordination
            svt_release_object(in_results_ptr->pcs_wrapper);
        }

        // Release the Input Results
        svt_release_object(in_results_wrapper_ptr);
    }

    return NULL;
}
