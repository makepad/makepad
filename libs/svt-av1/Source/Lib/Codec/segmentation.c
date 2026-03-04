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

#include "segmentation.h"
#include "segmentation_params.h"
#include "me_context.h"
#include "common_dsp_rtcd.h"
#if DEBUG_SEGMENT_QP || DEBUG_ROI
#include "svt_log.h"
#include <inttypes.h>
#endif
#include "deblocking_filter.h"

static uint16_t get_variance_for_cu(const BlockSize bsize, const int org_x, const int org_y, uint16_t* variance_ptr) {
    int index0, index1;
    //Assumes max CU size is 64
    switch (bsize) {
    case BLOCK_4X4:
    case BLOCK_4X8:
    case BLOCK_8X4:
    case BLOCK_8X8:
        index0 = index1 = ME_TIER_ZERO_PU_8x8_0 + ((org_x >> 3) + org_y);
        break;

    case BLOCK_8X16:
        index0 = ME_TIER_ZERO_PU_8x8_0 + ((org_x >> 3) + org_y);
        index1 = index0 + 1;
        break;

    case BLOCK_16X8:
        index0 = ME_TIER_ZERO_PU_8x8_0 + ((org_x >> 3) + org_y);
        index1 = index0 + org_y;
        break;

    case BLOCK_4X16:
    case BLOCK_16X4:
    case BLOCK_16X16:
        index0 = index1 = ME_TIER_ZERO_PU_16x16_0 + ((org_x >> 4) + (org_y >> 2));
        break;

    case BLOCK_16X32:
        index0 = ME_TIER_ZERO_PU_16x16_0 + ((org_x >> 4) + (org_y >> 2));
        index1 = index0 + 1;
        break;

    case BLOCK_32X16:
        index0 = ME_TIER_ZERO_PU_16x16_0 + ((org_x >> 4) + (org_y >> 2));
        index1 = index0 + (org_y >> 2);
        break;

    case BLOCK_8X32:
    case BLOCK_32X8:
    case BLOCK_32X32:
        index0 = index1 = ME_TIER_ZERO_PU_32x32_0 + ((org_x >> 5) + (org_y >> 4));
        break;

    case BLOCK_32X64:
        index0 = ME_TIER_ZERO_PU_32x32_0 + ((org_x >> 5) + (org_y >> 4));
        index1 = index0 + 1;
        break;

    case BLOCK_64X32:
        index0 = ME_TIER_ZERO_PU_32x32_0 + ((org_x >> 5) + (org_y >> 4));
        index1 = index0 + (org_y >> 4);
        break;

    case BLOCK_64X64:
    case BLOCK_16X64:
    case BLOCK_64X16:
    default:
        index0 = index1 = 0;
        break;
    }
    return (variance_ptr[index0] + variance_ptr[index1]) >> 1;
}

// org_x/y are the block location with respect to current SB origin
static void roi_map_apply_segmentation_based_quantization(PictureControlSet* pcs, SuperBlock* sb_ptr,
                                                          BlkStruct* blk_ptr, const BlockSize bsize, const int org_x,
                                                          const int org_y) {
    SequenceControlSet*    scs                 = pcs->ppcs->scs;
    const SvtAv1RoiMapEvt* roi_map             = pcs->ppcs->roi_map_evt;
    SegmentationParams*    segmentation_params = &pcs->ppcs->frm_hdr.segmentation_params;
    const int              stride_b64          = (scs->max_input_luma_width + 63) / 64;
    uint8_t                segment_id;
    if (scs->seq_header.sb_size == BLOCK_64X64) {
        const int column_b64 = sb_ptr->org_x >> 6;
        const int row_b64    = sb_ptr->org_y >> 6;
        segment_id           = roi_map->b64_seg_map[row_b64 * stride_b64 + column_b64];
    } else { // sb128
        segment_id = MAX_SEGMENTS;
        // 4 b64 blocks to check intersection
        int       b64_seg_columns[4] = {sb_ptr->org_x, sb_ptr->org_x + 64, sb_ptr->org_x, sb_ptr->org_x + 64};
        int       b64_seg_rows[4]    = {sb_ptr->org_y, sb_ptr->org_y, sb_ptr->org_y + 64, sb_ptr->org_y + 64};
        int       blk_org_x          = sb_ptr->org_x + org_x;
        int       blk_org_y          = sb_ptr->org_y + org_y;
        const int bwidth             = block_size_wide[bsize];
        const int bheight            = block_size_high[bsize];
        for (int i = 0; i < 4; ++i) {
            if (blk_org_x < b64_seg_columns[i] + 64 && blk_org_x + bwidth > b64_seg_columns[i] &&
                blk_org_y < b64_seg_rows[i] + 64 && blk_org_y + bheight > b64_seg_rows[i]) {
                const int column_b64 = b64_seg_columns[i] >> 6;
                const int row_b64    = b64_seg_rows[i] >> 6;
                segment_id           = MIN(segment_id, roi_map->b64_seg_map[row_b64 * stride_b64 + column_b64]);
            }
        }
    }

    for (int i = segment_id; i >= 0; i--) {
        int32_t q_index = pcs->ppcs->frm_hdr.quantization_params.base_q_idx +
            segmentation_params->feature_data[i][SEG_LVL_ALT_Q];
        // Avoid lossless since SVT-AV1 doesn't support it.
        if (q_index > 0) {
            blk_ptr->segment_id = i;
            break;
        }
    }
    assert(pcs->ppcs->frm_hdr.quantization_params.base_q_idx +
               segmentation_params->feature_data[blk_ptr->segment_id][SEG_LVL_ALT_Q] >
           0);
}

void svt_aom_apply_segmentation_based_quantization(PictureControlSet* pcs, SuperBlock* sb_ptr, BlkStruct* blk_ptr,
                                                   const BlockSize bsize, const int org_x, const int org_y) {
    if (pcs->ppcs->roi_map_evt != NULL) {
        roi_map_apply_segmentation_based_quantization(pcs, sb_ptr, blk_ptr, bsize, org_x, org_y);
        return;
    }
    uint16_t*           variance_ptr        = pcs->ppcs->variance[sb_ptr->index];
    SegmentationParams* segmentation_params = &pcs->ppcs->frm_hdr.segmentation_params;
    uint16_t            variance            = get_variance_for_cu(bsize, org_x, org_y, variance_ptr);
    blk_ptr->segment_id                     = 0;
    for (int i = MAX_SEGMENTS - 1; i >= 0; i--) {
        if (variance <= segmentation_params->variance_bin_edge[i]) {
            int32_t q_index = pcs->ppcs->frm_hdr.quantization_params.base_q_idx +
                segmentation_params->feature_data[i][SEG_LVL_ALT_Q];
            // Avoid lossless since SVT-AV1 doesn't support it.
            // Spec: Uncompressed header syntax and get_qindex(1, segmentID). And spec 5.11.34. Residual syntax. force TX_4X4 when lossless.
            if (q_index > 0) {
                blk_ptr->segment_id = i;
                break;
            }
        }
    }
}

static void roi_map_setup_segmentation(PictureControlSet* pcs, SequenceControlSet* scs) {
    UNUSED(scs);
    SvtAv1RoiMapEvt*    roi_map                       = pcs->ppcs->roi_map_evt;
    SegmentationParams* segmentation_params           = &pcs->ppcs->frm_hdr.segmentation_params;
    segmentation_params->segmentation_enabled         = true;
    segmentation_params->segmentation_update_data     = true;
    segmentation_params->segmentation_update_map      = true;
    segmentation_params->segmentation_temporal_update = false;

    for (int i = 0; i <= roi_map->max_seg_id; i++) {
        segmentation_params->feature_enabled[i][SEG_LVL_ALT_Q]      = 1;
        segmentation_params->feature_data[i][SEG_LVL_ALT_Q]         = roi_map->seg_qp[i];
        segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_Y_V] = 1;
        segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_Y_H] = 1;
        segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_U]   = 1;
        segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_V]   = 1;
    }

    // setup loop filter data
    int32_t filter_level[4];
    uint8_t qindex = pcs->ppcs->frm_hdr.quantization_params.base_q_idx;
    svt_av1_pick_filter_level_by_q(pcs, qindex, filter_level);
    for (int i = 0; i <= roi_map->max_seg_id; i++) {
        uint8_t qindex_seg = CLIP3(0, 255, qindex + segmentation_params->feature_data[i][SEG_LVL_ALT_Q]);
        int32_t filter_level_seg[4];
        svt_av1_pick_filter_level_by_q(pcs, qindex_seg, filter_level_seg);
        if (segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_Y_V]) {
            segmentation_params->feature_data[i][SEG_LVL_ALT_LF_Y_V] = filter_level_seg[0] - filter_level[0];
        }
        if (segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_Y_H]) {
            segmentation_params->feature_data[i][SEG_LVL_ALT_LF_Y_H] = filter_level_seg[1] - filter_level[1];
        }
        if (segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_U]) {
            segmentation_params->feature_data[i][SEG_LVL_ALT_LF_U] = filter_level_seg[2] - filter_level[2];
        }
        if (segmentation_params->feature_enabled[i][SEG_LVL_ALT_LF_V]) {
            segmentation_params->feature_data[i][SEG_LVL_ALT_LF_V] = filter_level_seg[3] - filter_level[3];
        }
    }
#if DEBUG_ROI
    if (segmentation_params->feature_enabled[0][SEG_LVL_ALT_LF_Y_V]) {
        SVT_LOG("frame %" PRIu64 ", lf_y_v: %d %d %d %d %d %d %d %d\n",
                pcs->picture_number,
                segmentation_params->feature_data[0][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[1][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[2][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[3][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[4][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[5][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[6][SEG_LVL_ALT_LF_Y_V],
                segmentation_params->feature_data[7][SEG_LVL_ALT_LF_Y_V]);
    }
    if (segmentation_params->feature_enabled[0][SEG_LVL_ALT_LF_Y_H]) {
        SVT_LOG("frame %" PRIu64 ", lf_y_h: %d %d %d %d %d %d %d %d\n",
                pcs->picture_number,
                segmentation_params->feature_data[0][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[1][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[2][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[3][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[4][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[5][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[6][SEG_LVL_ALT_LF_Y_H],
                segmentation_params->feature_data[7][SEG_LVL_ALT_LF_Y_H]);
    }
#endif
    calculate_segmentation_data(segmentation_params);
}

void svt_aom_setup_segmentation(PictureControlSet* pcs, SequenceControlSet* scs) {
    if (pcs->ppcs->roi_map_evt != NULL) {
        roi_map_setup_segmentation(pcs, scs);
        return;
    }
    SegmentationParams* segmentation_params   = &pcs->ppcs->frm_hdr.segmentation_params;
    segmentation_params->segmentation_enabled = scs->static_config.aq_mode == 1;
    if (segmentation_params->segmentation_enabled) {
        segmentation_params->segmentation_update_data =
            1; //always updating for now. Need to set this based on actual deltas
        segmentation_params->segmentation_update_map      = 1;
        segmentation_params->segmentation_temporal_update = false; //!frame_is_intra_only(pcs->ppcs);
        find_segment_qps(segmentation_params, pcs);
        for (int i = 0; i < MAX_SEGMENTS; i++) {
            segmentation_params->feature_enabled[i][SEG_LVL_ALT_Q] = 1;
        }

        calculate_segmentation_data(segmentation_params);
    }
}

void calculate_segmentation_data(SegmentationParams* segmentation_params) {
    for (int i = 0; i < MAX_SEGMENTS; i++) {
        for (int j = 0; j < SEG_LVL_MAX; j++) {
            if (segmentation_params->feature_enabled[i][j]) {
                segmentation_params->last_active_seg_id = i;
                if (j >= SEG_LVL_REF_FRAME) {
                    segmentation_params->seg_id_pre_skip = 1;
                }
            }
        }
    }
}

void find_segment_qps(SegmentationParams* segmentation_params,
                      PictureControlSet*  pcs) { //QP needs to be specified as qpindex, not qp.
    uint16_t    min_var = UINT16_MAX, max_var = MIN_UNSIGNED_VALUE, avg_var = 0;
    const float strength = 2; //to tune

    // get range of variance
    for (uint32_t sb_idx = 0; sb_idx < pcs->b64_total_count; ++sb_idx) {
        uint16_t* variance_ptr = pcs->ppcs->variance[sb_idx];
        uint32_t  var_index, local_avg = 0;
        // Loop over all 8x8s in a 64x64
        for (var_index = ME_TIER_ZERO_PU_8x8_0; var_index <= ME_TIER_ZERO_PU_8x8_63; var_index++) {
            max_var = MAX(max_var, variance_ptr[var_index]);
            min_var = MIN(min_var, variance_ptr[var_index]);
            local_avg += variance_ptr[var_index];
        }
        avg_var += (local_avg >> 6);
    }
    avg_var /= pcs->b64_total_count;
    avg_var = svt_log2f(avg_var);

    //get variance bin edges & QPs
    uint16_t min_var_log = svt_log2f(MAX(1, min_var));
    uint16_t max_var_log = svt_log2f(MAX(1, max_var));
    uint16_t step_size   = (uint16_t)(max_var_log - min_var_log) <= MAX_SEGMENTS
          ? 1
          : ROUND(((max_var_log - min_var_log)) / MAX_SEGMENTS);
    uint16_t bin_edge    = min_var_log + step_size;
    uint16_t bin_center  = bin_edge >> 1;

    for (int i = MAX_SEGMENTS - 1; i >= 0; i--) {
        segmentation_params->variance_bin_edge[i]           = POW2(bin_edge);
        segmentation_params->feature_data[i][SEG_LVL_ALT_Q] = ROUND((uint16_t)strength *
                                                                    (MAX(1, bin_center) - avg_var));
        bin_edge += step_size;
        bin_center += step_size;
    }
    if (segmentation_params->feature_data[0][SEG_LVL_ALT_Q] < 0) {
        // avoid lossless block
        segmentation_params->feature_data[0][SEG_LVL_ALT_Q] = 0;
    }
#if DEBUG_SEGMENT_QP
    SVT_LOG("frame %d, base_qindex %d, seg qp offset: %d %d %d %d %d %d %d %d\n",
            (int)pcs->picture_number,
            pcs->ppcs->frm_hdr.quantization_params.base_q_idx,
            segmentation_params->feature_data[0][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[1][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[2][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[3][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[4][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[5][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[6][SEG_LVL_ALT_Q],
            segmentation_params->feature_data[7][SEG_LVL_ALT_Q]);
#endif
}
