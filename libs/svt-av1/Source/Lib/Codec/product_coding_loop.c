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
#include <stdbool.h>

#include "definitions.h"
#include "me_sb_results.h"
#include "utility.h"
#include "rd_cost.h"
#include "full_loop.h"
#include "pic_operators.h"
#include "md_process.h"
#include "transforms.h"
#include "motion_estimation.h"
#include "aom_dsp_rtcd.h"
#include "coding_loop.h"
#include "svt_log.h"
#include "common_utils.h"
#include "resize.h"
#include "mv.h"
#include "mcomp.h"
#include "av1me.h"
#include "limits.h"
#include "ac_bias.h"

#include "pack_unpack_c.h"
#include "enc_inter_prediction.h"
#include "enc_mode_config.h"

#include "inter_prediction.h"
#include "enc_intra_prediction.h"
#include "mode_decision.h"
#include "adaptive_mv_pred.h"
#include "segmentation.h"

#define INIT_BIT_EST 6000
#define DIVIDE_AND_ROUND(x, y) (((x) + ((y) >> 1)) / (y))
uint64_t svt_spatial_full_distortion_ssim_kernel(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                 uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                 uint32_t area_width, uint32_t area_height, bool hbd, double ac_bias);
void     aom_av1_set_ssim_rdmult(ModeDecisionContext* ctx, PictureControlSet* pcs, const int mi_row, const int mi_col);

static const EbPredictionFunc product_prediction_fun_table_light_pd0[2] = {svt_av1_intra_prediction,
                                                                           svt_aom_inter_pu_prediction_av1_light_pd0};
static const EbPredictionFunc product_prediction_fun_table_light_pd1[2] = {svt_av1_intra_prediction,
                                                                           svt_aom_inter_pu_prediction_av1_light_pd1};
static const EbPredictionFunc product_prediction_fun_table[2]           = {svt_av1_intra_prediction,
                                                                           svt_aom_inter_pu_prediction_av1};
static const EbFastCostFunc   av1_product_fast_cost_func_table[2]       = {
    svt_aom_intra_fast_cost, /*INTRA */
    svt_aom_inter_fast_cost /*INTER */
};
#define MV_COST_WEIGHT 108

static void determine_best_references(PictureControlSet* pcs, ModeDecisionContext* ctx, MvReferenceFrame* ref_arr,
                                      uint8_t* tot_ref) {
    const MeSbResults* sb_results   = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
    const uint8_t      total_me_cnt = sb_results->total_me_candidate_index[ctx->me_block_offset];
    const MeCandidate* me_results   = &sb_results->me_candidate_array[ctx->me_cand_offset];

    uint8_t is_last_added     = 0;
    uint8_t is_bwd_added      = 0;
    uint8_t is_last_bwd_added = 0;

    uint8_t ri = 0;
    for (uint8_t me_index = 0; me_index < total_me_cnt; ++me_index) {
        const MeCandidate* cand = &me_results[me_index];
        if (cand->direction == 0) {
            ref_arr[ri++] = svt_get_ref_frame_type(REF_LIST_0, cand->ref_idx_l0);

            is_last_added = (cand->ref_idx_l0 == 0) ? 1 : is_last_added;
        }

        else if (cand->direction == 1) {
            ref_arr[ri++] = svt_get_ref_frame_type(REF_LIST_1, cand->ref_idx_l1);

            is_bwd_added = (cand->ref_idx_l1 == 0) ? 1 : is_bwd_added;
        }

        else if (cand->direction == 2) {
            MvReferenceFrame rf[2];
            rf[0] = svt_get_ref_frame_type(cand->ref0_list, cand->ref_idx_l0);
            rf[1] = svt_get_ref_frame_type(cand->ref1_list, cand->ref_idx_l1);

            {
                ref_arr[ri++]     = av1_ref_frame_type(rf);
                is_last_bwd_added = (rf[0] == LAST_FRAME && rf[1] == BWDREF_FRAME) ? 1 : is_last_bwd_added;
            }
        } else {
            svt_aom_assert_err(0, "corrupted me resutls");
        }
    }

    if (pcs->slice_type == B_SLICE) {
        if (!is_last_added && pcs->ppcs->ref_list0_count_try) {
            ref_arr[ri++] = LAST_FRAME;
        }
        if (!is_bwd_added && pcs->ppcs->ref_list1_count_try) {
            ref_arr[ri++] = BWDREF_FRAME;
        }
        if (!is_last_bwd_added && pcs->ppcs->ref_list0_count_try && pcs->ppcs->ref_list1_count_try) {
            ref_arr[ri++] = LAST_BWD_FRAME;
        }
    }
    *tot_ref = ri;
}

/***************************************************
* Update Recon Samples Neighbor Arrays
***************************************************/
static void mode_decision_update_neighbor_arrays_light_pd0(ModeDecisionContext* ctx, PC_TREE* pc_tree) {
    // LPD0 only updates the recon buffer for intra prediction; if not needed, no updates required
    if (ctx->skip_intra || ctx->lpd0_use_src_samples) {
        return;
    }

    // LPD0 assumes only a single block per shape (either PART_N or PART_H/PART_V for boundary blocks
    // where PART_N is invalid, in which case the second H/V block would not be allowed).
    const Part shape     = from_part_to_shape[pc_tree->partition];
    int        mi_row    = pc_tree->mi_row;
    int        mi_col    = pc_tree->mi_col;
    BlockSize  sub_bsize = partition_mi_offset(pc_tree->bsize, shape, 0 /*nsi*/, &mi_row, &mi_col);
    BlkStruct* blk_ptr   = pc_tree->block_data[shape][0];

    const int bwidth  = block_size_wide[sub_bsize];
    const int bheight = block_size_high[sub_bsize];
    svt_aom_update_recon_neighbor_array(ctx->recon_neigh_y,
                                        blk_ptr->neigh_top_recon[0],
                                        blk_ptr->neigh_left_recon[0],
                                        mi_col << MI_SIZE_LOG2,
                                        mi_row << MI_SIZE_LOG2,
                                        bwidth,
                                        bheight);
}

/***************************************************
* Update Recon Samples Neighbor Arrays
***************************************************/
static void mode_decision_update_neighbor_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                 const BlockSize bsize, const int mi_row, const int mi_col) {
    const int       bwidth          = block_size_wide[bsize];
    const int       bheight         = block_size_high[bsize];
    const int       org_x           = mi_col << MI_SIZE_LOG2;
    const int       org_y           = mi_row << MI_SIZE_LOG2;
    const int       blk_origin_x_uv = ROUND_UV(org_x) >> 1;
    const int       blk_origin_y_uv = ROUND_UV(org_y) >> 1;
    const BlockSize bsize_uv        = get_plane_block_size(bsize, 1, 1);
    const int       bwidth_uv       = block_size_wide[bsize_uv];
    const int       bheight_uv      = block_size_high[bsize_uv];
    const bool      has_chroma      = is_chroma_reference(mi_row, mi_col, bsize, 1, 1);

    const int is_inter = is_inter_block(&ctx->blk_ptr->block_mi);

    const uint16_t tile_idx = ctx->tile_index;

    struct PartitionContext partition;
    partition.above = partition_context_lookup[bsize].above;
    partition.left  = partition_context_lookup[bsize].left;

    svt_aom_neighbor_array_unit_mode_write(ctx->leaf_partition_na,
                                           (uint8_t*)(&partition),
                                           org_x,
                                           org_y,
                                           bwidth,
                                           bheight,
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    if (ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx) {
        const uint8_t  tx_depth     = ctx->blk_ptr->block_mi.tx_depth;
        const uint16_t txb_count    = tx_blocks_per_depth[bsize][tx_depth];
        const TxSize   tx_size      = tx_depth_to_tx_size[tx_depth][bsize];
        const int      tx_width     = tx_size_wide[tx_size];
        const int      tx_height    = tx_size_high[tx_size];
        const TxSize   tx_size_uv   = av1_get_max_uv_txsize(bsize, 1, 1);
        const int      tx_width_uv  = tx_size_wide[tx_size_uv];
        const int      tx_height_uv = tx_size_high[tx_size_uv];
        for (uint8_t txb_itr = 0; txb_itr < txb_count; txb_itr++) {
            const Position txb_org             = {org_x + tx_org[bsize][is_inter][tx_depth][txb_itr].x,
                                                  org_y + tx_org[bsize][is_inter][tx_depth][txb_itr].y};
            uint8_t        dc_sign_level_coeff = (uint8_t)ctx->blk_ptr->quant_dc.y[txb_itr];
            svt_aom_neighbor_array_unit_mode_write(ctx->luma_dc_sign_level_coeff_na,
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   txb_org.x,
                                                   txb_org.y,
                                                   tx_width,
                                                   tx_height,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

            svt_aom_neighbor_array_unit_mode_write(
                pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                (uint8_t*)&dc_sign_level_coeff,
                txb_org.x,
                txb_org.y,
                tx_width,
                tx_height,
                NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

            if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1 && (tx_depth == 0 || txb_itr == 0)) {
                //  Update chroma CB cbf and Dc context
                uint8_t dc_sign_level_coeff_cb = (uint8_t)ctx->blk_ptr->quant_dc.u[txb_itr];
                svt_aom_neighbor_array_unit_mode_write(ctx->cb_dc_sign_level_coeff_na,
                                                       (uint8_t*)&dc_sign_level_coeff_cb,
                                                       ROUND_UV(txb_org.x) >> 1,
                                                       ROUND_UV(txb_org.y) >> 1,
                                                       tx_width_uv,
                                                       tx_height_uv,
                                                       NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

                //  Update chroma CR cbf and Dc context
                uint8_t dc_sign_level_coeff_cr = (uint8_t)ctx->blk_ptr->quant_dc.v[txb_itr];
                svt_aom_neighbor_array_unit_mode_write(ctx->cr_dc_sign_level_coeff_na,
                                                       (uint8_t*)&dc_sign_level_coeff_cr,
                                                       ROUND_UV(txb_org.x) >> 1,
                                                       ROUND_UV(txb_org.y) >> 1,
                                                       tx_width_uv,
                                                       tx_height_uv,
                                                       NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
            }
        }
    }
    if (pcs->ppcs->frm_hdr.tx_mode == TX_MODE_SELECT) {
        uint8_t tx_size = tx_depth_to_tx_size[ctx->blk_ptr->block_mi.tx_depth][bsize];
        uint8_t bw      = tx_size_wide[tx_size];
        uint8_t bh      = tx_size_high[tx_size];

        svt_aom_neighbor_array_unit_mode_write(
            ctx->txfm_context_array, &bw, org_x, org_y, bwidth, bheight, NEIGHBOR_ARRAY_UNIT_TOP_MASK);

        svt_aom_neighbor_array_unit_mode_write(
            ctx->txfm_context_array, &bh, org_x, org_y, bwidth, bheight, NEIGHBOR_ARRAY_UNIT_LEFT_MASK);
    }
    if (!ctx->skip_intra || ctx->inter_intra_comp_ctrls.enabled) {
        if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md && ctx->pd_pass == PD_PASS_1) {
            // copy HBD
            svt_aom_update_recon_neighbor_array16bit(ctx->luma_recon_na_16bit,
                                                     ctx->blk_ptr->neigh_top_recon_16bit[0],
                                                     ctx->blk_ptr->neigh_left_recon_16bit[0],
                                                     org_x,
                                                     org_y,
                                                     bwidth,
                                                     bheight);

            if (ctx->txs_ctrls.enabled) {
                svt_aom_update_recon_neighbor_array16bit(
                    pcs->md_tx_depth_1_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                    ctx->blk_ptr->neigh_top_recon_16bit[0],
                    ctx->blk_ptr->neigh_left_recon_16bit[0],
                    org_x,
                    org_y,
                    bwidth,
                    bheight);
                svt_aom_update_recon_neighbor_array16bit(
                    pcs->md_tx_depth_2_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                    ctx->blk_ptr->neigh_top_recon_16bit[0],
                    ctx->blk_ptr->neigh_left_recon_16bit[0],
                    org_x,
                    org_y,
                    bwidth,
                    bheight);
            }

            if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
                svt_aom_update_recon_neighbor_array16bit(ctx->cb_recon_na_16bit,
                                                         ctx->blk_ptr->neigh_top_recon_16bit[1],
                                                         ctx->blk_ptr->neigh_left_recon_16bit[1],
                                                         blk_origin_x_uv,
                                                         blk_origin_y_uv,
                                                         bwidth_uv,
                                                         bheight_uv);
                svt_aom_update_recon_neighbor_array16bit(ctx->cr_recon_na_16bit,
                                                         ctx->blk_ptr->neigh_top_recon_16bit[2],
                                                         ctx->blk_ptr->neigh_left_recon_16bit[2],
                                                         blk_origin_x_uv,
                                                         blk_origin_y_uv,
                                                         bwidth_uv,
                                                         bheight_uv);
            }

            // copy 8 bit
            svt_aom_update_recon_neighbor_array(ctx->recon_neigh_y,
                                                ctx->blk_ptr->neigh_top_recon[0],
                                                ctx->blk_ptr->neigh_left_recon[0],
                                                org_x,
                                                org_y,
                                                bwidth,
                                                bheight);

            if (ctx->txs_ctrls.enabled) {
                svt_aom_update_recon_neighbor_array(pcs->md_tx_depth_1_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                                    ctx->blk_ptr->neigh_top_recon[0],
                                                    ctx->blk_ptr->neigh_left_recon[0],
                                                    org_x,
                                                    org_y,
                                                    bwidth,
                                                    bheight);
                svt_aom_update_recon_neighbor_array(pcs->md_tx_depth_2_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                                    ctx->blk_ptr->neigh_top_recon[0],
                                                    ctx->blk_ptr->neigh_left_recon[0],
                                                    org_x,
                                                    org_y,
                                                    bwidth,
                                                    bheight);
            }

            if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
                svt_aom_update_recon_neighbor_array(ctx->recon_neigh_cb,
                                                    ctx->blk_ptr->neigh_top_recon[1],
                                                    ctx->blk_ptr->neigh_left_recon[1],
                                                    blk_origin_x_uv,
                                                    blk_origin_y_uv,
                                                    bwidth_uv,
                                                    bheight_uv);
                svt_aom_update_recon_neighbor_array(ctx->recon_neigh_cr,
                                                    ctx->blk_ptr->neigh_top_recon[2],
                                                    ctx->blk_ptr->neigh_left_recon[2],
                                                    blk_origin_x_uv,
                                                    blk_origin_y_uv,
                                                    bwidth_uv,
                                                    bheight_uv);
            }
        } else if (!ctx->hbd_md) {
            svt_aom_update_recon_neighbor_array(ctx->recon_neigh_y,
                                                ctx->blk_ptr->neigh_top_recon[0],
                                                ctx->blk_ptr->neigh_left_recon[0],
                                                org_x,
                                                org_y,
                                                bwidth,
                                                bheight);
            if (ctx->txs_ctrls.enabled) {
                svt_aom_update_recon_neighbor_array(pcs->md_tx_depth_1_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                                    ctx->blk_ptr->neigh_top_recon[0],
                                                    ctx->blk_ptr->neigh_left_recon[0],
                                                    org_x,
                                                    org_y,
                                                    bwidth,
                                                    bheight);
                svt_aom_update_recon_neighbor_array(pcs->md_tx_depth_2_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                                    ctx->blk_ptr->neigh_top_recon[0],
                                                    ctx->blk_ptr->neigh_left_recon[0],
                                                    org_x,
                                                    org_y,
                                                    bwidth,
                                                    bheight);
            }
            if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
                svt_aom_update_recon_neighbor_array(ctx->recon_neigh_cb,
                                                    ctx->blk_ptr->neigh_top_recon[1],
                                                    ctx->blk_ptr->neigh_left_recon[1],
                                                    blk_origin_x_uv,
                                                    blk_origin_y_uv,
                                                    bwidth_uv,
                                                    bheight_uv);
                svt_aom_update_recon_neighbor_array(ctx->recon_neigh_cr,
                                                    ctx->blk_ptr->neigh_top_recon[2],
                                                    ctx->blk_ptr->neigh_left_recon[2],
                                                    blk_origin_x_uv,
                                                    blk_origin_y_uv,
                                                    bwidth_uv,
                                                    bheight_uv);
            }
        } else {
            svt_aom_update_recon_neighbor_array16bit(ctx->luma_recon_na_16bit,
                                                     ctx->blk_ptr->neigh_top_recon_16bit[0],
                                                     ctx->blk_ptr->neigh_left_recon_16bit[0],
                                                     org_x,
                                                     org_y,
                                                     bwidth,
                                                     bheight);
            if (ctx->txs_ctrls.enabled) {
                svt_aom_update_recon_neighbor_array16bit(
                    pcs->md_tx_depth_1_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                    ctx->blk_ptr->neigh_top_recon_16bit[0],
                    ctx->blk_ptr->neigh_left_recon_16bit[0],
                    org_x,
                    org_y,
                    bwidth,
                    bheight);
                svt_aom_update_recon_neighbor_array16bit(
                    pcs->md_tx_depth_2_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                    ctx->blk_ptr->neigh_top_recon_16bit[0],
                    ctx->blk_ptr->neigh_left_recon_16bit[0],
                    org_x,
                    org_y,
                    bwidth,
                    bheight);
            }
            if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
                svt_aom_update_recon_neighbor_array16bit(ctx->cb_recon_na_16bit,
                                                         ctx->blk_ptr->neigh_top_recon_16bit[1],
                                                         ctx->blk_ptr->neigh_left_recon_16bit[1],
                                                         blk_origin_x_uv,
                                                         blk_origin_y_uv,
                                                         bwidth_uv,
                                                         bheight_uv);
                svt_aom_update_recon_neighbor_array16bit(ctx->cr_recon_na_16bit,
                                                         ctx->blk_ptr->neigh_top_recon_16bit[2],
                                                         ctx->blk_ptr->neigh_left_recon_16bit[2],
                                                         blk_origin_x_uv,
                                                         blk_origin_y_uv,
                                                         bwidth_uv,
                                                         bheight_uv);
            }
        }
    }
}

/* input is bsize of the block to copy. mi_row/mi_col are the origin coordinates of the block to be copied. */
void svt_aom_copy_neighbour_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t src_idx, uint32_t dst_idx,
                                   const BlockSize bsize, const int mi_row, const int mi_col) {
    const uint16_t tile_idx = ctx->tile_index;

    const int       bwidth       = block_size_wide[bsize];
    const int       bheight      = block_size_high[bsize];
    const int       blk_org_x    = mi_col << MI_SIZE_LOG2;
    const int       blk_org_y    = mi_row << MI_SIZE_LOG2;
    const int       blk_org_x_uv = ROUND_UV(blk_org_x) >> 1;
    const int       blk_org_y_uv = ROUND_UV(blk_org_y) >> 1;
    const BlockSize bsize_uv     = get_plane_block_size(bsize, 1, 1);
    const int       bwidth_uv    = block_size_wide[bsize_uv];
    const int       bheight_uv   = block_size_high[bsize_uv];
    const bool      has_chroma   = is_chroma_reference(mi_row, mi_col, bsize, 1, 1);

    svt_aom_copy_neigh_arr(pcs->mdleaf_partition_na[src_idx][tile_idx],
                           pcs->mdleaf_partition_na[dst_idx][tile_idx],
                           blk_org_x,
                           blk_org_y,
                           bwidth,
                           bheight,
                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

    // if using 8bit MD and bypassing encdec, need to save 8bit and 10bit recon
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md && ctx->pd_pass == PD_PASS_1) {
        // Copy 10bit arrays
        svt_aom_copy_neigh_arr(pcs->md_luma_recon_na_16bit[src_idx][tile_idx],
                               pcs->md_luma_recon_na_16bit[dst_idx][tile_idx],
                               blk_org_x,
                               blk_org_y,
                               bwidth,
                               bheight,
                               NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        if (ctx->txs_ctrls.enabled) {
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_1_luma_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_tx_depth_1_luma_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_2_luma_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_tx_depth_2_luma_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
        if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_aom_copy_neigh_arr(pcs->md_cb_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_cb_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);

            svt_aom_copy_neigh_arr(pcs->md_cr_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_cr_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }

        // Copy 8bit arrays
        svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[src_idx][tile_idx],
                               pcs->md_luma_recon_na[dst_idx][tile_idx],
                               blk_org_x,
                               blk_org_y,
                               bwidth,
                               bheight,
                               NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        if (ctx->txs_ctrls.enabled) {
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_1_luma_recon_na[src_idx][tile_idx],
                                   pcs->md_tx_depth_1_luma_recon_na[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_2_luma_recon_na[src_idx][tile_idx],
                                   pcs->md_tx_depth_2_luma_recon_na[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
        if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_aom_copy_neigh_arr(pcs->md_cb_recon_na[src_idx][tile_idx],
                                   pcs->md_cb_recon_na[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);

            svt_aom_copy_neigh_arr(pcs->md_cr_recon_na[src_idx][tile_idx],
                                   pcs->md_cr_recon_na[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
    } else if (!ctx->hbd_md) {
        svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[src_idx][tile_idx],
                               pcs->md_luma_recon_na[dst_idx][tile_idx],
                               blk_org_x,
                               blk_org_y,
                               bwidth,
                               bheight,
                               NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        if (ctx->txs_ctrls.enabled) {
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_1_luma_recon_na[src_idx][tile_idx],
                                   pcs->md_tx_depth_1_luma_recon_na[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_2_luma_recon_na[src_idx][tile_idx],
                                   pcs->md_tx_depth_2_luma_recon_na[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
        if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_aom_copy_neigh_arr(pcs->md_cb_recon_na[src_idx][tile_idx],
                                   pcs->md_cb_recon_na[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);

            svt_aom_copy_neigh_arr(pcs->md_cr_recon_na[src_idx][tile_idx],
                                   pcs->md_cr_recon_na[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
    } else {
        svt_aom_copy_neigh_arr(pcs->md_luma_recon_na_16bit[src_idx][tile_idx],
                               pcs->md_luma_recon_na_16bit[dst_idx][tile_idx],
                               blk_org_x,
                               blk_org_y,
                               bwidth,
                               bheight,
                               NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        if (ctx->txs_ctrls.enabled) {
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_1_luma_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_tx_depth_1_luma_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
            svt_aom_copy_neigh_arr(pcs->md_tx_depth_2_luma_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_tx_depth_2_luma_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x,
                                   blk_org_y,
                                   bwidth,
                                   bheight,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
        if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_aom_copy_neigh_arr(pcs->md_cb_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_cb_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);

            svt_aom_copy_neigh_arr(pcs->md_cr_recon_na_16bit[src_idx][tile_idx],
                                   pcs->md_cr_recon_na_16bit[dst_idx][tile_idx],
                                   blk_org_x_uv,
                                   blk_org_y_uv,
                                   bwidth_uv,
                                   bheight_uv,
                                   NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
    }

    //svt_aom_neighbor_array_unit_reset(pcs->md_y_dcs_na[depth]);
    svt_aom_copy_neigh_arr(pcs->md_y_dcs_na[src_idx][tile_idx],
                           pcs->md_y_dcs_na[dst_idx][tile_idx],
                           blk_org_x,
                           blk_org_y,
                           bwidth,
                           bheight,
                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

    svt_aom_copy_neigh_arr(pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[src_idx][tile_idx],
                           pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[dst_idx][tile_idx],
                           blk_org_x,
                           blk_org_y,
                           bwidth,
                           bheight,
                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    if (has_chroma && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
        svt_aom_copy_neigh_arr(pcs->md_cb_dc_sign_level_coeff_na[src_idx][tile_idx],
                               pcs->md_cb_dc_sign_level_coeff_na[dst_idx][tile_idx],
                               blk_org_x_uv,
                               blk_org_y_uv,
                               bwidth_uv,
                               bheight_uv,
                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        //svt_aom_neighbor_array_unit_reset(pcs->md_cr_dc_sign_level_coeff_na[depth]);

        svt_aom_copy_neigh_arr(pcs->md_cr_dc_sign_level_coeff_na[src_idx][tile_idx],
                               pcs->md_cr_dc_sign_level_coeff_na[dst_idx][tile_idx],
                               blk_org_x_uv,
                               blk_org_y_uv,
                               bwidth_uv,
                               bheight_uv,
                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    }

    //svt_aom_neighbor_array_unit_reset(pcs->md_txfm_context_array[depth]);
    svt_aom_copy_neigh_arr(pcs->md_txfm_context_array[src_idx][tile_idx],
                           pcs->md_txfm_context_array[dst_idx][tile_idx],
                           blk_org_x,
                           blk_org_y,
                           bwidth,
                           bheight,
                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
}

// Update the neighbour arrays with the data from the passed block. Assumes the passed block is valid
// and the data is accurate (i.e. the block was tested in MD).
static void md_update_all_neighbour_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree,
                                           Part shape, int nsi) {
    assert(pc_tree->tested_blk[shape][nsi]);
    ctx->blk_ptr        = pc_tree->block_data[shape][nsi];
    int       mi_row    = pc_tree->mi_row;
    int       mi_col    = pc_tree->mi_col;
    BlockSize sub_bsize = partition_mi_offset(pc_tree->bsize, shape, nsi, &mi_row, &mi_col);

    mode_decision_update_neighbor_arrays(pcs, ctx, sub_bsize, mi_row, mi_col);
    if (ctx->pd_pass == PD_PASS_1 || !ctx->shut_fast_rate || ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx ||
        ctx->rate_est_ctrls.update_skip_coeff_ctx || ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled) {
        svt_aom_update_mi_map(pcs, ctx, pc_tree->partition, sub_bsize, mi_row, mi_col);
    }
}

static void md_update_all_neighbour_arrays_multiple(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                    PC_TREE* pc_tree) {
    const Part    shape           = from_part_to_shape[pc_tree->partition];
    const uint8_t shape_block_cnt = num_ns_per_shape[shape];

    for (uint32_t blk_it = 0; blk_it < shape_block_cnt; blk_it++) {
        // Only update neighbour arrays if the current block was tested. This should always be true
        // if we're calling this function, except for blocks that are outside the picture boundary,
        // but that are part of a valid shape (e.g. second block in PART_H at bottom pic boundary).
        if (pc_tree->tested_blk[shape][blk_it]) {
            md_update_all_neighbour_arrays(pcs, ctx, pc_tree, shape, blk_it);
        }
    }
}

/************************************************************************************************
* av1_perform_inverse_transform_recon_luma
* Apply inverse transform for Luma samples
************************************************************************************************/
void av1_perform_inverse_transform_recon_luma(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                              ModeDecisionCandidateBuffer* cand_bf) {
    const uint8_t  tx_depth       = cand_bf->cand->block_mi.tx_depth;
    const TxSize   tx_size        = tx_depth_to_tx_size[tx_depth][ctx->blk_geom->bsize];
    const int      txb_width      = tx_size_wide[tx_size];
    const int      txb_height     = tx_size_high[tx_size];
    const uint16_t tu_total_count = tx_blocks_per_depth[ctx->blk_geom->bsize][tx_depth];
    uint16_t       txb_itr        = 0;
    uint32_t       txb_1d_offset  = 0;
    const bool is_inter = (is_inter_mode(cand_bf->cand->block_mi.mode) || cand_bf->cand->block_mi.use_intrabc) ? true
                                                                                                               : false;
    do {
        uint32_t txb_origin_x     = tx_org[ctx->blk_geom->bsize][is_inter][tx_depth][txb_itr].x;
        uint32_t txb_origin_y     = tx_org[ctx->blk_geom->bsize][is_inter][tx_depth][txb_itr].y;
        uint32_t txb_origin_index = txb_origin_x + txb_origin_y * cand_bf->pred->stride_y;
        // Store recon with block origin offset b/c we may access in other blocks (for 4xN/Nx4 cases where chroma
        // is not allowed for each block)
        const Position blk_org   = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
        uint32_t rec_luma_offset = blk_org.x + txb_origin_x + (blk_org.y + txb_origin_y) * cand_bf->recon->stride_y;
        uint32_t y_has_coeff     = (cand_bf->y_has_coeff & (1 << txb_itr)) > 0;
        if (y_has_coeff) {
            svt_aom_inv_transform_recon_wrapper(
                pcs,
                ctx,
                cand_bf->pred->buffer_y,
                txb_origin_index,
                cand_bf->pred->stride_y,
                ctx->hbd_md ? (uint8_t*)ctx->cfl_temp_luma_recon16bit : ctx->cfl_temp_luma_recon,
                rec_luma_offset,
                cand_bf->recon->stride_y,
                (int32_t*)cand_bf->rec_coeff->buffer_y,
                txb_1d_offset,
                ctx->hbd_md,
                tx_size,
                cand_bf->cand->transform_type[txb_itr],
                PLANE_TYPE_Y,
                (uint32_t)cand_bf->eob.y[txb_itr]);
        } else {
            if (ctx->hbd_md) {
                svt_av1_copy_wxh_16bit((uint16_t*)cand_bf->pred->buffer_y + txb_origin_index,
                                       cand_bf->pred->stride_y,
                                       ctx->cfl_temp_luma_recon16bit + rec_luma_offset,
                                       cand_bf->recon->stride_y,
                                       txb_height,
                                       txb_width);
            } else {
                svt_av1_copy_wxh_8bit(cand_bf->pred->buffer_y + txb_origin_index,
                                      cand_bf->pred->stride_y,
                                      ctx->cfl_temp_luma_recon + rec_luma_offset,
                                      cand_bf->recon->stride_y,
                                      txb_height,
                                      txb_width);
            }
        }
        txb_1d_offset += txb_width * txb_height;
        ++txb_itr;
    } while (txb_itr < tu_total_count);
}

static void av1_perform_inverse_transform_recon(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                ModeDecisionCandidateBuffer* cand_bf, const BlockGeom* blk_geom) {
    const uint8_t  tx_depth       = cand_bf->cand->block_mi.tx_depth;
    const uint32_t tu_total_count = tx_blocks_per_depth[ctx->blk_geom->bsize][tx_depth];
    uint32_t       txb_itr        = 0;
    uint32_t       txb_1d_offset = 0, txb_1d_offset_uv = 0;
    const bool     is_inter = is_inter_mode(cand_bf->cand->block_mi.mode) || cand_bf->cand->block_mi.use_intrabc;
    do {
        const uint32_t txb_origin_x = tx_org[ctx->blk_geom->bsize][is_inter][tx_depth][txb_itr].x;
        const uint32_t txb_origin_y = tx_org[ctx->blk_geom->bsize][is_inter][tx_depth][txb_itr].y;
        TxSize         tx_size      = tx_depth_to_tx_size[tx_depth][blk_geom->bsize];
        const int      txb_width    = tx_size_wide[tx_size];
        const int      txb_height   = tx_size_high[tx_size] >> ctx->mds_subres_step;
        ;
        if (ctx->mds_subres_step == 2) {
            if (tx_size == TX_64X64) {
                tx_size = TX_64X16;
            } else if (tx_size == TX_32X32) {
                tx_size = TX_32X8;
            } else if (tx_size == TX_16X16) {
                tx_size = TX_16X4;
            } else {
                assert(0);
            }
        } else if (ctx->mds_subres_step == 1) {
            if (tx_size == TX_64X64) {
                tx_size = TX_64X32;
            } else if (tx_size == TX_32X32) {
                tx_size = TX_32X16;
            } else if (tx_size == TX_16X16) {
                tx_size = TX_16X8;
            } else if (tx_size == TX_8X8) {
                tx_size = TX_8X4;
            } else {
                assert(0);
            }
        }
        uint32_t rec_luma_offset = txb_origin_x + txb_origin_y * cand_bf->recon->stride_y;
        uint32_t rec_cb_offset   = ((ROUND_UV(txb_origin_x) + ROUND_UV(txb_origin_y) * cand_bf->recon->stride_cb) >> 1);
        uint32_t rec_cr_offset   = ((ROUND_UV(txb_origin_x) + ROUND_UV(txb_origin_y) * cand_bf->recon->stride_cr) >> 1);
        uint32_t txb_origin_index         = txb_origin_x + txb_origin_y * cand_bf->pred->stride_y;
        EbPictureBufferDesc* recon_buffer = cand_bf->recon;

        // If bypassing encdec, update the recon pointer to copy the recon directly
        // into the buffer used for EncDec; avoids copy after this function call.
        // cand_bf->recon is only used to update other buffers after this point.
        if (ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1) {
            if (ctx->fixed_partition) {
                svt_aom_get_recon_pic(pcs, &recon_buffer, ctx->hbd_md);
                uint16_t org_x  = ctx->blk_org_x + tx_org[blk_geom->bsize][is_inter][tx_depth][txb_itr].x;
                uint16_t org_y  = ctx->blk_org_y + tx_org[blk_geom->bsize][is_inter][tx_depth][txb_itr].y;
                rec_luma_offset = (recon_buffer->org_y + org_y) * recon_buffer->stride_y +
                    (recon_buffer->org_x + org_x);

                uint32_t round_origin_x = ROUND_UV(org_x); // for Chroma blocks with size of 4
                uint32_t round_origin_y = ROUND_UV(org_y); // for Chroma blocks with size of 4
                rec_cb_offset = rec_cr_offset = (((recon_buffer->org_y + round_origin_y) >> 1) *
                                                 recon_buffer->stride_cb) +
                    ((recon_buffer->org_x + round_origin_x) >> 1);
            } else {
                recon_buffer    = ctx->blk_ptr->recon_tmp;
                rec_luma_offset = txb_origin_x + (txb_origin_y * recon_buffer->stride_y);
                rec_cb_offset   = ROUND_UV((txb_origin_x) + (txb_origin_y * recon_buffer->stride_cb)) >> 1;
                rec_cr_offset   = ROUND_UV((txb_origin_x) + (txb_origin_y * recon_buffer->stride_cr)) >> 1;
            }
        }
        if (ctx->blk_ptr->y_has_coeff & (1 << txb_itr)) {
            svt_aom_inv_transform_recon_wrapper(pcs,
                                                ctx,
                                                cand_bf->pred->buffer_y,
                                                txb_origin_index,
                                                cand_bf->pred->stride_y << ctx->mds_subres_step,
                                                recon_buffer->buffer_y,
                                                rec_luma_offset,
                                                recon_buffer->stride_y << ctx->mds_subres_step,
                                                (int32_t*)cand_bf->rec_coeff->buffer_y,
                                                txb_1d_offset,
                                                ctx->hbd_md,
                                                tx_size,
                                                cand_bf->cand->transform_type[txb_itr],
                                                PLANE_TYPE_Y,
                                                (uint32_t)cand_bf->eob.y[txb_itr]);
            if (ctx->mds_subres_step == 2) {
                for (int i = 0; i < (txb_height * 4); i += 4) {
                    if (ctx->hbd_md) {
                        svt_memcpy(
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + (i + 1) * recon_buffer->stride_y,
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + i * recon_buffer->stride_y,
                            txb_width * sizeof(uint16_t));
                        svt_memcpy(
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + (i + 2) * recon_buffer->stride_y,
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + i * recon_buffer->stride_y,
                            txb_width * sizeof(uint16_t));
                        svt_memcpy(
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + (i + 3) * recon_buffer->stride_y,
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + i * recon_buffer->stride_y,
                            txb_width * sizeof(uint16_t));
                    } else {
                        svt_memcpy(recon_buffer->buffer_y + rec_luma_offset + (i + 1) * recon_buffer->stride_y,
                                   recon_buffer->buffer_y + rec_luma_offset + i * recon_buffer->stride_y,
                                   txb_width);
                        svt_memcpy(recon_buffer->buffer_y + rec_luma_offset + (i + 2) * recon_buffer->stride_y,
                                   recon_buffer->buffer_y + rec_luma_offset + i * recon_buffer->stride_y,
                                   txb_width);
                        svt_memcpy(recon_buffer->buffer_y + rec_luma_offset + (i + 3) * recon_buffer->stride_y,
                                   recon_buffer->buffer_y + rec_luma_offset + i * recon_buffer->stride_y,
                                   txb_width);
                    }
                }
            } else if (ctx->mds_subres_step) {
                for (int i = 0; i < (txb_height * 2); i += 2) {
                    if (ctx->hbd_md) {
                        svt_memcpy(
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + (i + 1) * recon_buffer->stride_y,
                            ((uint16_t*)recon_buffer->buffer_y) + rec_luma_offset + i * recon_buffer->stride_y,
                            txb_width * sizeof(uint16_t));
                    } else {
                        svt_memcpy(recon_buffer->buffer_y + rec_luma_offset + (i + 1) * recon_buffer->stride_y,
                                   recon_buffer->buffer_y + rec_luma_offset + i * recon_buffer->stride_y,
                                   txb_width);
                    }
                }
            }
        } else {
            svt_av1_picture_copy_y(cand_bf->pred,
                                   txb_origin_index,
                                   recon_buffer,
                                   rec_luma_offset,
                                   txb_width,
                                   txb_height << ctx->mds_subres_step,
                                   ctx->hbd_md);
        }

        //CHROMA
        if (ctx->has_uv && (tx_depth == 0 || txb_itr == 0)) {
            if (ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
                const TxSize   tx_size_uv        = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
                const uint32_t chroma_txb_width  = tx_size_wide[tx_size_uv];
                const uint32_t chroma_txb_height = tx_size_high[tx_size_uv];
                const uint32_t cb_tu_chroma_origin_index =
                    ((ROUND_UV(txb_origin_x) + ROUND_UV(txb_origin_y) * cand_bf->rec_coeff->stride_cb) >> 1);
                const uint32_t cr_tu_chroma_origin_index =
                    ((ROUND_UV(txb_origin_x) + ROUND_UV(txb_origin_y) * cand_bf->rec_coeff->stride_cr) >> 1);
                if (ctx->blk_ptr->u_has_coeff & (1 << txb_itr)) {
                    svt_aom_inv_transform_recon_wrapper(pcs,
                                                        ctx,
                                                        cand_bf->pred->buffer_cb,
                                                        cb_tu_chroma_origin_index,
                                                        cand_bf->pred->stride_cb,
                                                        recon_buffer->buffer_cb,
                                                        rec_cb_offset,
                                                        recon_buffer->stride_cb,
                                                        (int32_t*)cand_bf->rec_coeff->buffer_cb,
                                                        txb_1d_offset_uv,
                                                        ctx->hbd_md,
                                                        tx_size_uv,
                                                        cand_bf->cand->transform_type_uv,
                                                        PLANE_TYPE_UV,
                                                        (uint32_t)cand_bf->eob.u[txb_itr]);
                } else {
                    svt_av1_picture_copy_cb(cand_bf->pred,
                                            cb_tu_chroma_origin_index,
                                            recon_buffer,
                                            rec_cb_offset,
                                            chroma_txb_width,
                                            chroma_txb_height,
                                            ctx->hbd_md);
                }

                if (ctx->blk_ptr->v_has_coeff & (1 << txb_itr)) {
                    svt_aom_inv_transform_recon_wrapper(pcs,
                                                        ctx,
                                                        cand_bf->pred->buffer_cr,
                                                        cr_tu_chroma_origin_index,
                                                        cand_bf->pred->stride_cr,
                                                        recon_buffer->buffer_cr,
                                                        rec_cr_offset,
                                                        recon_buffer->stride_cr,
                                                        (int32_t*)cand_bf->rec_coeff->buffer_cr,
                                                        txb_1d_offset_uv,
                                                        ctx->hbd_md,
                                                        tx_size_uv,
                                                        cand_bf->cand->transform_type_uv,
                                                        PLANE_TYPE_UV,
                                                        (uint32_t)cand_bf->eob.v[txb_itr]);
                } else {
                    svt_av1_picture_copy_cr(cand_bf->pred,
                                            cr_tu_chroma_origin_index,
                                            recon_buffer,
                                            rec_cr_offset,
                                            chroma_txb_width,
                                            chroma_txb_height,
                                            ctx->hbd_md);
                }

                txb_1d_offset_uv += chroma_txb_width * chroma_txb_height;
            }
        }
        txb_1d_offset += txb_width * (txb_height << ctx->mds_subres_step);
        ++txb_itr;
    } while (txb_itr < tu_total_count);
}

/*******************************************
 * Coding Loop - Fast Loop Initialization
 *******************************************/
static void product_coding_loop_init_fast_loop(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    ctx->tx_depth = ctx->blk_ptr->block_mi.tx_depth = 0;
    // Generate Split, Skip and intra mode contexts for the rate estimation
    svt_aom_coding_loop_context_generation(pcs, ctx);

    return;
}

static void fast_loop_core_light_pd0(ModeDecisionCandidateBuffer* cand_bf, PictureControlSet* pcs,
                                     ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                     uint32_t input_origin_index, uint32_t cu_origin_index) {
    ModeDecisionCandidate* cand = cand_bf->cand;
    EbPictureBufferDesc*   pred = cand_bf->pred;

    if (ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0) {
        MvReferenceFrame rf[2] = {cand->block_mi.ref_frame[0], cand->block_mi.ref_frame[1]};

        assert(rf[1] == NONE_FRAME);
        const int8_t ref_idx_first  = get_ref_frame_idx(rf[0]);
        const int8_t list_idx_first = get_list_idx(rf[0]);

        EbReferenceObject* ref_obj =
            (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx_first][ref_idx_first]->object_ptr;
        EbPictureBufferDesc* ref_pic = svt_aom_get_ref_pic_buffer(pcs, cand->block_mi.ref_frame[0]);
        const int16_t        mv_x    = cand->block_mi.mv[0].x >> 3;
        const int16_t        mv_y    = cand->block_mi.mv[0].y >> 3;

        // -------
        // Use scaled references if resolution of the reference is different from that of the input
        // -------
        svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, 0);
        const int32_t ref_origin_index = ref_pic->org_x + (ctx->blk_org_x + mv_x) +
            (ctx->blk_org_y + mv_y + ref_pic->org_y) * ref_pic->stride_y;
        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
        unsigned int            sse;
        uint8_t*                pred_y = ref_pic->buffer_y + ref_origin_index;
        uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
        *(cand_bf->fast_cost)          = fn_ptr->vf(pred_y, ref_pic->stride_y, src_y, input_pic->stride_y, &sse);
    } else {
        // intrabc not allowed in light_pd0
        product_prediction_fun_table_light_pd0[is_inter_mode(cand->block_mi.mode)](0, ctx, pcs, cand_bf);
        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
        unsigned int            sse;
        uint8_t*                pred_y = pred->buffer_y + cu_origin_index;
        uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
        *(cand_bf->fast_cost)          = fn_ptr->vf(pred_y, pred->stride_y, src_y, input_pic->stride_y, &sse);
    }
}

// Light PD1 fast loop core; assumes luma only, 8bit only, and that SSD is not used.
static void fast_loop_core_light_pd1(ModeDecisionCandidateBuffer* cand_bf, PictureControlSet* pcs,
                                     ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic, BlockLocation* loc) {
    uint64_t       luma_fast_dist;
    const uint32_t full_lambda = ctx->full_lambda_md[EB_8_BIT_MD];

    ModeDecisionCandidate* cand = cand_bf->cand;
    EbPictureBufferDesc*   pred = cand_bf->pred;
    // If not first candidate to be tested, take advantage of known info to skip current candidate
    if (ctx->mds0_best_cost != (uint64_t)~0) {
        if (is_intra_mode(cand->block_mi.mode) && ctx->cand_reduction_ctrls.cand_elimination_ctrls.enabled) {
            const uint64_t best_dist = ctx->cand_bf_ptr_array[ctx->mds0_best_idx]->luma_fast_dist;

            // Use more aggressive dc_only_th at MDS0
            uint32_t th = cand->block_mi.mode != DC_PRED ? ctx->cand_reduction_ctrls.cand_elimination_ctrls.dc_only_th
                                                         : ctx->cand_reduction_ctrls.cand_elimination_ctrls.skip_dc_th;
            th *= (ctx->blk_geom->bheight * ctx->blk_geom->bwidth);
            if (best_dist < th) {
                // already injected/tested; set cost to max and exit
                *(cand_bf->fast_cost) = MAX_MODE_COST;
                return;
            }
        }
    }
    // Prediction
    ctx->uv_intra_comp_only = false;
    product_prediction_fun_table_light_pd1[is_inter_mode(cand->block_mi.mode)](0, ctx, pcs, cand_bf);
    // Distortion
    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
    unsigned int            sse;
    uint8_t*                pred_y = pred->buffer_y;
    uint8_t*                src_y  = input_pic->buffer_y + loc->input_origin_index;
    cand_bf->luma_fast_dist        = fn_ptr->vf(pred_y, pred->stride_y, src_y, input_pic->stride_y, &sse);
    // Shift variance by 4 because we use full lambda in the cost (since variance is proportional to sse)
    // and full lambda is set with the expectation the variance is a squared metric shifted by 4 (the same
    // shift is applied to sse in the full loop)
    luma_fast_dist = cand_bf->luma_fast_dist << 4;
    // Set full_dist to sse because it's used by lpd1_bypass_tx_th to skip the TX
    cand_bf->full_dist = sse;
    // If distortion cost is greater than the best cost, exit early. This candidate will never be
    // selected b/c only one candidate is sent to MDS3
    if (ctx->mds0_best_cost != (uint64_t)~0) {
        const uint64_t distortion_cost = RDCOST(full_lambda, 0, luma_fast_dist);
        if (distortion_cost > ctx->mds0_best_cost) {
            *(cand_bf->fast_cost) = MAX_MODE_COST;
            return;
        }
    }

    // Fast Cost
    if (ctx->shut_fast_rate) {
        *(cand_bf->fast_cost)     = luma_fast_dist;
        cand_bf->fast_luma_rate   = 0;
        cand_bf->fast_chroma_rate = 0;
    } else {
        *(cand_bf->fast_cost) = av1_product_fast_cost_func_table[is_inter_mode(cand->block_mi.mode)](
            pcs, ctx, cand_bf, full_lambda, luma_fast_dist);
    }
}

static void obmc_trans_face_off(ModeDecisionCandidateBuffer* cand_bf, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                EbPictureBufferDesc* input_pic, BlockLocation* loc) {
    const uint32_t input_origin_index = loc->input_origin_index;
    // 10bit lambda is shifted left by 4 to account for the difference in range between 8bit/10bit SSE. The 10bit variance function
    // automatically shifts the variance so the variance of 10bit will match the range of 8bit. Therefore, the shifted lambda is not
    // needed, and as such, the shift is nullified here so the lambda/distortion are properly proportioned.
    uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] >> 4 : ctx->full_lambda_md[EB_8_BIT_MD];
    ModeDecisionCandidate* cand = cand_bf->cand;
    EbPictureBufferDesc*   pred = cand_bf->pred;

    if (is_inter_singleref_mode(cand->block_mi.mode)) {
        MvReferenceFrame rf[2] = {cand->block_mi.ref_frame[0], cand->block_mi.ref_frame[1]};

        uint8_t is_obmc_allowed = svt_aom_obmc_motion_mode_allowed(
                                      pcs, ctx, ctx->blk_geom->bsize, 2, rf[0], NONE_FRAME, cand->block_mi.mode) ==
            OBMC_CAUSAL;

        if (is_inter_mode(cand_bf->cand->block_mi.mode) && is_obmc_allowed &&
            cand->block_mi.motion_mode == SIMPLE_TRANSLATION && cand->block_mi.is_interintra_used == 0) {
            // Derive the fast luma rate of OBMC
            MotionMode last_motion_mode_allowed        = svt_aom_motion_mode_allowed(pcs,
                                                                              cand->block_mi.num_proj_ref,
                                                                              ctx->blk_ptr->overlappable_neighbors,
                                                                              ctx->blk_geom->bsize,
                                                                              rf[0],
                                                                              rf[1],
                                                                              (PredictionMode)cand->block_mi.mode);
            int        inter_mode_bits_num_translation = 0;
            int        inter_mode_bits_num_obmc        = 0;

            switch (last_motion_mode_allowed) {
            case SIMPLE_TRANSLATION:
                break;
            case OBMC_CAUSAL:
                inter_mode_bits_num_translation = ctx->md_rate_est_ctx->motion_mode_fac_bits1[ctx->blk_geom->bsize][0];
                inter_mode_bits_num_obmc        = ctx->md_rate_est_ctx->motion_mode_fac_bits1[ctx->blk_geom->bsize][1];
                break;
            default:
                inter_mode_bits_num_translation =
                    ctx->md_rate_est_ctx->motion_mode_fac_bits[ctx->blk_geom->bsize][SIMPLE_TRANSLATION];
                inter_mode_bits_num_obmc =
                    ctx->md_rate_est_ctx->motion_mode_fac_bits[ctx->blk_geom->bsize][OBMC_CAUSAL];
                break;
            }
            uint64_t obmc_fast_luma_rate = cand_bf->fast_luma_rate + inter_mode_bits_num_obmc -
                inter_mode_bits_num_translation;
            uint64_t luma_fast_dist;
            // Take a copy of the simple-translation results
            uint64_t simple_translation_cost                 = *(cand_bf->fast_cost);
            uint64_t simple_translation_fast_luma_rate       = cand_bf->fast_luma_rate;
            uint64_t simple_translation_fast_chroma_rate     = cand_bf->fast_chroma_rate;
            uint64_t simple_translation_luma_fast_distortion = cand_bf->luma_fast_dist;
            // Modify the motion-mode
            cand->block_mi.motion_mode = OBMC_CAUSAL;

            // Prediction
            ctx->uv_intra_comp_only = false;
            svt_aom_inter_pu_prediction_av1_obmc(ctx->hbd_md, ctx, pcs, cand_bf);

            // Distortion
            if (!ctx->hbd_md) {
                const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
                unsigned int            sse;
                uint8_t*                pred_y = pred->buffer_y;
                uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
                cand_bf->luma_fast_dist        = fn_ptr->vf(pred_y, pred->stride_y, src_y, input_pic->stride_y, &sse);
            } else {
                const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
                unsigned int            sse;
                uint16_t*               pred_y = ((uint16_t*)pred->buffer_y);
                uint16_t*               src_y  = ((uint16_t*)input_pic->buffer_y) + input_origin_index;
                cand_bf->luma_fast_dist        = fn_ptr->vf_hbd_10(
                    CONVERT_TO_BYTEPTR(pred_y), pred->stride_y, CONVERT_TO_BYTEPTR(src_y), input_pic->stride_y, &sse);
            }
            // Shift variance by 4 because we use full lambda in the cost (since variance is proportional to sse)
            // and full lambda is set with the expectation the variance is a squared metric shifted by 4 (the same
            // shift is applied to sse in the full loop)
            luma_fast_dist          = cand_bf->luma_fast_dist << 4;
            cand_bf->fast_luma_rate = obmc_fast_luma_rate;
            *(cand_bf->fast_cost)   = RDCOST(
                full_lambda, cand_bf->fast_luma_rate + cand_bf->fast_chroma_rate, luma_fast_dist);
            if (simple_translation_cost < *(cand_bf->fast_cost)) {
                // Restore the simple-translation results
                cand->block_mi.motion_mode = SIMPLE_TRANSLATION;
                *(cand_bf->fast_cost)      = simple_translation_cost;
                cand_bf->fast_luma_rate    = simple_translation_fast_luma_rate;
                cand_bf->fast_chroma_rate  = simple_translation_fast_chroma_rate;
                cand_bf->luma_fast_dist    = simple_translation_luma_fast_distortion;
                cand_bf->valid_luma_pred   = 0;

            } else {
                cand_bf->valid_luma_pred = 1;
            }
        }
    }
}

uint32_t hadamard_path(ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                       BlockLocation* loc) {
    EbPictureBufferDesc* pred = cand_bf->pred;

    const uint32_t input_origin_index = loc->input_origin_index;

    BlockSize bsize = ctx->blk_geom->bsize;
    uint32_t  input_idx, pred_idx, res_idx;

    int16_t* res_ptr   = (int16_t*)cand_bf->residual->buffer_y;
    int32_t* coeff_ptr = (int32_t*)ctx->tx_coeffs->buffer_y;

    uint32_t satd_cost = 0;

    const TxSize tx_size = AOMMIN(TX_32X32, eb_max_txsize_lookup[bsize]);

    const int stepr = eb_tx_size_high_unit[tx_size];
    const int stepc = eb_tx_size_wide_unit[tx_size];
    const int txbw  = tx_size_wide[tx_size];
    const int txbh  = tx_size_high[tx_size];

    const int max_blocks_wide = block_size_wide[bsize] >> MI_SIZE_LOG2;
    const int max_blocks_high = block_size_high[bsize] >> MI_SIZE_LOG2;

    int row, col;

    for (row = 0; row < max_blocks_high; row += stepr) {
        for (col = 0; col < max_blocks_wide; col += stepc) {
            input_idx = ((row * input_pic->stride_y) + col) << 2;
            pred_idx  = ((row * pred->stride_y) + col) << 2;
            res_idx   = 0;

            svt_aom_residual_kernel(input_pic->buffer_y,
                                    input_idx + input_origin_index,
                                    input_pic->stride_y,
                                    pred->buffer_y,
                                    pred_idx,
                                    pred->stride_y,
                                    (int16_t*)res_ptr,
                                    res_idx,
                                    cand_bf->residual->stride_y,
                                    ctx->hbd_md,
                                    txbw,
                                    txbh);

            switch (tx_size) {
            case TX_4X4:
                svt_aom_hadamard_4x4(res_ptr, cand_bf->residual->stride_y, &(coeff_ptr[0]));
                break;

            case TX_8X8:
                svt_aom_hadamard_8x8(res_ptr, cand_bf->residual->stride_y, &(coeff_ptr[0]));
                break;

            case TX_16X16:
                svt_aom_hadamard_16x16(res_ptr, cand_bf->residual->stride_y, &(coeff_ptr[0]));
                break;

            case TX_32X32:
                svt_aom_hadamard_32x32(res_ptr, cand_bf->residual->stride_y, &(coeff_ptr[0]));
                break;

            default:
                assert(0);
            }
            satd_cost += svt_aom_satd(&(coeff_ptr[0]), tx_size_2d[tx_size]);
        }
    }
    return (satd_cost);
}

void fast_loop_core(ModeDecisionCandidateBuffer* cand_bf, PictureControlSet* pcs, ModeDecisionContext* ctx,
                    EbPictureBufferDesc* input_pic, BlockLocation* loc) {
    const uint32_t input_origin_index = loc->input_origin_index;
    uint64_t       luma_fast_dist;
    // 10bit lambda is shifted left by 4 to account for the difference in range between 8bit/10bit SSE. The 10bit variance function
    // automatically shifts the variance so the variance of 10bit will match the range of 8bit. Therefore, the shifted lambda is not
    // needed, and as such, the shift is nullified here so the lambda/distortion are properly proportioned.
    uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] >> 4 : ctx->full_lambda_md[EB_8_BIT_MD];
    ModeDecisionCandidate* cand = cand_bf->cand;
    EbPictureBufferDesc*   pred = cand_bf->pred;
    // Prediction
    ctx->uv_intra_comp_only = false;
    product_prediction_fun_table[is_inter_mode(cand->block_mi.mode) || cand->block_mi.use_intrabc](
        ctx->hbd_md, ctx, pcs, cand_bf);
#if OPT_PER_BLK_INTRA
    if (ctx->mds0_use_hadamard_blk) {
#else
    if (ctx->mds0_use_hadamard) {
#endif
        uint32_t satd           = hadamard_path(cand_bf, ctx, input_pic, loc);
        cand_bf->luma_fast_dist = satd;

        luma_fast_dist = cand_bf->luma_fast_dist << 4;
    } else {
        // Distortion
        if (!ctx->hbd_md) {
            const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
            unsigned int            sse;
            uint8_t*                pred_y = pred->buffer_y;
            uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
            cand_bf->luma_fast_dist        = fn_ptr->vf(pred_y, pred->stride_y, src_y, input_pic->stride_y, &sse);
        } else {
            const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
            unsigned int            sse;
            uint16_t*               pred_y = ((uint16_t*)pred->buffer_y);
            uint16_t*               src_y  = ((uint16_t*)input_pic->buffer_y) + input_origin_index;
            cand_bf->luma_fast_dist        = fn_ptr->vf_hbd_10(
                CONVERT_TO_BYTEPTR(pred_y), pred->stride_y, CONVERT_TO_BYTEPTR(src_y), input_pic->stride_y, &sse);
        }
        // Shift variance by 4 because we use full lambda in the cost (since variance is proportional to sse)
        // and full lambda is set with the expectation the variance is a squared metric shifted by 4 (the same
        // shift is applied to sse in the full loop)
        luma_fast_dist = cand_bf->luma_fast_dist << 4;
    }
    if (ctx->mds0_ctrls.pruning_method_th && ctx->pd_pass == PD_PASS_1) {
        if (ctx->mds0_ctrls.pruning_method_th != (uint8_t)~0 &&
            (MIN(ctx->md_me_dist, ctx->md_pme_dist) / (ctx->blk_geom->bwidth * ctx->blk_geom->bheight)) >
                ctx->mds0_ctrls.pruning_method_th) {
            if (ctx->mds0_ctrls.per_class_dist_to_cost_th[cand_bf->cand->cand_class] != (uint16_t)~0 &&
                ctx->mds0_best_cost_per_class[cand_bf->cand->cand_class] != (uint64_t)~0) {
                const uint64_t distortion_cost = RDCOST(full_lambda, 0, luma_fast_dist);
                if ((100 *
                     (int64_t)((int64_t)distortion_cost -
                               (int64_t)ctx->mds0_best_cost_per_class[cand_bf->cand->cand_class])) >
                    ((int64_t)ctx->mds0_best_cost_per_class[cand_bf->cand->cand_class] *
                     ctx->mds0_ctrls.per_class_dist_to_cost_th[cand_bf->cand->cand_class])) {
                    *(cand_bf->fast_cost) = MAX_MODE_COST;
                    return;
                }
            }
        } else {
            if (ctx->mds0_ctrls.dist_to_cost_th != (uint16_t)~0 && ctx->mds0_best_cost != (uint64_t)~0) {
                const uint64_t distortion_cost = RDCOST(full_lambda, 0, luma_fast_dist);
                if ((100 * (int64_t)((int64_t)distortion_cost - (int64_t)ctx->mds0_best_cost)) >
                    ((int64_t)ctx->mds0_best_cost * ctx->mds0_ctrls.dist_to_cost_th)) {
                    *(cand_bf->fast_cost) = MAX_MODE_COST;
                    return;
                }
            }
        }
    }
    // Fast Cost
    if (ctx->shut_fast_rate) {
        *(cand_bf->fast_cost)     = luma_fast_dist;
        cand_bf->fast_luma_rate   = 0;
        cand_bf->fast_chroma_rate = 0;
    } else {
        *(cand_bf->fast_cost) = av1_product_fast_cost_func_table[is_inter_mode(cand->block_mi.mode)](
            pcs, ctx, cand_bf, full_lambda, luma_fast_dist);
    }
    cand_bf->valid_luma_pred = 1;

    if (ctx->obmc_ctrls.enabled && ctx->obmc_ctrls.trans_face_off == 1) {
        obmc_trans_face_off(cand_bf, pcs, ctx, input_pic, loc);
    }
    // Init full cost in case we bypass stage1/stage2
    *(cand_bf->full_cost) = *(cand_bf->fast_cost);
}

/* Set the max number of NICs for each MD stage, based on the picture type and scaling settings.

   pic_type = I_SLICE ? 0 : REF ? 1 : 2;
*/
void svt_aom_set_nics(SequenceControlSet* scs, NicScalingCtrls* scaling_ctrls, uint32_t mds1_count[CAND_CLASS_TOTAL],
                      uint32_t mds2_count[CAND_CLASS_TOTAL], uint32_t mds3_count[CAND_CLASS_TOTAL], uint8_t pic_type,
                      uint32_t qp) {
    for (CandClass cidx = CAND_CLASS_0; cidx < CAND_CLASS_TOTAL; cidx++) {
        mds1_count[cidx] = MD_STAGE_NICS[pic_type][cidx];
        mds2_count[cidx] = MD_STAGE_NICS[pic_type][cidx] >> 1;
        mds3_count[cidx] = MD_STAGE_NICS[pic_type][cidx] >> 2;
    }

    // minimum nics allowed
    uint8_t min_mds1_nics = (pic_type < 2 && scaling_ctrls->stage1_scaling_num) ? 2 : 1;
    uint8_t min_mds2_nics = (pic_type < 2 && scaling_ctrls->stage2_scaling_num) ? 2 : 1;
    uint8_t min_mds3_nics = (pic_type < 2 && scaling_ctrls->stage3_scaling_num) ? 2 : 1;

    // Set the scaling numerators
    uint32_t stage1_num = scaling_ctrls->stage1_scaling_num;
    uint32_t stage2_num = scaling_ctrls->stage2_scaling_num;
    uint32_t stage3_num = scaling_ctrls->stage3_scaling_num;
    // The scaling denominator is 16 for all stages
    uint32_t scale_denum = MD_STAGE_NICS_SCAL_DENUM;
    // no NIC setting should be done beyond this point
    for (CandClass cidx = 0; cidx < CAND_CLASS_TOTAL; ++cidx) {
        mds1_count[cidx] = MAX(min_mds1_nics, DIVIDE_AND_ROUND(mds1_count[cidx] * stage1_num, scale_denum));
        mds2_count[cidx] = MAX(min_mds2_nics, DIVIDE_AND_ROUND(mds2_count[cidx] * stage2_num, scale_denum));
        mds3_count[cidx] = MAX(min_mds3_nics, DIVIDE_AND_ROUND(mds3_count[cidx] * stage3_num, scale_denum));
    }
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(
        scs->qp_based_th_scaling_ctrls.nic_max_qp_based_th_scaling, &q_weight, &q_weight_denom, qp);
    for (CandClass cidx = 0; cidx < CAND_CLASS_TOTAL; ++cidx) {
        mds1_count[cidx] = MAX(min_mds1_nics, DIVIDE_AND_ROUND(mds1_count[cidx] * q_weight, q_weight_denom));
        mds2_count[cidx] = MAX(min_mds2_nics, DIVIDE_AND_ROUND(mds2_count[cidx] * q_weight, q_weight_denom));
        mds3_count[cidx] = MAX(min_mds3_nics, DIVIDE_AND_ROUND(mds3_count[cidx] * q_weight, q_weight_denom));
    }
}

void set_md_stage_counts(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    // Step 1: Set the number of NICs for each stage
    // no NIC setting should be done beyond this point
    // Set md_stage count
    uint8_t pic_type = pcs->slice_type == I_SLICE ? 0 : !pcs->ppcs->is_highest_layer ? 1 : 2;
    svt_aom_set_nics(pcs->scs,
                     &ctx->nic_ctrls.scaling_ctrls,
                     ctx->md_stage_1_count,
                     ctx->md_stage_2_count,
                     ctx->md_stage_3_count,
                     pic_type,
                     pcs->ppcs->scs->static_config.qp);

    // Step 2: derive bypass_stage1 flags
    ctx->bypass_md_stage_1 = (ctx->nic_ctrls.md_staging_mode == MD_STAGING_MODE_1 ||
                              ctx->nic_ctrls.md_staging_mode == MD_STAGING_MODE_2)
        ? false
        : true;
    ctx->bypass_md_stage_2 = (ctx->nic_ctrls.md_staging_mode == MD_STAGING_MODE_2) ? false : true;
}

static void sort_fast_cost_based_candidates(
    ModeDecisionContext* ctx, uint32_t input_buffer_start_idx,
    uint32_t  input_buffer_count, //how many cand buffers to sort. one of the buffer can have max cost.
    uint32_t* cand_buff_indices) {
    ModeDecisionCandidateBuffer** buffer_ptr_array     = ctx->cand_bf_ptr_array;
    uint32_t                      input_buffer_end_idx = input_buffer_start_idx + input_buffer_count - 1;
    uint32_t                      buffer_index, i, j;
    uint32_t                      k = 0;
    for (buffer_index = input_buffer_start_idx; buffer_index <= input_buffer_end_idx; buffer_index++, k++) {
        cand_buff_indices[k] = buffer_index;
    }
    for (i = 0; i < input_buffer_count - 1; ++i) {
        for (j = i + 1; j < input_buffer_count; ++j) {
            if (*(buffer_ptr_array[cand_buff_indices[j]]->fast_cost) <
                *(buffer_ptr_array[cand_buff_indices[i]]->fast_cost)) {
                buffer_index         = cand_buff_indices[i];
                cand_buff_indices[i] = (uint32_t)cand_buff_indices[j];
                cand_buff_indices[j] = (uint32_t)buffer_index;
            }
        }
    }
}

void sort_full_cost_based_candidates(ModeDecisionContext* ctx, uint32_t num_of_cand_to_sort,
                                     uint32_t* cand_buff_indices) {
    uint32_t                      i, j, index;
    ModeDecisionCandidateBuffer** buffer_ptr_array = ctx->cand_bf_ptr_array;
    for (i = 0; i < num_of_cand_to_sort - 1; ++i) {
        for (j = i + 1; j < num_of_cand_to_sort; ++j) {
            if (*(buffer_ptr_array[cand_buff_indices[j]]->full_cost) <
                *(buffer_ptr_array[cand_buff_indices[i]]->full_cost)) {
                index                = cand_buff_indices[i];
                cand_buff_indices[i] = (uint32_t)cand_buff_indices[j];
                cand_buff_indices[j] = (uint32_t)index;
            }
        }
    }
}

static void construct_best_sorted_arrays_md_stage_3(
    ModeDecisionContext* ctx,
    uint32_t*            best_candidate_index_array) { //best = union from all classes

    uint32_t best_candi = 0;
    for (CandClass class_i = CAND_CLASS_0; class_i < CAND_CLASS_TOTAL; class_i++) {
        for (uint32_t candi = 0; candi < ctx->md_stage_3_count[class_i]; candi++) {
            best_candidate_index_array[best_candi++] = ctx->cand_buff_indices[class_i][candi];
        }
    }

    assert(best_candi == ctx->md_stage_3_total_count);
}

/* Determine if independent chroma search should be performed. Function is called when the
independent chroma search is set to be performed before the last MD stage.

The chroma search may be skipped if there are no intra candidates, or based on speed features.*/
static bool perform_ind_uv_search_last_mds(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer** buffer_ptr_array,
                                           uint32_t* best_cand_idx_array) {
    const uint32_t mds3_cand_count  = ctx->md_stage_3_total_count;
    uint16_t       mds3_intra_count = 0;
    uint64_t       best_intra_cost  = MAX_MODE_COST;
    uint64_t       best_inter_cost  = MAX_MODE_COST;
    for (uint32_t i = 0; i < mds3_cand_count; ++i) {
        uint32_t   id       = best_cand_idx_array[i];
        const bool is_inter = (is_inter_mode(buffer_ptr_array[id]->cand->block_mi.mode) ||
                               buffer_ptr_array[id]->cand->block_mi.use_intrabc);
        // If independent chroma search is to be skipped when there is only UV_DC_PRED modes, don't count UV_DC_PRED
        mds3_intra_count += !is_inter &&
                (!ctx->uv_ctrls.skip_ind_uv_if_only_dc || buffer_ptr_array[id]->cand->block_mi.uv_mode != UV_DC_PRED)
            ? 1
            : 0;
        if (is_inter) {
            if (*buffer_ptr_array[id]->full_cost < best_inter_cost) {
                best_inter_cost = *buffer_ptr_array[id]->full_cost;
            }
        } else {
            if (*buffer_ptr_array[id]->full_cost < best_intra_cost) {
                best_intra_cost = *buffer_ptr_array[id]->full_cost;
            }
        }
    }

    // Update md_stage_3_total_intra_count based based on inter/intra cost deviation
    if (ctx->uv_ctrls.inter_vs_intra_cost_th &&
        (best_inter_cost * ctx->uv_ctrls.inter_vs_intra_cost_th) < (best_intra_cost * 100)) {
        mds3_intra_count = 0;
    }

    return (mds3_intra_count > 0);
}

static void md_stage_0_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t fast_cand_count,
                                 EbPictureBufferDesc* input_pic, uint32_t input_origin_index,
                                 uint32_t blk_origin_index) {
    uint32_t cand_buff_idx = 0;
    for (uint32_t cand_idx = 0; cand_idx < fast_cand_count; cand_idx++) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[cand_buff_idx];
        cand_bf->cand                        = &ctx->fast_cand_array[cand_idx];

        // Initialize tx_depth
        cand_bf->cand->block_mi.tx_depth = 0;
        fast_loop_core_light_pd0(cand_bf, pcs, ctx, input_pic, input_origin_index, blk_origin_index);
        if (*cand_bf->fast_cost < ctx->mds0_best_cost) {
            ctx->mds0_best_cost = *cand_bf->fast_cost;
            ctx->mds0_best_idx  = cand_buff_idx;
            cand_buff_idx       = !cand_buff_idx;
        }
    }
}

static void md_stage_0_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                 ModeDecisionCandidateBuffer** cand_bf_ptr_array_base,
                                 ModeDecisionCandidate* fast_cand_array, uint32_t fast_cand_count,
                                 EbPictureBufferDesc* input_pic, BlockLocation* loc) {
    // Set MD Staging fast_loop_core settings
    ctx->mds_do_chroma = false;
    /* If the interpolation filter type is assigned at the picture level, use that value, OW use regular.
     * NB intra_bc always uses BILINEAR, but IBC is not allowed in LPD1. */
    const InterpFilter default_interp_filter = (pcs->ppcs->frm_hdr.interpolation_filter == SWITCHABLE)
        ? 0
        : av1_broadcast_interp_filter(pcs->ppcs->frm_hdr.interpolation_filter);

    // 2nd fast loop: src-to-recon
    uint32_t cand_buff_idx = 0;

    for (uint32_t cand_idx = 0; cand_idx < fast_cand_count; cand_idx++) {
        ModeDecisionCandidateBuffer* cand_bf = cand_bf_ptr_array_base[cand_buff_idx];
        cand_bf->cand                        = &fast_cand_array[cand_idx];

        cand_bf->cand->block_mi.tx_depth       = 0;
        cand_bf->cand->block_mi.interp_filters = default_interp_filter;
        // Prediction
        fast_loop_core_light_pd1(cand_bf, pcs, ctx, input_pic, loc);

        if (*cand_bf->fast_cost < ctx->mds0_best_cost) {
            ctx->mds0_best_cost = *cand_bf->fast_cost;
            ctx->mds0_best_idx  = cand_buff_idx;
            cand_buff_idx       = !cand_buff_idx;
        }
    }
}

// returns true if the candidate should be processed during the current MDS0 iteration, false otherwise.
// Function will only be applicate to classes which use multiple iterations
static bool process_cand_itr(ModeDecisionContext* ctx, ModeDecisionCandidate* cand, uint8_t itr,
                             PredictionMode best_reg_intra_mode, uint64_t best_reg_intra_cost,
                             const uint64_t regular_intra_cost[PAETH_PRED + 1]) {
    if (itr == 0) {
        if (ctx->cand_reduction_ctrls.reduce_filter_intra &&
            (ctx->intra_ctrls.skip_angular_delta1_th != -1 || ctx->intra_ctrls.skip_angular_delta2_th != -1 ||
             ctx->intra_ctrls.skip_angular_delta3_th != -1)) {
            // Eval regular only if itr=0 (i.e. skip angular and skip filter)
            if (cand->block_mi.angle_delta[PLANE_TYPE_Y] != 0 ||
                cand->block_mi.filter_intra_mode != FILTER_INTRA_MODES) {
                return false;
            }
        } else if (ctx->intra_ctrls.skip_angular_delta1_th != -1 || ctx->intra_ctrls.skip_angular_delta2_th != -1 ||
                   ctx->intra_ctrls.skip_angular_delta3_th != -1) {
            // Eval regular and filter intra only if itr=0 (i.e. skip angular)
            if (cand->block_mi.angle_delta[PLANE_TYPE_Y] != 0) {
                return false;
            }
        } else { // if (ctx->cand_reduction_ctrls.reduce_filter_intra)
            // Eval regular and angular only if itr=0 (i.e. skip filter)
            if (cand->block_mi.filter_intra_mode != FILTER_INTRA_MODES) {
                return false;
            }
        }
    } else {
        if (ctx->cand_reduction_ctrls.reduce_filter_intra &&
            (ctx->intra_ctrls.skip_angular_delta1_th != -1 || ctx->intra_ctrls.skip_angular_delta2_th != -1 ||
             ctx->intra_ctrls.skip_angular_delta3_th != -1)) {
            // Eval angular and filter intra if itr=1 (i.e. skip regular)
            if (cand->block_mi.angle_delta[PLANE_TYPE_Y] == 0 &&
                cand->block_mi.filter_intra_mode == FILTER_INTRA_MODES) {
                return false;
            }
        } else if ((ctx->intra_ctrls.skip_angular_delta1_th != -1 || ctx->intra_ctrls.skip_angular_delta2_th != -1 ||
                    ctx->intra_ctrls.skip_angular_delta3_th != -1)) {
            // Eval angular only if itr=1 (i.e. skip regular and skip filter intra)
            if (cand->block_mi.angle_delta[PLANE_TYPE_Y] == 0) {
                return false;
            }
        } else { // if (ctx->cand_reduction_ctrls.reduce_filter_intra)
            // Eval filter intra only if itr=1 (i.e. skip regular and skip angular)
            if (cand->block_mi.filter_intra_mode == FILTER_INTRA_MODES) {
                return false;
            }
        }
        // Use regular info to reduce angular processing
        // Eval the child-angular if the parent-angular is the best out of the regular modes (i.e. itr=0 eval)
        if (cand->block_mi.filter_intra_mode == FILTER_INTRA_MODES) {
            if (best_reg_intra_mode != cand->block_mi.mode) {
                if (ctx->intra_ctrls.skip_angular_delta1_th != -1 &&
                    (abs(cand->block_mi.angle_delta[PLANE_TYPE_Y]) == 1) &&
                    (((regular_intra_cost[cand->block_mi.mode] - MAX(best_reg_intra_cost, 1)) * 100) >
                     ctx->intra_ctrls.skip_angular_delta1_th * best_reg_intra_cost)) {
                    return false;
                } else if (ctx->intra_ctrls.skip_angular_delta2_th != -1 &&
                           (abs(cand->block_mi.angle_delta[PLANE_TYPE_Y]) == 2) &&
                           (((regular_intra_cost[cand->block_mi.mode] - MAX(best_reg_intra_cost, 1)) * 100) >
                            ctx->intra_ctrls.skip_angular_delta2_th * best_reg_intra_cost)) {
                    return false;
                } else if (ctx->intra_ctrls.skip_angular_delta3_th != -1 &&
                           (abs(cand->block_mi.angle_delta[PLANE_TYPE_Y]) == 3) &&
                           (((regular_intra_cost[cand->block_mi.mode] - MAX(best_reg_intra_cost, 1)) * 100) >
                            ctx->intra_ctrls.skip_angular_delta3_th * best_reg_intra_cost)) {
                    return false;
                }
            }
        } else {
            // Eval filter candidates
            // Always test FILTER_DC candidate
            if (cand->block_mi.filter_intra_mode == FILTER_DC_PRED) {
                return true;
            }

            // Only test other filter modes if the corresponding non-filter mode was chosen during the first pass
            if (fimode_to_intramode[cand->block_mi.filter_intra_mode] != best_reg_intra_mode) {
                return false;
            }
        }
    }
    return true;
}

static void md_stage_0(PictureControlSet* pcs, ModeDecisionContext* ctx,
                       ModeDecisionCandidateBuffer** cand_bf_ptr_array_base,
                       ModeDecisionCandidate* fast_candidate_array, uint32_t fast_cand_count,
                       EbPictureBufferDesc* input_pic, BlockLocation* loc, uint32_t cand_bf_start_index,
                       uint32_t max_buffers) {
    const uint8_t apply_unipred_bias = pcs->scs->vq_ctrls.sharpness_ctrls.unipred_bias && pcs->ppcs->is_noise_level;
    // Set MD Staging fast_loop_core settings
    ctx->mds_do_ifs = (ctx->ifs_ctrls.level == IFS_MDS0);
    /* If the interpolation filter type is known, assign it, OW will be assigned in IFS search.
     * NB intra_bc always uses BILINEAR, but the IBC filters are updated automatically during prediction, so
     * no need for a special check here. */
    const InterpFilter default_interp_filter = (pcs->ppcs->frm_hdr.interpolation_filter == SWITCHABLE)
        ? 0
        : av1_broadcast_interp_filter(pcs->ppcs->frm_hdr.interpolation_filter);
    ctx->mds_do_chroma                       = false;
    // 2nd fast loop: src-to-recon
    uint32_t highest_cost_index = cand_bf_start_index;

    // Process CLASS_0 candidates through iterations at mds0;
    // 1st iteration : evaluate the regular mode(s).
    //
    // 2nd iteration : evaluate the angular (pAngle!=0) mode(s) and filter-intra modes
    //    - skip pAngle !=0 of a given mode, if pAngle==0 of the same mode is not the best after the 1st iteration
    //      and if the cost to the best is significant
    //    - skip filter-intra mode if regular winner does not match (e.g. test filter-paeth if Paeth is best at first pass). Always test filter-DC.
    uint8_t        tot_itr             = (ctx->target_class != CAND_CLASS_0 ||
                       (!ctx->cand_reduction_ctrls.reduce_filter_intra &&
                        !((ctx->intra_ctrls.skip_angular_delta1_th != -1 ||
                           ctx->intra_ctrls.skip_angular_delta2_th != -1 ||
                           ctx->intra_ctrls.skip_angular_delta3_th != -1))))
                           ? 1
                           : 2;
    uint64_t       best_reg_intra_cost = MAX_CU_COST; // Derived at the 1st itr
    PredictionMode best_reg_intra_mode = INTRA_INVALID; // Derived at the 1st itr
    uint64_t       regular_intra_cost[PAETH_PRED + 1];
    for (unsigned i = 0; i < PAETH_PRED + 1; i++) {
        regular_intra_cost[i] = MAX_CU_COST;
    }

    uint32_t tot_processed_cand = 0;

    for (uint8_t itr = 0; itr < tot_itr; itr++) {
        for (uint32_t cand_idx = 0; cand_idx < fast_cand_count; cand_idx++) {
            if (fast_candidate_array[cand_idx].cand_class != ctx->target_class) {
                continue;
            }

            ModeDecisionCandidateBuffer* cand_bf = cand_bf_ptr_array_base[highest_cost_index];
            ModeDecisionCandidate*       cand = cand_bf->cand = &fast_candidate_array[cand_idx];
            if (ctx->intra_ctrls.prune_using_best_mode && cand->cand_class == CAND_CLASS_0 && itr == 0) {
                // If (V better than DC), then skip H
                if (cand->block_mi.mode == H_PRED && best_reg_intra_mode == V_PRED) {
                    continue;
                }
                // If DC better than H and better than V, then skip Smooth
                if ((cand->block_mi.mode == SMOOTH_PRED || cand->block_mi.mode == SMOOTH_V_PRED ||
                     cand->block_mi.mode == SMOOTH_H_PRED) &&
                    best_reg_intra_mode == DC_PRED) {
                    continue;
                }
            }
            cand->block_mi.tx_depth       = 0;
            cand->block_mi.interp_filters = default_interp_filter;
            // Check whether a candidate should be considered in the current iteration
            if (tot_itr > 1) {
                if (!process_cand_itr(ctx, cand, itr, best_reg_intra_mode, best_reg_intra_cost, regular_intra_cost)) {
                    continue;
                }
            }

            // Perform prediction and calculate cost
            fast_loop_core(cand_bf, pcs, ctx, input_pic, loc);

            tot_processed_cand++;

            if (apply_unipred_bias && is_inter_singleref_mode(cand_bf->cand->block_mi.mode)) {
                *cand_bf->fast_cost = (*cand_bf->fast_cost * uni_psy_bias[pcs->ppcs->picture_qp]) / 100;
            }
            if (*cand_bf->fast_cost < ctx->mds0_best_cost) {
                ctx->mds0_best_cost  = *cand_bf->fast_cost;
                ctx->mds0_best_class = cand->cand_class;
                if (cand->cand_class == CAND_CLASS_0) {
                    ctx->mds0_best_class0_cost = *cand_bf->fast_cost;
                }
            }
            if (*cand_bf->fast_cost < ctx->mds0_best_cost_per_class[cand_bf->cand->cand_class]) {
                ctx->mds0_best_cost_per_class[cand_bf->cand->cand_class] = *cand_bf->fast_cost;
            }
            if (cand->cand_class == CAND_CLASS_0 && itr == 0 &&
                cand->block_mi.filter_intra_mode == FILTER_INTRA_MODES &&
                ((ctx->intra_ctrls.prune_using_best_mode && ctx->intra_ctrls.intra_mode_end >= H_PRED) ||
                 tot_itr > 1)) {
                regular_intra_cost[cand->block_mi.mode] = *cand_bf->fast_cost;

                if (*cand_bf->fast_cost < best_reg_intra_cost) {
                    best_reg_intra_cost = *cand_bf->fast_cost;
                    best_reg_intra_mode = cand->block_mi.mode;
                }
            }

            // Get the candidate buffer to use for processing the next candidate.
            // The buffer will be an empty buffer or the buffer of the candidate with the
            // highest cost so far.
            if (tot_processed_cand < max_buffers) {
                highest_cost_index++;
            } else {
                const uint64_t* fast_cost_array    = ctx->fast_cost_array;
                const uint32_t  buffer_index_start = cand_bf_start_index;
                const uint32_t  buffer_index_end   = buffer_index_start + max_buffers;
                if (max_buffers == 2) {
                    highest_cost_index = fast_cost_array[buffer_index_start] < fast_cost_array[buffer_index_start + 1]
                        ? buffer_index_start + 1
                        : buffer_index_start;
                } else {
                    highest_cost_index    = buffer_index_start;
                    uint64_t highest_cost = fast_cost_array[buffer_index_start];
                    for (uint32_t buff = buffer_index_start + 1; buff < buffer_index_end; buff++) {
                        if (fast_cost_array[buff] > highest_cost) {
                            highest_cost_index = buff;
                            highest_cost       = fast_cost_array[buff];
                        }
                    }
                }
            }
        }
    }

    //if pruning happened, update MDS1 count accordingly to not process invalid candidates in subsequent MD stages
    ctx->md_stage_1_count[ctx->target_class] = MIN(ctx->md_stage_1_count[ctx->target_class], tot_processed_cand);

    // Set the cost of the scratch candidate to max to get discarded @ the sorting phase
    *(cand_bf_ptr_array_base[highest_cost_index]->fast_cost) = MAX_CU_COST;
}

void svt_pme_sad_loop_kernel_c(const svt_mv_cost_param* mv_cost_params,
                               uint8_t*                 src, // input parameter, source samples Ptr
                               uint32_t                 src_stride, // input parameter, source stride
                               uint8_t*                 ref, // input parameter, reference samples Ptr
                               uint32_t                 ref_stride, // input parameter, reference stride
                               uint32_t                 block_height, // input parameter, block height (M)
                               uint32_t                 block_width, // input parameter, block width (N)
                               uint32_t* best_cost, int16_t* best_mvx, int16_t* best_mvy,
                               int16_t search_position_start_x, int16_t search_position_start_y,
                               int16_t search_area_width, int16_t search_area_height, int16_t search_step, int16_t mvx,
                               int16_t mvy) {
    int16_t xSearchIndex;
    int16_t ySearchIndex;
    int16_t col_num       = 0;
    int16_t search_step_x = 1;
    for (ySearchIndex = 0; ySearchIndex < search_area_height; ySearchIndex += search_step) {
        for (xSearchIndex = 0; xSearchIndex < search_area_width; xSearchIndex += search_step_x) {
            if (((search_area_width - xSearchIndex) < 8) && (col_num == 0)) {
                continue;
            }
            if (col_num == 7) {
                col_num       = 0;
                search_step_x = search_step;
            } else {
                col_num++;
                search_step_x = 1;
            }
            uint32_t x, y;
            uint32_t cost = 0;

            for (y = 0; y < block_height; y++) {
                for (x = 0; x < block_width; x++) {
                    cost += EB_ABS_DIFF(src[y * src_stride + x], ref[xSearchIndex + y * ref_stride + x]);
                }
            }

            Mv       best_mv;
            uint32_t refinement_pos_x = search_position_start_x + xSearchIndex;
            uint32_t refinement_pos_y = search_position_start_y + ySearchIndex;
            best_mv.x                 = mvx + (refinement_pos_x * 8);
            best_mv.y                 = mvy + (refinement_pos_y * 8);
            cost += svt_aom_fp_mv_err_cost(&best_mv, mv_cost_params);
            if (cost < *best_cost) {
                *best_mvx  = mvx + (refinement_pos_x * 8);
                *best_mvy  = mvy + (refinement_pos_y * 8);
                *best_cost = cost;
            }
        }

        ref += search_step * ref_stride;
    }

    return;
}

static void md_full_pel_search_large_lbd(svt_mv_cost_param* mv_cost_params, ModeDecisionContext* ctx,
                                         EbPictureBufferDesc* input_pic, EbPictureBufferDesc* ref_pic,
                                         uint32_t input_origin_index, int16_t mvx, int16_t mvy,
                                         int16_t search_position_start_x, int16_t search_position_end_x,
                                         int16_t search_position_start_y, int16_t search_position_end_y,
                                         int16_t sparse_search_step, int16_t* best_mvx, int16_t* best_mvy,
                                         uint32_t* best_cost) {
    //We cannot use sparse_search_step with mpsad for search_position_start_x/search_position_end_x,
    //So for x dimension we assume sparse_search_step is always 1

    int32_t ref_origin_index = ref_pic->org_x + (ctx->blk_org_x + (mvx >> 3) + search_position_start_x) +
        (ctx->blk_org_y + (mvy >> 3) + ref_pic->org_y + search_position_start_y) * ref_pic->stride_y;

    int16_t remain_search_area  = 8 - ((search_position_end_x - search_position_start_x) % 8);
    remain_search_area          = remain_search_area == 8 ? 0 : remain_search_area;
    search_position_end_x       = MAX(search_position_end_x, search_position_end_x + remain_search_area);
    uint32_t search_area_width  = search_position_end_x - search_position_start_x;
    uint32_t search_area_height = search_position_end_y - search_position_start_y + 1;
    assert(!(search_area_width & 7));
    if (search_area_width & 0xfffffff8) {
        svt_pme_sad_loop_kernel(mv_cost_params,
                                input_pic->buffer_y + input_origin_index,
                                input_pic->stride_y,
                                ref_pic->buffer_y + ref_origin_index,
                                ref_pic->stride_y,
                                ctx->blk_geom->bheight,
                                ctx->blk_geom->bwidth,
                                best_cost,
                                best_mvx,
                                best_mvy,
                                search_position_start_x,
                                search_position_start_y,
                                (search_area_width & 0xfffffff8), //pass search_area_width multiple by 8
                                search_area_height,
                                sparse_search_step,
                                mvx,
                                mvy);
    }

    if (search_area_width & 7) {
        uint32_t cost;
        for (int32_t refinement_pos_y = search_position_start_y; refinement_pos_y <= search_position_end_y;
             refinement_pos_y         = refinement_pos_y + sparse_search_step) {
            int32_t refinement_pos_x = search_position_start_x + (search_area_width & 0xfffffff8);
            for (; refinement_pos_x <= search_position_end_x; refinement_pos_x++) {
                ref_origin_index = ref_pic->org_x + (ctx->blk_org_x + (mvx >> 3) + refinement_pos_x) +
                    (ctx->blk_org_y + (mvy >> 3) + ref_pic->org_y + refinement_pos_y) * ref_pic->stride_y;

                assert((ctx->blk_geom->bwidth >> 3) < 17);

                cost = svt_nxm_sad_kernel(input_pic->buffer_y + input_origin_index,
                                          input_pic->stride_y,
                                          ref_pic->buffer_y + ref_origin_index,
                                          ref_pic->stride_y,
                                          ctx->blk_geom->bheight,
                                          ctx->blk_geom->bwidth);

                Mv best_mv;
                best_mv.x = mvx + (refinement_pos_x * 8);
                best_mv.y = mvy + (refinement_pos_y * 8);
                cost += svt_aom_fp_mv_err_cost(&best_mv, mv_cost_params);
                if (cost < *best_cost) {
                    *best_mvx  = mvx + (refinement_pos_x * 8);
                    *best_mvy  = mvy + (refinement_pos_y * 8);
                    *best_cost = cost;
                }
            }
        }
    }
}

static void svt_init_mv_cost_params(svt_mv_cost_param* mv_cost_params, ModeDecisionContext* ctx, const Mv* ref_mv,
                                    uint8_t base_q_idx, uint32_t rdmult, uint8_t hbd_md) {
    mv_cost_params->ref_mv        = ref_mv;
    mv_cost_params->full_ref_mv   = get_fullmv_from_mv(ref_mv);
    mv_cost_params->early_exit_th = 1020 - (ctx->blk_geom->sq_size >> 2);
    mv_cost_params->mv_cost_type  = ctx->md_subpel_me_ctrls.skip_diag_refinement >= 3 ? MV_COST_OPT : MV_COST_ENTROPY;
    mv_cost_params->error_per_bit = AOMMAX(rdmult >> RD_EPB_SHIFT, 1);
    mv_cost_params->sad_per_bit   = svt_aom_get_sad_per_bit(base_q_idx, hbd_md);
    mv_cost_params->mvjcost       = ctx->md_rate_est_ctx->nmv_vec_cost;
    mv_cost_params->mvcost[0]     = ctx->md_rate_est_ctx->nmvcoststack[0];
    mv_cost_params->mvcost[1]     = ctx->md_rate_est_ctx->nmvcoststack[1];
}

static void md_full_pel_search(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                               EbPictureBufferDesc* ref_pic, uint32_t input_origin_index, DistortionType dist_type,
                               int16_t mvx, int16_t mvy, int16_t search_position_start_x, int16_t search_position_end_x,
                               int16_t search_position_start_y, int16_t search_position_end_y,
                               int16_t sparse_search_step, uint8_t is_sprs_lev0_performed, int16_t* best_mvx,
                               int16_t* best_mvy, uint32_t* best_cost, uint8_t hbd_md) {
    // Mvcost params
    svt_mv_cost_param mv_cost_params;
    FrameHeader*      frm_hdr = &pcs->ppcs->frm_hdr;
    uint32_t          rdmult  = dist_type != SAD ? ctx->full_lambda_md[hbd_md ? EB_10_BIT_MD : EB_8_BIT_MD]
                                                 : ctx->fast_lambda_md[hbd_md ? EB_10_BIT_MD : EB_8_BIT_MD];
    svt_init_mv_cost_params(
        &mv_cost_params, ctx, &ctx->ref_mv, frm_hdr->quantization_params.base_q_idx, rdmult, hbd_md);
    uint32_t cost;
    // Search area adjustment
    if ((ctx->blk_org_x + (mvx >> 3) + search_position_start_x) < (-ref_pic->org_x + 1)) {
        search_position_start_x = (-ref_pic->org_x + 1) - (ctx->blk_org_x + (mvx >> 3));
    }

    if ((ctx->blk_org_x + ctx->blk_geom->bwidth + (mvx >> 3) + search_position_end_x) >
        (ref_pic->org_x + ref_pic->max_width - 1)) {
        search_position_end_x = (ref_pic->org_x + ref_pic->max_width - 1) -
            (ctx->blk_org_x + ctx->blk_geom->bwidth + (mvx >> 3));
    }

    if ((ctx->blk_org_y + (mvy >> 3) + search_position_start_y) < (-ref_pic->org_y + 1)) {
        search_position_start_y = (-ref_pic->org_y + 1) - (ctx->blk_org_y + (mvy >> 3));
    }

    if ((ctx->blk_org_y + ctx->blk_geom->bheight + (mvy >> 3) + search_position_end_y) >
        (ref_pic->org_y + ref_pic->max_height - 1)) {
        search_position_end_y = (ref_pic->org_y + ref_pic->max_height - 1) -
            (ctx->blk_org_y + ctx->blk_geom->bheight + (mvy >> 3));
    }
    if (dist_type == SAD && ctx->enable_psad) {
        if (!hbd_md && (search_position_end_x - search_position_start_x) >= 7) {
            md_full_pel_search_large_lbd(&mv_cost_params,
                                         ctx,
                                         input_pic,
                                         ref_pic,
                                         input_origin_index,
                                         mvx,
                                         mvy,
                                         search_position_start_x,
                                         search_position_end_x,
                                         search_position_start_y,
                                         search_position_end_y,
                                         sparse_search_step,
                                         best_mvx,
                                         best_mvy,
                                         best_cost);
            return;
        }
    }
    for (int32_t refinement_pos_x = search_position_start_x; refinement_pos_x <= search_position_end_x;
         refinement_pos_x         = refinement_pos_x + sparse_search_step) {
        for (int32_t refinement_pos_y = search_position_start_y; refinement_pos_y <= search_position_end_y;
             refinement_pos_y         = refinement_pos_y + sparse_search_step) {
            // If sparse search level_1, and if search level_0 previously performed
            if (sparse_search_step == 2 && is_sprs_lev0_performed) {
                // If level_0 range
                if ((refinement_pos_x + (mvx >> 3)) >= ctx->sprs_lev0_start_x &&
                    (refinement_pos_x + (mvx >> 3)) <= ctx->sprs_lev0_end_x &&
                    (refinement_pos_y + (mvy >> 3)) >= ctx->sprs_lev0_start_y &&
                    (refinement_pos_y + (mvy >> 3)) <= ctx->sprs_lev0_end_y) {
                    // If level_0 position
                    if (refinement_pos_x % 4 == 0 && refinement_pos_y % 4 == 0) {
                        continue;
                    }
                }
            }
            int32_t ref_origin_index = ref_pic->org_x + (ctx->blk_org_x + (mvx >> 3) + refinement_pos_x) +
                (ctx->blk_org_y + (mvy >> 3) + ref_pic->org_y + refinement_pos_y) * ref_pic->stride_y;

            if (dist_type == VAR) {
                if (!hbd_md) {
                    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
                    unsigned int            sse;
                    uint8_t*                pred_y = ref_pic->buffer_y + ref_origin_index;
                    uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
                    cost = fn_ptr->vf(pred_y, ref_pic->stride_y, src_y, input_pic->stride_y, &sse);
                } else {
                    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
                    unsigned int            sse;
                    uint16_t*               pred_y = ((uint16_t*)ref_pic->buffer_y) + ref_origin_index;
                    uint16_t*               src_y  = ((uint16_t*)input_pic->buffer_y) + input_origin_index;
                    cost                           = fn_ptr->vf_hbd_10(CONVERT_TO_BYTEPTR(pred_y),
                                             ref_pic->stride_y,
                                             CONVERT_TO_BYTEPTR(src_y),
                                             input_pic->stride_y,
                                             &sse);
                }
            } else if (dist_type == SSD) {
                EbSpatialFullDistType spatial_full_dist_type_fun = hbd_md ? svt_full_distortion_kernel16_bits
                                                                          : svt_spatial_full_distortion_kernel;

                cost = (uint32_t)spatial_full_dist_type_fun(input_pic->buffer_y,
                                                            input_origin_index,
                                                            input_pic->stride_y,
                                                            ref_pic->buffer_y,
                                                            ref_origin_index,
                                                            ref_pic->stride_y,
                                                            ctx->blk_geom->bwidth,
                                                            ctx->blk_geom->bheight);
            } else {
                assert((ctx->blk_geom->bwidth >> 3) < 17);

                if (hbd_md) {
                    cost = sad_16b_kernel(((uint16_t*)input_pic->buffer_y) + input_origin_index,
                                          input_pic->stride_y,
                                          ((uint16_t*)ref_pic->buffer_y) + ref_origin_index,
                                          ref_pic->stride_y,
                                          ctx->blk_geom->bheight,
                                          ctx->blk_geom->bwidth);
                } else {
                    cost = svt_nxm_sad_kernel(input_pic->buffer_y + input_origin_index,
                                              input_pic->stride_y,
                                              ref_pic->buffer_y + ref_origin_index,
                                              ref_pic->stride_y,
                                              ctx->blk_geom->bheight,
                                              ctx->blk_geom->bwidth);
                }
            }
            Mv best_mv;
            best_mv.x = mvx + (refinement_pos_x * 8);
            best_mv.y = mvy + (refinement_pos_y * 8);
            cost += svt_aom_fp_mv_err_cost(&best_mv, &mv_cost_params);
            if (cost < *best_cost) {
                *best_mvx  = mvx + (refinement_pos_x * 8);
                *best_mvy  = mvy + (refinement_pos_y * 8);
                *best_cost = cost;
            }
        }
    }
}

// Derive me_sb_addr and me_block_offset used to access ME_MV
static void derive_me_offsets(const SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx) {
    // @ this stage NSQ block(s) are inheriting SQ block(s) ME results; MV(s), pruning PA_ME results

    ctx->geom_offset_x = 0;
    ctx->geom_offset_y = 0;

    if (scs->seq_header.sb_size == BLOCK_128X128) {
        uint32_t me_sb_size         = scs->b64_size;
        uint32_t me_pic_width_in_sb = (pcs->ppcs->aligned_width + scs->b64_size - 1) / me_sb_size;
        uint32_t me_sb_x            = (ctx->blk_org_x / me_sb_size);
        uint32_t me_sb_y            = (ctx->blk_org_y / me_sb_size);
        ctx->me_sb_addr             = me_sb_x + me_sb_y * me_pic_width_in_sb;
        ctx->geom_offset_x          = (me_sb_x & 0x1) * me_sb_size;
        ctx->geom_offset_y          = (me_sb_y & 0x1) * me_sb_size;
        ctx->me_block_offset        = svt_aom_get_me_block_offset(
            ctx->blk_org_x, ctx->blk_org_y, ctx->blk_geom->bsize, pcs->ppcs->enable_me_8x8, pcs->ppcs->enable_me_16x16);
    } else {
        ctx->me_sb_addr = ctx->sb_ptr->index;

        ctx->me_block_offset = svt_aom_get_me_block_offset(
            ctx->blk_org_x, ctx->blk_org_y, ctx->blk_geom->bsize, pcs->ppcs->enable_me_8x8, pcs->ppcs->enable_me_16x16);
    }

    assert(ctx->me_block_offset != (uint32_t)(-1));
    ctx->me_cand_offset = ctx->me_block_offset * pcs->ppcs->pa_me_data->max_cand;
}

#define MAX_MD_NSQ_SARCH_MVC_CNT 6

static void md_nsq_motion_search(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                 uint32_t input_origin_index, MvReferenceFrame rf, const MeSbResults* me_results,
                                 int16_t* me_mv_x, int16_t* me_mv_y) {
    assert(rf < REF_FRAMES); // Must be unipred
    const uint8_t list_idx = get_list_idx(rf);
    const uint8_t ref_idx  = get_ref_frame_idx(rf);
    // Step 0: derive the MVC list for the NSQ search; 1 SQ MV (default MV for NSQ) and up to 4 sub-block MV(s) (e.g. if 16x8 then 2 8x8, if 32x8 then 4 8x8)
    int16_t       mvc_x_array[MAX_MD_NSQ_SARCH_MVC_CNT];
    int16_t       mvc_y_array[MAX_MD_NSQ_SARCH_MVC_CNT];
    int8_t        mvc_count = 0;
    const uint8_t max_refs  = pcs->ppcs->pa_me_data->max_refs;
    const uint8_t max_l0    = pcs->ppcs->pa_me_data->max_l0;
    bool          is_present;
    ctx->enable_psad = ctx->md_nsq_me_ctrls.enable_psad;
    // SQ MV (default MVC for NSQ)
    mvc_x_array[mvc_count] = *me_mv_x;
    mvc_y_array[mvc_count] = *me_mv_y;
    mvc_count++;
    if ((ctx->blk_geom->bwidth != 4 && ctx->blk_geom->bheight != 4) && ctx->blk_geom->sq_size >= 16) {
        uint8_t       min_size      = MIN(ctx->blk_geom->bwidth, ctx->blk_geom->bheight);
        const uint8_t number_of_pus = pcs->ppcs->enable_me_16x16
            ? pcs->ppcs->enable_me_8x8 ? pcs->ppcs->max_number_of_pus_per_sb : MAX_SB64_PU_COUNT_NO_8X8
            : MAX_SB64_PU_COUNT_WO_16X16;
        // Origin of the current block relative to its parent 64x64 block
        const Position blk_geom_offset = {.x = ctx->blk_org_x - ctx->sb_origin_x - ctx->geom_offset_x,
                                          .y = ctx->blk_org_y - ctx->sb_origin_y - ctx->geom_offset_y};
        // Derive the sub-block(s) MVs (additional MVC for NSQ)
        for (uint32_t block_index = 0; block_index < number_of_pus; block_index++) {
            if ((min_size == partition_width[block_index] || min_size == partition_height[block_index]) &&
                ((pu_search_index_map[block_index][0] >= (unsigned)blk_geom_offset.x) &&
                 (pu_search_index_map[block_index][0] < (unsigned)blk_geom_offset.x + ctx->blk_geom->bwidth)) &&
                ((pu_search_index_map[block_index][1] >= (unsigned)blk_geom_offset.y) &&
                 (pu_search_index_map[block_index][1] < (unsigned)blk_geom_offset.y + ctx->blk_geom->bheight)) &&
                svt_aom_is_me_data_present(
                    block_index, block_index * pcs->ppcs->pa_me_data->max_cand, me_results, list_idx, ref_idx)) {
                if (list_idx == 0) {
                    mvc_x_array[mvc_count] = (me_results->me_mv_array[block_index * max_refs + ref_idx].x) << 3;
                    mvc_y_array[mvc_count] = (me_results->me_mv_array[block_index * max_refs + ref_idx].y) << 3;
                } else {
                    mvc_x_array[mvc_count] = (me_results->me_mv_array[block_index * max_refs + max_l0 + ref_idx].x)
                        << 3;
                    mvc_y_array[mvc_count] = (me_results->me_mv_array[block_index * max_refs + max_l0 + ref_idx].y)
                        << 3;
                }
                is_present = 0;
                for (int16_t mvc_index = 0; mvc_index < mvc_count; mvc_index++) {
                    if (mvc_x_array[mvc_count] == mvc_x_array[mvc_index] &&
                        mvc_y_array[mvc_count] == mvc_y_array[mvc_index]) {
                        is_present = 1;
                        break;
                    }
                }
                if (!is_present) {
                    mvc_count++;
                }
            }
        }
    }
    mvc_x_array[mvc_count] = 0;
    mvc_y_array[mvc_count] = 0;

    is_present = 0;
    for (int16_t mvc_index = 0; mvc_index < mvc_count; mvc_index++) {
        if (mvc_x_array[mvc_count] == mvc_x_array[mvc_index] && mvc_y_array[mvc_count] == mvc_y_array[mvc_index]) {
            is_present = 1;
            break;
        }
    }
    if (!is_present) {
        mvc_count++;
    }
    // Search Center
    int16_t              search_center_mvx  = mvc_x_array[0];
    int16_t              search_center_mvy  = mvc_y_array[0];
    uint32_t             search_center_cost = (uint32_t)~0;
    uint8_t              hbd_md             = EB_8_BIT_MD;
    EbReferenceObject*   ref_obj            = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;
    EbPictureBufferDesc* ref_pic            = svt_aom_get_ref_pic_buffer(pcs, rf);
    // -------
    // Use scaled references if resolution of the reference is different from that of the input
    // -------
    svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, hbd_md);

    for (int16_t mvc_index = 0; mvc_index < mvc_count; mvc_index++) {
        // Round-up the search center to the closest integer
        mvc_x_array[mvc_index] = (mvc_x_array[mvc_index] + 4) & ~0x07;
        mvc_y_array[mvc_index] = (mvc_y_array[mvc_index] + 4) & ~0x07;
        md_full_pel_search(pcs,
                           ctx,
                           input_pic,
                           ref_pic,
                           input_origin_index,
                           ctx->md_nsq_me_ctrls.dist_type,
                           mvc_x_array[mvc_index],
                           mvc_y_array[mvc_index],
                           0,
                           0,
                           0,
                           0,
                           1,
                           0,
                           &search_center_mvx,
                           &search_center_mvy,
                           &search_center_cost,
                           hbd_md);
    }

    *me_mv_x                  = search_center_mvx;
    *me_mv_y                  = search_center_mvy;
    int16_t  best_search_mvx  = (int16_t)~0;
    int16_t  best_search_mvy  = (int16_t)~0;
    uint32_t best_search_cost = (uint32_t)~0;
    md_full_pel_search(pcs,
                       ctx,
                       input_pic,
                       ref_pic,
                       input_origin_index,
                       ctx->md_nsq_me_ctrls.dist_type,
                       search_center_mvx,
                       search_center_mvy,
                       -(ctx->md_nsq_me_ctrls.full_pel_search_width >> 1),
                       +(ctx->md_nsq_me_ctrls.full_pel_search_width >> 1),
                       -(ctx->md_nsq_me_ctrls.full_pel_search_height >> 1),
                       +(ctx->md_nsq_me_ctrls.full_pel_search_height >> 1),
                       4,
                       0,
                       &best_search_mvx,
                       &best_search_mvy,
                       &best_search_cost,
                       hbd_md);
    md_full_pel_search(pcs,
                       ctx,
                       input_pic,
                       ref_pic,
                       input_origin_index,
                       ctx->md_nsq_me_ctrls.dist_type,
                       best_search_mvx,
                       best_search_mvy,
                       -2,
                       +2,
                       -2,
                       +2,
                       2,
                       0,
                       &best_search_mvx,
                       &best_search_mvy,
                       &best_search_cost,
                       hbd_md);

    md_full_pel_search(pcs,
                       ctx,
                       input_pic,
                       ref_pic,
                       input_origin_index,
                       ctx->md_nsq_me_ctrls.dist_type,
                       best_search_mvx,
                       best_search_mvy,
                       -1,
                       +1,
                       -1,
                       +1,
                       1,
                       0,
                       &best_search_mvx,
                       &best_search_mvy,
                       &best_search_cost,
                       hbd_md);
    if (best_search_cost < search_center_cost) {
        *me_mv_x = best_search_mvx;
        *me_mv_y = best_search_mvy;
    }
}

/*
   clips input MV (in 1/8 precision) to stay within boundaries of a given ref pic
*/
static void clip_mv_on_pic_boundary(int32_t blk_org_x, int32_t blk_org_y, int32_t bwidth, int32_t bheight,
                                    EbPictureBufferDesc* ref_pic, int16_t* mvx, int16_t* mvy) {
    if (blk_org_x + (*mvx >> 3) + bwidth > ref_pic->max_width + ref_pic->org_x) {
        *mvx = (ref_pic->max_width - blk_org_x) << 3;
    }

    if (blk_org_y + (*mvy >> 3) + bheight > ref_pic->max_height + ref_pic->org_y) {
        *mvy = (ref_pic->max_height - blk_org_y) << 3;
    }

    if (blk_org_x + (*mvx >> 3) < -ref_pic->org_x) {
        *mvx = (-blk_org_x - bwidth) << 3;
    }

    if (blk_org_y + (*mvy >> 3) < -ref_pic->org_y) {
        *mvy = (-blk_org_y - bheight) << 3;
    }
}

/*
 * Check the size of the spatial MVs and MVPs of the given block
 *
 * Return a motion category, based on the MV size.
 */
static uint8_t check_spatial_mv_size(ModeDecisionContext* ctx, uint8_t list_idx, uint8_t ref_idx, int16_t* me_mv_x,
                                     int16_t* me_mv_y) {
    uint8_t search_area_multiplier = 0;

    // Iterate over all MVPs; if large, set high search_area_multiplier
    for (int8_t mvp_index = 0; mvp_index < ctx->mvp_count[list_idx][ref_idx]; mvp_index++) {
        if (ctx->mvp_array[list_idx][ref_idx][mvp_index].x > HIGH_SPATIAL_MV_TH ||
            ctx->mvp_array[list_idx][ref_idx][mvp_index].y > HIGH_SPATIAL_MV_TH || *me_mv_x > HIGH_SPATIAL_MV_TH ||
            *me_mv_y > HIGH_SPATIAL_MV_TH) {
            search_area_multiplier = MAX(3, search_area_multiplier);
            return search_area_multiplier; // reached MAX value already
        } else if (ctx->mvp_array[list_idx][ref_idx][mvp_index].x > MEDIUM_SPATIAL_MV_TH ||
                   ctx->mvp_array[list_idx][ref_idx][mvp_index].y > MEDIUM_SPATIAL_MV_TH ||
                   *me_mv_x > MEDIUM_SPATIAL_MV_TH || *me_mv_y > MEDIUM_SPATIAL_MV_TH) {
            search_area_multiplier = MAX(2, search_area_multiplier);
        } else if (ctx->mvp_array[list_idx][ref_idx][mvp_index].x > LOW_SPATIAL_MV_TH ||
                   ctx->mvp_array[list_idx][ref_idx][mvp_index].y > LOW_SPATIAL_MV_TH || *me_mv_x > LOW_SPATIAL_MV_TH ||
                   *me_mv_y > LOW_SPATIAL_MV_TH) {
            search_area_multiplier = MAX(1, search_area_multiplier);
        }
    }
    return search_area_multiplier;
}

/*
 * Check the size of the temporal MVs
 *
 * Return a motion category, based on the MV size.
 */
static uint8_t check_temporal_mv_size(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    uint8_t search_area_multiplier = 0;

    Av1Common*  cm             = pcs->ppcs->av1_cm;
    int32_t     mi_row         = ctx->blk_org_y >> MI_SIZE_LOG2;
    int32_t     mi_col         = ctx->blk_org_x >> MI_SIZE_LOG2;
    TPL_MV_REF* prev_frame_mvs = pcs->tpl_mvs + (mi_row >> 1) * (cm->mi_stride >> 1) + (mi_col >> 1);
    TPL_MV_REF* mv             = prev_frame_mvs;
    if (prev_frame_mvs->mfmv0.as_int != INVALID_MV) {
        if (ABS(mv->mfmv0.y) > MEDIUM_TEMPORAL_MV_TH || ABS(mv->mfmv0.x) > MEDIUM_TEMPORAL_MV_TH) {
            search_area_multiplier = MAX(2, search_area_multiplier);
        } else if (ABS(mv->mfmv0.y) > LOW_TEMPORAL_MV_TH || ABS(mv->mfmv0.x) > LOW_TEMPORAL_MV_TH) {
            search_area_multiplier = MAX(1, search_area_multiplier);
        }
    }

    return search_area_multiplier;
}

/*
 * Detect if block has high motion, and if so, perform an expanded ME search.
 */
static void md_sq_motion_search(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                uint32_t input_origin_index, MvReferenceFrame rf, int16_t* me_mv_x, int16_t* me_mv_y) {
    uint8_t hbd_md = EB_8_BIT_MD;
    assert(rf < REF_FRAMES); // Must be unipred
    const uint8_t        list_idx = get_list_idx(rf);
    const uint8_t        ref_idx  = get_ref_frame_idx(rf);
    EbReferenceObject*   ref_obj  = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;
    EbPictureBufferDesc* ref_pic  = svt_aom_get_ref_pic_buffer(pcs, rf);
    // -------
    // Use scaled references if resolution of the reference is different from that of the input
    // -------
    svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, hbd_md);

    MdSqMotionSearchCtrls* md_sq_me_ctrls = &ctx->md_sq_me_ctrls;
    uint16_t               dist           = ABS(
        (int16_t)((int64_t)pcs->picture_number - (int64_t)pcs->ppcs->ref_pic_poc_array[list_idx][ref_idx]));
    uint8_t search_area_multiplier = 0;
    ctx->enable_psad               = md_sq_me_ctrls->enable_psad;
    // Get pa_me distortion and MVs
    int16_t  pa_me_mvx  = (int16_t)~0;
    int16_t  pa_me_mvy  = (int16_t)~0;
    uint32_t pa_me_cost = (uint32_t)~0;
    md_full_pel_search(pcs,
                       ctx,
                       input_pic,
                       ref_pic,
                       input_origin_index,
                       md_sq_me_ctrls->dist_type,
                       *me_mv_x,
                       *me_mv_y,
                       0,
                       0,
                       0,
                       0,
                       1,
                       0,
                       &pa_me_mvx,
                       &pa_me_mvy,
                       &pa_me_cost,
                       hbd_md);
    // Identify potential high active block(s) and ME failure using 2 checks : (1) high ME_MV distortion, (2) active co - located block for non - intra ref(Temporal - MV(s)) or active surrounding block(s) for intra ref(Spatial - MV(s))
    if (ctx->blk_geom->sq_size <= 64) {
        uint32_t fast_lambda = ctx->hbd_md ? ctx->fast_lambda_md[EB_10_BIT_MD] : ctx->fast_lambda_md[EB_8_BIT_MD];

        // Check if pa_me distortion is above the per-pixel threshold.  Rate is set to 16.
        if (RDCOST(fast_lambda, 16, pa_me_cost) >
            RDCOST(
                fast_lambda, 16, md_sq_me_ctrls->pame_distortion_th * ctx->blk_geom->bwidth * ctx->blk_geom->bheight)) {
            ref_obj = (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;

            search_area_multiplier = !(ref_obj == NULL || ref_obj->frame_type == KEY_FRAME ||
                                       ref_obj->frame_type == INTRA_ONLY_FRAME)
                ? check_temporal_mv_size(pcs, ctx)
                : check_spatial_mv_size(ctx, list_idx, ref_idx, me_mv_x, me_mv_y);
        }
    }

    // If high motion was detected, perform an expanded ME search
    if (search_area_multiplier) {
        int16_t  best_search_mvx  = (int16_t)~0;
        int16_t  best_search_mvy  = (int16_t)~0;
        uint32_t best_search_cost = (uint32_t)~0;

        dist = svt_aom_get_scaled_picture_distance(dist);

        // Sparse-search Level_0
        if (md_sq_me_ctrls->sprs_lev0_enabled) {
            uint16_t sprs_lev0_w = (md_sq_me_ctrls->sprs_lev0_multiplier *
                                    MIN((md_sq_me_ctrls->sprs_lev0_w * search_area_multiplier * dist),
                                        md_sq_me_ctrls->max_sprs_lev0_w)) /
                100;
            uint16_t sprs_lev0_h = (md_sq_me_ctrls->sprs_lev0_multiplier *
                                    MIN((md_sq_me_ctrls->sprs_lev0_h * search_area_multiplier * dist),
                                        md_sq_me_ctrls->max_sprs_lev0_h)) /
                100;
            uint8_t sprs_lev0_step = md_sq_me_ctrls->sprs_lev0_step;

            // Derive start/end position of sparse search (must be a multiple of the step size)
            int16_t search_position_start_x = -(((sprs_lev0_w >> 1) / sprs_lev0_step) * sprs_lev0_step);
            int16_t search_position_end_x   = +(((sprs_lev0_w >> 1) / sprs_lev0_step) * sprs_lev0_step);
            int16_t search_position_start_y = -(((sprs_lev0_h >> 1) / sprs_lev0_step) * sprs_lev0_step);
            int16_t search_position_end_y   = +(((sprs_lev0_h >> 1) / sprs_lev0_step) * sprs_lev0_step);

            ctx->sprs_lev0_start_x = (*me_mv_x >> 3) + search_position_start_x;
            ctx->sprs_lev0_end_x   = (*me_mv_x >> 3) + search_position_end_x;
            ctx->sprs_lev0_start_y = (*me_mv_y >> 3) + search_position_start_y;
            ctx->sprs_lev0_end_y   = (*me_mv_y >> 3) + search_position_end_y;
            md_full_pel_search(pcs,
                               ctx,
                               input_pic,
                               ref_pic,
                               input_origin_index,
                               md_sq_me_ctrls->dist_type,
                               *me_mv_x,
                               *me_mv_y,
                               search_position_start_x,
                               search_position_end_x,
                               search_position_start_y,
                               search_position_end_y,
                               sprs_lev0_step,
                               0,
                               &best_search_mvx,
                               &best_search_mvy,
                               &best_search_cost,
                               hbd_md);
            *me_mv_x = best_search_mvx;
            *me_mv_y = best_search_mvy;
        }

        // Sparse-search Level_1
        if (md_sq_me_ctrls->sprs_lev1_enabled) {
            uint16_t sprs_lev1_w = (md_sq_me_ctrls->sprs_lev1_multiplier *
                                    MIN((md_sq_me_ctrls->sprs_lev1_w * search_area_multiplier * dist),
                                        md_sq_me_ctrls->max_sprs_lev1_w)) /
                100;
            uint16_t sprs_lev1_h = (md_sq_me_ctrls->sprs_lev1_multiplier *
                                    MIN((md_sq_me_ctrls->sprs_lev1_h * search_area_multiplier * dist),
                                        md_sq_me_ctrls->max_sprs_lev1_h)) /
                100;
            uint8_t sprs_lev1_step = md_sq_me_ctrls->sprs_lev1_step;

            // Derive start/end position of sparse search (must be a multiple of the step size)
            int16_t search_position_start_x = -(((sprs_lev1_w >> 1) / sprs_lev1_step) * sprs_lev1_step);
            int16_t search_position_end_x   = +(((sprs_lev1_w >> 1) / sprs_lev1_step) * sprs_lev1_step);
            int16_t search_position_start_y = -(((sprs_lev1_h >> 1) / sprs_lev1_step) * sprs_lev1_step);
            int16_t search_position_end_y   = +(((sprs_lev1_h >> 1) / sprs_lev1_step) * sprs_lev1_step);

            search_position_start_x = (search_position_start_x % 4 == 0) ? search_position_start_x - 2
                                                                         : search_position_start_x;
            search_position_end_x   = (search_position_end_x % 4 == 0) ? search_position_end_x + 2
                                                                       : search_position_end_x;
            search_position_start_y = (search_position_start_y % 4 == 0) ? search_position_start_y - 2
                                                                         : search_position_start_y;
            search_position_end_y   = (search_position_end_y % 4 == 0) ? search_position_end_y + 2
                                                                       : search_position_end_y;
            md_full_pel_search(
                pcs,
                ctx,
                input_pic,
                ref_pic,
                input_origin_index,
                md_sq_me_ctrls->dist_type,
                *me_mv_x,
                *me_mv_y,
                search_position_start_x,
                search_position_end_x,
                search_position_start_y,
                search_position_end_y,
                sprs_lev1_step,
                (ctx->md_sq_me_ctrls.sprs_lev0_enabled && ctx->md_sq_me_ctrls.sprs_lev0_step == 4) ? 1 : 0,
                &best_search_mvx,
                &best_search_mvy,
                &best_search_cost,
                hbd_md);
            *me_mv_x = best_search_mvx;
            *me_mv_y = best_search_mvy;
        }

        // Sparse-search Level_2
        if (md_sq_me_ctrls->sprs_lev2_enabled) {
            md_full_pel_search(pcs,
                               ctx,
                               input_pic,
                               ref_pic,
                               input_origin_index,
                               md_sq_me_ctrls->dist_type,
                               *me_mv_x,
                               *me_mv_y,
                               -(((md_sq_me_ctrls->sprs_lev2_w >> 1) / md_sq_me_ctrls->sprs_lev2_step) *
                                 md_sq_me_ctrls->sprs_lev2_step),
                               +(((md_sq_me_ctrls->sprs_lev2_w >> 1) / md_sq_me_ctrls->sprs_lev2_step) *
                                 md_sq_me_ctrls->sprs_lev2_step),
                               -(((md_sq_me_ctrls->sprs_lev2_h >> 1) / md_sq_me_ctrls->sprs_lev2_step) *
                                 md_sq_me_ctrls->sprs_lev2_step),
                               +(((md_sq_me_ctrls->sprs_lev2_h >> 1) / md_sq_me_ctrls->sprs_lev2_step) *
                                 md_sq_me_ctrls->sprs_lev2_step),
                               md_sq_me_ctrls->sprs_lev2_step,
                               0,
                               &best_search_mvx,
                               &best_search_mvy,
                               &best_search_cost,
                               hbd_md);
            *me_mv_x = best_search_mvx;
            *me_mv_y = best_search_mvy;
        }
    }
}

/*
 * Perform 1/2-Pel, 1/4-Pel, and 1/8-Pel search around the best Full-Pel position
 */
static int md_subpel_search(SUBPEL_STAGE       search_stage, //ME or PME
                            PictureControlSet* pcs, ModeDecisionContext* ctx, MdSubPelSearchCtrls md_subpel_ctrls,
                            MvReferenceFrame rf, Mv* me_mv) {
    int16_t              me_mv_x   = me_mv->x;
    int16_t              me_mv_y   = me_mv->y;
    EbPictureBufferDesc* input_pic = pcs->ppcs->enhanced_pic;
    assert(rf < REF_FRAMES); // Must be unipred
    const uint8_t list_idx = get_list_idx(rf);
    const uint8_t ref_idx  = get_ref_frame_idx(rf);
    FrameHeader*  frm_hdr  = &pcs->ppcs->frm_hdr;

    const Av1Common* const cm = pcs->ppcs->av1_cm;
    MacroBlockD*           xd = ctx->blk_ptr->av1xd;
    // ref_mv is used to calculate the cost of the motion vector
    Mv ref_mv = ctx->ref_mv;
    // High level params
    SUBPEL_MOTION_SEARCH_PARAMS  ms_params_struct;
    SUBPEL_MOTION_SEARCH_PARAMS* ms_params = &ms_params_struct;
    ms_params->search_stage                = search_stage;
    ms_params->list_idx                    = list_idx;
    ms_params->ref_idx                     = ref_idx;
    ms_params->allow_hp                    = pcs->ppcs->frm_hdr.allow_high_precision_mv;
    ms_params->forced_stop                 = md_subpel_ctrls.max_precision;
    // Maximum number of steps in logarithmic subpel search before giving up.
    ms_params->iters_per_step = md_subpel_ctrls.subpel_iters_per_step;
    // Derive mv_limits (TODO Hsan_Subpel should be derived under ctx @ eack block)
    // Set up limit values for MV components.
    // Mv beyond the range do not produce new/different prediction block.
    MvLimits mv_limits;
    int      mi_row    = xd->mi_row;
    int      mi_col    = xd->mi_col;
    int      mi_width  = mi_size_wide[ctx->blk_geom->bsize];
    int      mi_height = mi_size_high[ctx->blk_geom->bsize];
    mv_limits.row_min  = -(((mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND);
    mv_limits.col_min  = -(((mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND);
    mv_limits.row_max  = (cm->mi_rows - mi_row) * MI_SIZE + AOM_INTERP_EXTEND;
    mv_limits.col_max  = (cm->mi_cols - mi_col) * MI_SIZE + AOM_INTERP_EXTEND;
    svt_av1_set_mv_search_range(&mv_limits, &ref_mv);
    svt_av1_set_subpel_mv_search_range(&ms_params->mv_limits, (FullMvLimits*)&mv_limits, &ref_mv);
    // Mvcost params
    svt_init_mv_cost_params(&ms_params->mv_cost_params,
                            ctx,
                            &ref_mv,
                            frm_hdr->quantization_params.base_q_idx,
                            ctx->full_lambda_md[EB_8_BIT_MD],
                            0); // 10BIT not supported
    // Subpel variance params
    ms_params->var_params.vfp                = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
    ms_params->var_params.subpel_search_type = md_subpel_ctrls.subpel_search_type;
    ms_params->var_params.w                  = block_size_wide[ctx->blk_geom->bsize];
    ms_params->var_params.h                  = block_size_high[ctx->blk_geom->bsize];

    // Ref and src buffers
    MSBuffers* ms_buffers = &ms_params->var_params.ms_buffers;

    // Ref buffer
    EbReferenceObject*   ref_obj = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;
    EbPictureBufferDesc* ref_pic = svt_aom_get_ref_pic_buffer(pcs, rf);
    // -------
    // Use scaled references if resolution of the reference is different from that of the input
    // -------
    svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, EB_8_BIT_MD);

    int32_t ref_origin_index = ref_pic->org_x + ctx->blk_org_x + (ctx->blk_org_y + ref_pic->org_y) * ref_pic->stride_y;

    // Ref buffer
    struct svt_buf_2d ref_struct;
    ref_struct.buf    = ref_pic->buffer_y + ref_origin_index;
    ref_struct.width  = ref_pic->width;
    ref_struct.height = ref_pic->height;
    ref_struct.stride = ref_pic->stride_y;
    ms_buffers->ref   = &ref_struct;
    // Src buffer
    uint32_t input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
        (ctx->blk_org_x + input_pic->org_x);
    struct svt_buf_2d src_struct;
    src_struct.buf    = input_pic->buffer_y + input_origin_index;
    src_struct.width  = input_pic->width;
    src_struct.height = input_pic->height;
    src_struct.stride = input_pic->stride_y;
    ms_buffers->src   = &src_struct;
    Mv best_mv;
    // TODO: should use get_fullmv_from_mv instead of shifting
    best_mv.x          = me_mv_x >> 3;
    best_mv.y          = me_mv_y >> 3;
    Mv subpel_start_mv = get_mv_from_fullmv(&best_mv);

    int          not_used = 0;
    unsigned int pred_sse = 0; // not used
    // Assign which subpel search method to use
    fractional_mv_step_fp* subpel_search_method = md_subpel_ctrls.subpel_search_method == SUBPEL_TREE
        ? svt_av1_find_best_sub_pixel_tree
        : svt_av1_find_best_sub_pixel_tree_pruned;
    ms_params->pred_variance_th                 = md_subpel_ctrls.pred_variance_th;
    ms_params->abs_th_mult                      = md_subpel_ctrls.abs_th_mult;
    ms_params->round_dev_th                     = md_subpel_ctrls.round_dev_th;
    ms_params->skip_diag_refinement             = md_subpel_ctrls.skip_diag_refinement;
    uint8_t early_exit = (ctx->is_intra_bordered && ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled) ||
        (md_subpel_ctrls.skip_zz_mv && best_mv.as_int == 0) || (ctx->blk_geom->sq_size <= md_subpel_ctrls.min_blk_sz);
    ms_params->var_params.bias_fp = md_subpel_ctrls.bias_fp;
    int besterr                   = subpel_search_method(ctx,
                                       xd,
                                       (const struct AV1Common* const)cm,
                                       ms_params,
                                       subpel_start_mv,
                                       &best_mv,
                                       &not_used,
                                       &pred_sse,
                                       pcs->ppcs->picture_qp,
                                       ctx->blk_geom->bsize,
                                       early_exit);
    me_mv->x                      = best_mv.x;
    me_mv->y                      = best_mv.y;

    return besterr;
}

// Copy ME_MVs (generated @ PA) from input buffer (pcs-> .. ->me_results) to local
// MD buffers (ctx->sb_me_mv) - simplified for LPD1
static void read_refine_me_mvs_light_pd1(PictureControlSet* pcs, EbPictureBufferDesc* input_pic,
                                         ModeDecisionContext* ctx) {
    // init best ME cost to MAX
    ctx->md_me_dist = (uint32_t)~0;
    // Get the ME MV
    const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
    const BlockGeom*   blk_geom   = ctx->blk_geom;
    const uint8_t      max_l0     = pcs->ppcs->pa_me_data->max_l0;

    const bool subpel_enabled = ctx->md_subpel_me_ctrls.enabled;
    const bool skip_zero_mv   = ctx->md_subpel_me_ctrls.skip_zz_mv;

    const bool no_mv_stack = ctx->shut_fast_rate;

    for (int ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        const MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame       rf[2];
        av1_set_ref_frame(rf, ref_pair);

        if (rf[1] == NONE_FRAME) {
            const uint8_t list = get_list_idx(rf[0]);
            const uint8_t ref  = get_ref_frame_idx(rf[0]);

            if (svt_aom_is_me_data_present(ctx->me_block_offset, ctx->me_cand_offset, me_results, list, ref)) {
                EbPictureBufferDesc* ref_pic = svt_aom_get_ref_pic_buffer(pcs, rf[0]);
                EbReferenceObject*   ref_obj = pcs->ref_pic_ptr_array[list][ref]->object_ptr;
                // -------
                // Use scaled references if resolution of the reference is different from that of the input
                // -------
                svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, ctx->hbd_md);

                const Mv* me_mv_array_base = me_results->me_mv_array +
                    (ctx->me_block_offset * pcs->ppcs->pa_me_data->max_refs + ref);
                const Mv mv_cand = me_mv_array_base[list ? max_l0 : 0];
                Mv       me_mv   = {{mv_cand.x << 3, mv_cand.y << 3}};
                // can only skip if using dc only b/c otherwise need cost at candidate generation
                const bool skip_subpel = (ctx->is_intra_bordered &&
                                          ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled) ||
                    (skip_zero_mv && me_mv.x == 0 && me_mv.y == 0) ||
                    (ctx->blk_geom->sq_size <= ctx->md_subpel_me_ctrls.min_blk_sz);

                if (subpel_enabled && !skip_subpel) {
                    if (no_mv_stack) {
                        ctx->ref_mv.as_int = 0;
                    } else {
                        Mv      best_pred_mv[2] = {{{0}}, {{0}}};
                        uint8_t drl_index       = 0;
                        svt_aom_choose_best_av1_mv_pred(
                            ctx, ref_pair, NEWMV, me_mv, (Mv){{0}}, &drl_index, best_pred_mv);
                        ctx->ref_mv.as_int = best_pred_mv[0].as_int;
                    }
                    ctx->post_subpel_me_mv_cost[list][ref] = md_subpel_search(
                        SPEL_ME, pcs, ctx, ctx->md_subpel_me_ctrls, rf[0], &me_mv);

                    if (ctx->post_subpel_me_mv_cost[list][ref] < ctx->md_me_dist) {
                        ctx->md_me_dist = ctx->post_subpel_me_mv_cost[list][ref];
                    }
                }
                ctx->sb_me_mv[list][ref].as_int = me_mv.as_int;
                clip_mv_on_pic_boundary(ctx->blk_org_x,
                                        ctx->blk_org_y,
                                        blk_geom->bwidth,
                                        blk_geom->bheight,
                                        ref_pic,
                                        &ctx->sb_me_mv[list][ref].x,
                                        &ctx->sb_me_mv[list][ref].y);
            }
        }
    }
}

// Copy ME_MVs (generated @ PA) from input buffer (pcs-> .. ->me_results) to local
// MD buffers (ctx->sb_me_mv)
static void read_refine_me_mvs(PictureControlSet* pcs, ModeDecisionContext* ctx, const PC_TREE* const pc_tree) {
    const SequenceControlSet* scs = pcs->scs;
    derive_me_offsets(scs, pcs, ctx);
    const uint8_t        hbd_md    = EB_8_BIT_MD;
    EbPictureBufferDesc* input_pic = pcs->ppcs->enhanced_pic;
    const BlockGeom*     blk_geom  = ctx->blk_geom;

    // Update input origin
    const uint32_t input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
        (ctx->blk_org_x + input_pic->org_x);
    const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
    const uint8_t      max_l0     = pcs->ppcs->pa_me_data->max_l0;

    const bool          blk_avail_sqi      = pc_tree->tested_blk[PART_N][0];
    const bool          b_w_ne_h           = blk_geom->bwidth != blk_geom->bheight;
    const bool          md_nsq_me_enabled  = ctx->md_nsq_me_ctrls.enabled;
    const bool          md_sq_me_enabled   = ctx->md_sq_me_ctrls.enabled;
    MdSubPelSearchCtrls md_subpel_me_ctrls = ctx->md_subpel_me_ctrls;
    const bool          do_subpel          = md_subpel_me_ctrls.enabled;

    ctx->md_me_dist = (uint32_t)~0;
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        const MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame       rf[2];
        av1_set_ref_frame(rf, ref_pair);

        if (rf[1] == NONE_FRAME) {
            const uint8_t        list    = get_list_idx(rf[0]);
            const uint8_t        ref     = get_ref_frame_idx(rf[0]);
            EbReferenceObject*   ref_obj = pcs->ref_pic_ptr_array[list][ref]->object_ptr;
            EbPictureBufferDesc* ref_pic = svt_aom_get_ref_pic_buffer(pcs, rf[0]);
            // -------
            // Use scaled references if resolution of the reference is different from that of the input
            // -------
            svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, hbd_md);

            // Get the ME MV
            if (svt_aom_is_me_data_present(ctx->me_block_offset, ctx->me_cand_offset, me_results, list, ref)) {
                Mv me_mv;
                if (blk_avail_sqi &&
                    // If NSQ then use the MV of SQ as default MV center
                    b_w_ne_h &&
                    // Not applicable for BLOCK_128X64 and BLOCK_64X128 as the 2nd part of each and BLOCK_128X128 do not share the same me_results
                    blk_geom->bsize != BLOCK_64X128 && blk_geom->bsize != BLOCK_128X64) {
                    me_mv.x = (ctx->sq_sb_me_mv[list][ref].x + 4) & ~0x07;
                    me_mv.y = (ctx->sq_sb_me_mv[list][ref].y + 4) & ~0x07;
                } else if (blk_geom->bsize == BLOCK_4X4 && pc_tree->parent->tested_blk[PART_N][0]) {
                    me_mv.x = (ctx->sq_sb_me_mv[list][ref].x + 4) & ~0x07;
                    me_mv.y = (ctx->sq_sb_me_mv[list][ref].y + 4) & ~0x07;
                } else {
                    const Mv* me_mv_array_base = me_results->me_mv_array +
                        (ctx->me_block_offset * pcs->ppcs->pa_me_data->max_refs + ref);
                    const Mv mv_cand = me_mv_array_base[list ? max_l0 : 0];
                    me_mv.x          = mv_cand.x << 3;
                    me_mv.y          = mv_cand.y << 3;
                }
                clip_mv_on_pic_boundary(
                    ctx->blk_org_x, ctx->blk_org_y, blk_geom->bwidth, blk_geom->bheight, ref_pic, &me_mv.x, &me_mv.y);
                // Set ref MV
                Mv      best_pred_mv[2] = {{{0}}, {{0}}};
                uint8_t drl_index       = 0;
                svt_aom_choose_best_av1_mv_pred(ctx, ref_pair, NEWMV, me_mv, (Mv){{0}}, &drl_index, best_pred_mv);
                ctx->ref_mv.as_int = best_pred_mv[0].as_int;
                if (b_w_ne_h) {
                    if (md_nsq_me_enabled) {
                        md_nsq_motion_search(
                            pcs, ctx, input_pic, input_origin_index, rf[0], me_results, &me_mv.x, &me_mv.y);
                    }
                } else if (md_sq_me_enabled) {
                    md_sq_motion_search(pcs, ctx, input_pic, input_origin_index, rf[0], &me_mv.x, &me_mv.y);
                }
                ctx->post_subpel_me_mv_cost[list][ref] = (int32_t)~0;
                ctx->fp_me_mv[list][ref].as_int        = me_mv.as_int;

                if (do_subpel) {
                    ctx->post_subpel_me_mv_cost[list][ref] = md_subpel_search(
                        SPEL_ME, pcs, ctx, md_subpel_me_ctrls, rf[0], &me_mv);
                    if (ctx->post_subpel_me_mv_cost[list][ref] < ctx->md_me_dist) {
                        ctx->md_me_dist = ctx->post_subpel_me_mv_cost[list][ref];
                    }
                } else if (ctx->updated_enable_pme || ctx->ref_pruning_ctrls.enabled) {
                    // If full-pel cost for ME MVs will be needed for other features, ensure it is computed when subpel is off
                    int32_t ref_origin_index = ref_pic->org_x + (ctx->blk_org_x + (me_mv.x >> 3)) +
                        (ctx->blk_org_y + (me_mv.y >> 3) + ref_pic->org_y) * ref_pic->stride_y;
                    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
                    unsigned int            sse;
                    uint8_t*                pred_y = ref_pic->buffer_y + ref_origin_index;
                    uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
                    ctx->fp_me_dist[list][ref]     = fn_ptr->vf(
                        pred_y, ref_pic->stride_y, src_y, input_pic->stride_y, &sse);

                    svt_mv_cost_param mv_cost_params;
                    FrameHeader*      frm_hdr = &pcs->ppcs->frm_hdr;
                    // Variance is computed for 8bit, so use 8bit lambda
                    uint32_t rdmult = ctx->full_lambda_md[EB_8_BIT_MD];
                    svt_init_mv_cost_params(
                        &mv_cost_params, ctx, &ctx->ref_mv, frm_hdr->quantization_params.base_q_idx, rdmult, hbd_md);
                    Mv best_mv;
                    best_mv.as_int = me_mv.as_int;
                    ctx->fp_me_dist[list][ref] += svt_aom_fp_mv_err_cost(&best_mv, &mv_cost_params);
                }
                // Copy ME MV after subpel
                ctx->sub_me_mv[list][ref].as_int = me_mv.as_int;
                ctx->sb_me_mv[list][ref].as_int  = me_mv.as_int;
                clip_mv_on_pic_boundary(ctx->blk_org_x,
                                        ctx->blk_org_y,
                                        blk_geom->bwidth,
                                        blk_geom->bheight,
                                        ref_pic,
                                        &ctx->sb_me_mv[list][ref].x,
                                        &ctx->sb_me_mv[list][ref].y);
                if (ctx->shape == PART_N) {
                    ctx->sq_sb_me_mv[list][ref].as_int = ctx->sb_me_mv[list][ref].as_int;
                }
            }
        }
    }
}

/*
Loop over TPL blocks in the SB to update inter information.  Return 1 if the stats for the SB are valid; else return 0.

sb_max_rf_idx: The maximum rf_idx selected by any TPL block in the SB
*/

static bool get_sb_tpl_inter_stats(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t* sb_inter_selection,
                                   uint8_t* sb_max_list0_ref_idx, uint8_t* sb_max_list1_ref_idx) {
    PictureParentControlSet* ppcs = pcs->ppcs;

    // Check that TPL data is available and that INTRA was tested in TPL.
    // Note that not all INTRA modes may be tested in TPL.
    if (ppcs->tpl_ctrls.enable && ppcs->tpl_src_data_ready &&
        (ppcs->temporal_layer_index < ppcs->hierarchical_levels || !ppcs->tpl_ctrls.disable_intra_pred_nref)) {
        const int      aligned16_width = (ppcs->aligned_width + 15) >> 4;
        const uint32_t mb_origin_x     = ctx->sb_origin_x;
        const uint32_t mb_origin_y     = ctx->sb_origin_y;
        const int      tpl_blk_size    = ppcs->tpl_ctrls.dispenser_search_level == 0 ? 16
                    : ppcs->tpl_ctrls.dispenser_search_level == 1                    ? 32
                                                                                     : 64;
        // tpl_src_stats_buffer is created for 16x16 always, so TPL dispenser is for larger block sizes
        // the step between blocks must be adjusted
        const int tpl_blk_step = ppcs->tpl_ctrls.dispenser_search_level == 0 ? 1
            : ppcs->tpl_ctrls.dispenser_search_level == 1                    ? 2
                                                                             : 4;

        // Get actual SB width (for cases of incomplete SBs)
        SbGeom*   sb_geom = &ppcs->sb_geom[ctx->sb_index];
        const int sb_cols = MAX(1, sb_geom->width / tpl_blk_size);
        const int sb_rows = MAX(1, sb_geom->height / tpl_blk_size);

        uint8_t tot_cnt             = 0;
        uint8_t inter_cnt           = 0;
        uint8_t max_list_ref_idx[2] = {0};

        // Loop over all blocks in the SB
        for (int i = 0; i < sb_rows; i++) {
            TplSrcStats* tpl_src_stats_buffer =
                &ppcs->pa_me_data->tpl_src_stats_buffer[((mb_origin_y >> 4) + (i * tpl_blk_step)) * aligned16_width +
                                                        (mb_origin_x >> 4)];
            for (int j = 0; j < sb_cols; j++) {
                tot_cnt++;
                if (!is_intra_mode(tpl_src_stats_buffer->best_mode)) {
                    uint8_t list_index    = tpl_src_stats_buffer->best_rf_idx < 4 ? 0 : 1;
                    uint8_t ref_pic_index = tpl_src_stats_buffer->best_rf_idx >= 4
                        ? (tpl_src_stats_buffer->best_rf_idx - 4)
                        : tpl_src_stats_buffer->best_rf_idx;

                    max_list_ref_idx[list_index] = MAX(max_list_ref_idx[list_index], ref_pic_index);
                    inter_cnt++;
                }

                tpl_src_stats_buffer += tpl_blk_step;
            }
        }

        *sb_inter_selection   = (inter_cnt * 100) / tot_cnt;
        *sb_max_list0_ref_idx = max_list_ref_idx[0];
        *sb_max_list1_ref_idx = max_list_ref_idx[1];
        return 1;
    }
    return 0;
}

static void perform_md_reference_pruning(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    uint32_t early_inter_distortion_array[MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH];
    svt_memset(early_inter_distortion_array, 0xFE, sizeof(early_inter_distortion_array));
    uint32_t offset_tab[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH] = {{0}};
    svt_memset(ctx->ref_filtering_res, 0, sizeof(ctx->ref_filtering_res));
    uint32_t min_dist     = (uint32_t)~0;
    int      use_tpl_info = 0;
    uint8_t  sb_max_list0_ref_idx;
    uint8_t  sb_max_list1_ref_idx;
    uint8_t  sb_inter_selection;
    if (ctx->ref_pruning_ctrls.use_tpl_info_offset && pcs->ppcs->tpl_ctrls.enable) {
        if (get_sb_tpl_inter_stats(pcs, ctx, &sb_inter_selection, &sb_max_list0_ref_idx, &sb_max_list1_ref_idx)) {
            use_tpl_info = 1;
        }
    }
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        if (rf[1] == NONE_FRAME) {
            uint32_t best_mvp_distortion = (int32_t)~0;
            uint8_t  list_idx            = get_list_idx(rf[0]);
            uint8_t  ref_idx             = get_ref_frame_idx(rf[0]);
            // Only use TPL info if all references are tested
            if (use_tpl_info) {
                if ((list_idx == 0 && ref_idx > sb_max_list0_ref_idx) ||
                    (list_idx == 1 && ref_idx > sb_max_list1_ref_idx)) {
                    offset_tab[list_idx][ref_idx] += ctx->ref_pruning_ctrls.use_tpl_info_offset;
                }
            }
            // Step 1: derive the best MVP in term of distortion
            best_mvp_distortion = ctx->best_fp_mvp_dist[list_idx][ref_idx];

            // Evaluate the PA_ME MVs (if available)
            uint32_t           pa_me_distortion = (uint32_t)~0; //any non zero value
            const MeSbResults* me_results       = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
            if (svt_aom_is_me_data_present(ctx->me_block_offset, ctx->me_cand_offset, me_results, list_idx, ref_idx)) {
                //uint32_t pa_me_distortion = ctx->post_subpel_me_mv_cost[][];
                pa_me_distortion = ctx->fp_me_dist[list_idx][ref_idx];
            }
            // early_inter_distortion_array
            early_inter_distortion_array[list_idx * REF_LIST_MAX_DEPTH + ref_idx] = MIN(pa_me_distortion,
                                                                                        best_mvp_distortion);
            if (early_inter_distortion_array[list_idx * REF_LIST_MAX_DEPTH + ref_idx] < min_dist) {
                min_dist = early_inter_distortion_array[list_idx * REF_LIST_MAX_DEPTH + ref_idx];
            }
        }
    }
    uint32_t th = (ctx->ref_pruning_ctrls.check_closest_multiplier * (ctx->blk_geom->bheight * ctx->blk_geom->bwidth) *
                   pcs->ppcs->picture_qp) /
        24;
    if (ctx->ref_pruning_ctrls.check_closest_multiplier && early_inter_distortion_array[0] < th &&
        early_inter_distortion_array[REF_LIST_MAX_DEPTH] < th) {
        for (unsigned li = 0; li < MAX_NUM_OF_REF_PIC_LIST; li++) {
            for (unsigned ri = 0; ri < REF_LIST_MAX_DEPTH; ri++) {
                for (unsigned gi = 0; gi < TOT_INTER_GROUP; gi++) {
                    if (ri == 0 || ctx->ref_pruning_ctrls.max_dev_to_best[gi] == (uint32_t)~0) {
                        ctx->ref_filtering_res[gi][li][ri].do_ref = 1;
                    }
                }
            }
        }
    } else {
        // Sort early_inter_distortion_array
        unsigned num_of_cand_to_sort = MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH;
        uint32_t dev_to_the_best[MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH] = {0};
        for (unsigned i = 0; i < num_of_cand_to_sort - 1; ++i) {
            dev_to_the_best[i] = (((int64_t)MAX(early_inter_distortion_array[i], 1) - MAX(min_dist, 1)) * 100) /
                MAX(min_dist, 1);
        }
        for (unsigned li = 0; li < MAX_NUM_OF_REF_PIC_LIST; li++) {
            for (unsigned ri = 0; ri < REF_LIST_MAX_DEPTH; ri++) {
                for (unsigned gi = 0; gi < TOT_INTER_GROUP; gi++) {
                    uint32_t offset     = offset_tab[li][ri];
                    uint32_t pruning_th = (offset == (uint32_t)~0 || ctx->ref_pruning_ctrls.max_dev_to_best[gi] == 0)
                        ? 0
                        : (ctx->ref_pruning_ctrls.max_dev_to_best[gi] == (uint32_t)~0)
                        ? (uint32_t)~0
                        : MAX(0, ((int64_t)ctx->ref_pruning_ctrls.max_dev_to_best[gi] - (int64_t)offset));

                    if (dev_to_the_best[li * REF_LIST_MAX_DEPTH + ri] < pruning_th) {
                        ctx->ref_filtering_res[gi][li][ri].do_ref = 1;
                    }
                }
            }
        }
    }
}

/*
 * Read/store all nearest/near MVs for a block for single ref case, and save the best distortion for each ref.
 */
static void build_single_ref_mvp_array(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    const uint8_t        hbd_md         = EB_8_BIT_MD;
    EbPictureBufferDesc* input_pic      = pcs->ppcs->enhanced_pic;
    const MacroBlockD*   xd             = ctx->blk_ptr->av1xd;
    const BlockGeom*     blk_geom       = ctx->blk_geom;
    const bool           shut_fast_rate = ctx->shut_fast_rate;
    for (int ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        const MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];

        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);
        // Single ref
        if (rf[1] == NONE_FRAME) {
            const MvReferenceFrame frame_type = rf[0];
            const uint8_t          list       = get_list_idx(rf[0]);
            const uint8_t          ref        = get_ref_frame_idx(rf[0]);
            EbReferenceObject*     ref_obj    = pcs->ref_pic_ptr_array[list][ref]->object_ptr;
            EbPictureBufferDesc*   ref_pic    = svt_aom_get_ref_pic_buffer(pcs, rf[0]);
            // -------
            // Use scaled references if resolution of the reference is different from that of the input
            // -------
            svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, hbd_md);
            if (shut_fast_rate) {
                ctx->mvp_array[list][ref][0].as_int = 0;
                ctx->mvp_count[list][ref]           = 1;
                continue;
            }
            int8_t mvp_count = 0;

            //NEAREST
            const Mv as_mv                         = ctx->ref_mv_stack[frame_type][0].this_mv;
            ctx->mvp_array[list][ref][mvp_count].x = (as_mv.x + 4) & ~0x07;
            ctx->mvp_array[list][ref][mvp_count].y = (as_mv.y + 4) & ~0x07;
            clip_mv_on_pic_boundary(ctx->blk_org_x,
                                    ctx->blk_org_y,
                                    blk_geom->bwidth,
                                    blk_geom->bheight,
                                    ref_pic,
                                    &ctx->mvp_array[list][ref][mvp_count].x,
                                    &ctx->mvp_array[list][ref][mvp_count].y);
            mvp_count++;

            //NEAR
            const uint8_t max_drl_index = svt_aom_get_max_drl_index(xd->ref_mv_count[frame_type], NEARMV);

            for (int drli = 0; drli < max_drl_index; drli++) {
                Mv nearmv = ctx->ref_mv_stack[frame_type][1 + drli].this_mv;
                // cppcheck doesn't work well when the rhs and lhs have the same union
                // store temp values before reassigning
                const int16_t x = (nearmv.x + 4) & ~0x07;
                const int16_t y = (nearmv.y + 4) & ~0x07;
                nearmv.x        = x;
                nearmv.y        = y;
                clip_mv_on_pic_boundary(
                    ctx->blk_org_x, ctx->blk_org_y, blk_geom->bwidth, blk_geom->bheight, ref_pic, &nearmv.x, &nearmv.y);

                bool inj_near_mv = true;
                for (int idx = 0; idx < mvp_count; idx++) {
                    if (nearmv.as_int == ctx->mvp_array[list][ref][idx].as_int) {
                        inj_near_mv = false;
                        break;
                    }
                }
                if (inj_near_mv) {
                    ctx->mvp_array[list][ref][mvp_count].as_int = nearmv.as_int;
                    mvp_count++;
                }
            }
            ctx->mvp_count[list][ref] = mvp_count;

            // Compute the best distortion and save the best MVP index (results used in subpel, PME, and ref pruning)
            uint32_t best_mvp_cost      = (int32_t)~0;
            uint32_t input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
                (ctx->blk_org_x + input_pic->org_x);

            for (int8_t mvp_index = 0; mvp_index < ctx->mvp_count[list][ref]; mvp_index++) {
                int32_t ref_origin_index = ref_pic->org_x +
                    (ctx->blk_org_x + (ctx->mvp_array[list][ref][mvp_index].x >> 3)) +
                    (ctx->blk_org_y + (ctx->mvp_array[list][ref][mvp_index].y >> 3) + ref_pic->org_y) *
                        ref_pic->stride_y;
                const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
                unsigned int            sse;
                uint8_t*                pred_y = ref_pic->buffer_y + ref_origin_index;
                uint8_t*                src_y  = input_pic->buffer_y + input_origin_index;
                uint32_t mvp_cost = fn_ptr->vf(pred_y, ref_pic->stride_y, src_y, input_pic->stride_y, &sse);

                if (mvp_cost < best_mvp_cost) {
                    ctx->best_fp_mvp_idx[list][ref]  = mvp_index;
                    ctx->best_fp_mvp_dist[list][ref] = mvp_cost;
                    best_mvp_cost                    = mvp_cost;
                }
            }
        }
    }
}

bool svt_aom_is_valid_unipred_ref(ModeDecisionContext* ctx, uint8_t inter_cand_group, uint8_t list_idx,
                                  uint8_t ref_idx);

/*
* Performs an ME search around MVP(s)
* For a given (block, list_idx, ref_idx), if PME search is skipped then set ME_MV=ME_MV to preserve PME candidate = (ME_MV, PME_MV)
*/
static void pme_search(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic) {
    uint8_t hbd_md = EB_8_BIT_MD;
    // Modulate the PME-full-pel search-area using QP
    // The PME-full-pel search will be skipped  if width or/and height ends-up equal to 0 (only subpel-search will take place)
    uint8_t full_pel_search_width  = ctx->md_pme_ctrls.full_pel_search_width;
    uint8_t full_pel_search_height = ctx->md_pme_ctrls.full_pel_search_height;
    if (ctx->md_pme_ctrls.sa_q_weight) {
        uint32_t q_weight, q_weight_denom;
        svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.pme_qp_based_th_scaling,
                                                &q_weight,
                                                &q_weight_denom,
                                                pcs->scs->static_config.qp);
        full_pel_search_width  = MAX(3, DIVIDE_AND_ROUND(full_pel_search_width * q_weight, q_weight_denom));
        full_pel_search_height = MAX(3, DIVIDE_AND_ROUND(full_pel_search_height * q_weight, q_weight_denom));
    }
    input_pic = pcs->ppcs->enhanced_pic;

    uint32_t input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
        (ctx->blk_org_x + input_pic->org_x);

    ctx->enable_psad = 0;
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        // Reset search variable(s)
        uint32_t best_mvp_cost           = (int32_t)~0;
        uint32_t pme_mv_cost             = (int32_t)~0;
        uint32_t me_mv_cost              = (int32_t)~0;
        uint32_t post_subpel_pme_mv_cost = (int32_t)~0;

        if (rf[1] == NONE_FRAME) {
            uint8_t list_idx                     = get_list_idx(rf[0]);
            uint8_t ref_idx                      = get_ref_frame_idx(rf[0]);
            ctx->valid_pme_mv[list_idx][ref_idx] = 0;

            if (pcs->ppcs->scs->mrp_ctrls.pme_ref0_only && pcs->temporal_layer_index > 0) {
                if (ref_idx > 0) {
                    continue;
                }
            }

            EbReferenceObject*   ref_obj = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;
            EbPictureBufferDesc* ref_pic = svt_aom_get_ref_pic_buffer(pcs, rf[0]);
            // -------
            // Use scaled references if resolution of the reference is different from that of the input
            // -------
            svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, hbd_md);
            if (!svt_aom_is_valid_unipred_ref(ctx, PRED_ME_GROUP, list_idx, ref_idx)) {
                continue;
            }
            // Get the ME MV
            const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];

            uint8_t me_data_present = svt_aom_is_me_data_present(
                ctx->me_block_offset, ctx->me_cand_offset, me_results, list_idx, ref_idx);

            if (me_data_present) {
                // Early MVP vs. ME_MV check; do not perform PME search for blocks that have a valid ME_MV unless the ME_MV has a different direction than all MVP(s) and the ME_MV mag is higher than MV_TH (not around(0,0))
                if (ctx->md_pme_ctrls.early_check_mv_th_multiplier != MIN_SIGNED_VALUE) {
                    uint8_t is_me_mv_different_than_mvp = 0;
                    for (int8_t mvp_index = 0; mvp_index < ctx->mvp_count[list_idx][ref_idx]; mvp_index++) {
                        Mv mvp = {.as_int = ctx->mvp_array[list_idx][ref_idx][mvp_index].as_int};

                        int mv_th = (((pcs->ppcs->enhanced_pic->width * pcs->ppcs->enhanced_pic->height) >> 17) *
                                     ctx->md_pme_ctrls.early_check_mv_th_multiplier) /
                            10;

                        // Check x direction
                        if (ABS(mvp.x) > mv_th) {
                            if (ctx->fp_me_mv[list_idx][ref_idx].x * mvp.x < 0) {
                                is_me_mv_different_than_mvp = 1;
                                break;
                            }
                        }

                        // Check y direction
                        if (ABS(mvp.y) > mv_th) {
                            if (ctx->fp_me_mv[list_idx][ref_idx].y * mvp.y < 0) {
                                is_me_mv_different_than_mvp = 1;
                                break;
                            }
                        }
                    }

                    if (is_me_mv_different_than_mvp == 0) {
                        ctx->valid_pme_mv[list_idx][ref_idx]       = 1;
                        ctx->pme_res[list_idx][ref_idx].dist       = ctx->post_subpel_me_mv_cost[list_idx][ref_idx];
                        ctx->best_pme_mv[list_idx][ref_idx].as_int = ctx->sub_me_mv[list_idx][ref_idx].as_int;
                        continue;
                    }
                }

                me_mv_cost = ctx->fp_me_dist[list_idx][ref_idx];
            }

            // Step 1: derive the best MVP in term of distortion
            Mv best_mvp = {.as_int = ctx->mvp_array[list_idx][ref_idx][ctx->best_fp_mvp_idx[list_idx][ref_idx]].as_int};
            best_mvp_cost = ctx->best_fp_mvp_dist[list_idx][ref_idx];
            if (me_data_present) {
                int64_t pme_to_me_cost_dev = (((int64_t)MAX(best_mvp_cost, 1) - (int64_t)MAX(me_mv_cost, 1)) * 100) /
                    (int64_t)MAX(me_mv_cost, 1);

                if ((ABS(ctx->fp_me_mv[list_idx][ref_idx].x - best_mvp.x) <= ctx->md_pme_ctrls.pre_fp_pme_to_me_mv_th &&
                     ABS(ctx->fp_me_mv[list_idx][ref_idx].y - best_mvp.y) <=
                         ctx->md_pme_ctrls.pre_fp_pme_to_me_mv_th) ||
                    pme_to_me_cost_dev >= ctx->md_pme_ctrls.pre_fp_pme_to_me_cost_th) {
                    ctx->valid_pme_mv[list_idx][ref_idx]       = 1;
                    ctx->pme_res[list_idx][ref_idx].dist       = ctx->post_subpel_me_mv_cost[list_idx][ref_idx];
                    ctx->best_pme_mv[list_idx][ref_idx].as_int = ctx->sub_me_mv[list_idx][ref_idx].as_int;
                    continue;
                }
            }
            Mv best_search_mv;
            // Set ref MV
            Mv      best_pred_mv[2] = {{{0}}, {{0}}};
            uint8_t drl_index       = 0;
            svt_aom_choose_best_av1_mv_pred(ctx, ref_pair, NEWMV, best_mvp, (Mv){{0}}, &drl_index, best_pred_mv);
            ctx->ref_mv.as_int = best_pred_mv[0].as_int;
            ctx->enable_psad   = ctx->md_pme_ctrls.enable_psad;
            md_full_pel_search(pcs,
                               ctx,
                               input_pic,
                               ref_pic,
                               input_origin_index,
                               ctx->md_pme_ctrls.dist_type,
                               best_mvp.x,
                               best_mvp.y,
                               -(full_pel_search_width >> 1),
                               +(full_pel_search_width >> 1),
                               -(full_pel_search_height >> 1),
                               +(full_pel_search_height >> 1),
                               1,
                               0,
                               &best_search_mv.x,
                               &best_search_mv.y,
                               &pme_mv_cost,
                               hbd_md);
            if (me_data_present) {
                int64_t pme_to_me_cost_dev = (((int64_t)MAX(pme_mv_cost, 1) - (int64_t)MAX(me_mv_cost, 1)) * 100) /
                    (int64_t)MAX(me_mv_cost, 1);

                if ((ABS(ctx->fp_me_mv[list_idx][ref_idx].x - best_search_mv.x) <=
                         ctx->md_pme_ctrls.post_fp_pme_to_me_mv_th &&
                     ABS(ctx->fp_me_mv[list_idx][ref_idx].y - best_search_mv.y) <=
                         ctx->md_pme_ctrls.post_fp_pme_to_me_mv_th) ||
                    pme_to_me_cost_dev >= ctx->md_pme_ctrls.post_fp_pme_to_me_cost_th) {
                    ctx->valid_pme_mv[list_idx][ref_idx]       = 1;
                    ctx->pme_res[list_idx][ref_idx].dist       = ctx->post_subpel_me_mv_cost[list_idx][ref_idx];
                    ctx->best_pme_mv[list_idx][ref_idx].as_int = ctx->sub_me_mv[list_idx][ref_idx].as_int;
                    continue;
                }
            }
            if (ctx->md_subpel_pme_ctrls.enabled) {
                post_subpel_pme_mv_cost = (uint32_t)md_subpel_search(
                    SPEL_PME, pcs, ctx, ctx->md_subpel_pme_ctrls, rf[0], &best_search_mv);
            }
            ctx->best_pme_mv[list_idx][ref_idx].as_int = best_search_mv.as_int;
            ctx->valid_pme_mv[list_idx][ref_idx]       = 1;
            ctx->pme_res[list_idx][ref_idx].dist       = post_subpel_pme_mv_cost;
        }
    }
    for (uint8_t list_idx = 0; list_idx < MAX_NUM_OF_REF_PIC_LIST; list_idx++) {
        for (uint8_t ref_idx = 0; ref_idx < REF_LIST_MAX_DEPTH; ref_idx++) {
            if (ctx->pme_res[list_idx][ref_idx].dist < ctx->md_pme_dist) {
                ctx->md_pme_dist = ctx->pme_res[list_idx][ref_idx].dist;
            }
        }
    }
}

static void av1_cost_calc_cfl(PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx,
                              uint32_t component_mask, EbPictureBufferDesc* input_pic,
                              uint32_t input_cb_origin_in_index, uint32_t blk_chroma_origin_index,
                              uint64_t full_dist[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* coeff_bits, bool check_dc) {
    ModeDecisionCandidate* cand                                            = cand_bf->cand;
    uint64_t               cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
    uint64_t               cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
    uint32_t               chroma_width                                    = ctx->blk_geom->bwidth_uv;
    uint32_t               chroma_height                                   = ctx->blk_geom->bheight_uv;
    // FullLoop and TU search
    uint16_t cb_qindex = ctx->qp_index;

    full_dist[DIST_SSD][DIST_CALC_RESIDUAL]    = 0;
    full_dist[DIST_SSD][DIST_CALC_PREDICTION]  = 0;
    full_dist[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
    full_dist[DIST_SSIM][DIST_CALC_PREDICTION] = 0;
    *coeff_bits                                = 0;

    // Loop over alphas and find the best
    if (component_mask == COMPONENT_CHROMA_CB || component_mask == COMPONENT_CHROMA ||
        component_mask == COMPONENT_ALL) {
        cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        cb_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;
        cr_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;

        cb_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
        cr_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
        cb_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = 0;
        cr_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = 0;
        uint64_t cb_coeff_bits                              = 0;
        uint64_t cr_coeff_bits                              = 0;
        int32_t  alpha_q3                                   = (check_dc) ? 0
                                                                         : cfl_idx_to_alpha(cand->block_mi.cfl_alpha_idx,
                                                         cand->block_mi.cfl_alpha_signs,
                                                         CFL_PRED_U); // once for U, once for V
        assert(chroma_width * CFL_BUF_LINE + chroma_height <= CFL_BUF_SQUARE);

        if (!ctx->hbd_md) {
            svt_cfl_predict_lbd(ctx->pred_buf_q3,
                                &(cand_bf->pred->buffer_cb[blk_chroma_origin_index]),
                                cand_bf->pred->stride_cb,
                                &(ctx->scratch_prediction_ptr->buffer_cb[blk_chroma_origin_index]),
                                ctx->scratch_prediction_ptr->stride_cb,
                                alpha_q3,
                                8,
                                chroma_width,
                                chroma_height);
        } else {
            svt_cfl_predict_hbd(ctx->pred_buf_q3,
                                ((uint16_t*)cand_bf->pred->buffer_cb) + blk_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                ((uint16_t*)ctx->scratch_prediction_ptr->buffer_cb) + blk_chroma_origin_index,
                                ctx->scratch_prediction_ptr->stride_cb,
                                alpha_q3,
                                10,
                                chroma_width,
                                chroma_height);
        }

        // Cb Residual
        svt_aom_residual_kernel(input_pic->buffer_cb,
                                input_cb_origin_in_index,
                                input_pic->stride_cb,
                                ctx->scratch_prediction_ptr->buffer_cb,
                                blk_chroma_origin_index,
                                ctx->scratch_prediction_ptr->stride_cb,
                                (int16_t*)cand_bf->residual->buffer_cb,
                                blk_chroma_origin_index,
                                cand_bf->residual->stride_cb,
                                ctx->hbd_md,
                                chroma_width,
                                chroma_height);
        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA_CB,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             &cb_coeff_bits,
                             &cr_coeff_bits,
                             0);

        full_dist[DIST_SSD][DIST_CALC_RESIDUAL] += cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL];
        full_dist[DIST_SSD][DIST_CALC_PREDICTION] += cb_full_distortion[DIST_SSD][DIST_CALC_PREDICTION];

        full_dist[DIST_SSIM][DIST_CALC_RESIDUAL] += cb_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL];
        full_dist[DIST_SSIM][DIST_CALC_PREDICTION] += cb_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION];
        *coeff_bits += cb_coeff_bits;
    }
    if (component_mask == COMPONENT_CHROMA_CR || component_mask == COMPONENT_CHROMA ||
        component_mask == COMPONENT_ALL) {
        cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        cb_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;
        cr_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;

        cb_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
        cr_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
        cb_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = 0;
        cr_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = 0;

        uint64_t cb_coeff_bits = 0;
        uint64_t cr_coeff_bits = 0;
        int32_t  alpha_q3      = check_dc ? 0
                                          : cfl_idx_to_alpha(cand->block_mi.cfl_alpha_idx,
                                                       cand->block_mi.cfl_alpha_signs,
                                                       CFL_PRED_V); // once for U, once for V
        assert(chroma_width * CFL_BUF_LINE + chroma_height <= CFL_BUF_SQUARE);

        if (!ctx->hbd_md) {
            svt_cfl_predict_lbd(ctx->pred_buf_q3,
                                &(cand_bf->pred->buffer_cr[blk_chroma_origin_index]),
                                cand_bf->pred->stride_cr,
                                &(ctx->scratch_prediction_ptr->buffer_cr[blk_chroma_origin_index]),
                                ctx->scratch_prediction_ptr->stride_cr,
                                alpha_q3,
                                8,
                                chroma_width,
                                chroma_height);
        } else {
            svt_cfl_predict_hbd(ctx->pred_buf_q3,
                                ((uint16_t*)cand_bf->pred->buffer_cr) + blk_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                ((uint16_t*)ctx->scratch_prediction_ptr->buffer_cr) + blk_chroma_origin_index,
                                ctx->scratch_prediction_ptr->stride_cr,
                                alpha_q3,
                                10,
                                chroma_width,
                                chroma_height);
        }

        // Cr Residual
        svt_aom_residual_kernel(input_pic->buffer_cr,
                                input_cb_origin_in_index,
                                input_pic->stride_cr,
                                ctx->scratch_prediction_ptr->buffer_cr,
                                blk_chroma_origin_index,
                                ctx->scratch_prediction_ptr->stride_cr,
                                (int16_t*)cand_bf->residual->buffer_cr,
                                blk_chroma_origin_index,
                                cand_bf->residual->stride_cr,
                                ctx->hbd_md,
                                chroma_width,
                                chroma_height);
        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA_CR,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             &cb_coeff_bits,
                             &cr_coeff_bits,
                             0);
        full_dist[DIST_SSD][DIST_CALC_RESIDUAL] += cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL];
        full_dist[DIST_SSD][DIST_CALC_PREDICTION] += cr_full_distortion[DIST_SSD][DIST_CALC_PREDICTION];

        full_dist[DIST_SSIM][DIST_CALC_RESIDUAL] += cr_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL];
        full_dist[DIST_SSIM][DIST_CALC_PREDICTION] += cr_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION];
        *coeff_bits += cr_coeff_bits;
    }
}

#define PLANE_SIGN_TO_JOINT_SIGN(plane, a, b) (plane == CFL_PRED_U ? a * CFL_SIGNS + b - 1 : b * CFL_SIGNS + a - 1)

/************************************************************************************************
* md_cfl_rd_pick_alpha
* Pick the best alpha for cfl mode
************************************************************************************************/
static uint64_t md_cfl_rd_pick_alpha(PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf,
                                     ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                     uint32_t input_cb_origin_in_index, uint32_t blk_chroma_origin_index,
                                     uint8_t* cfl_alpha_idx, uint8_t* cfl_alpha_signs) {
    uint64_t best_rd = MAX_MODE_COST;
    uint64_t full_dist[DIST_TOTAL][DIST_CALC_TOTAL];
    uint64_t coeff_bits;

    uint32_t      full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    const int64_t mode_rd     = RDCOST(
        full_lambda,
        (uint64_t)ctx->md_rate_est_ctx->intra_uv_mode_fac_bits[CFL_ALLOWED][cand_bf->cand->block_mi.mode][UV_CFL_PRED],
        0);
    uint64_t best_rd_uv[CFL_JOINT_SIGNS][CFL_PRED_PLANES];
    uint8_t  best_c[CFL_JOINT_SIGNS][CFL_PRED_PLANES];

    for (uint8_t plane = 0; plane < CFL_PRED_PLANES; plane++) {
        coeff_bits                               = 0;
        full_dist[DIST_SSD][DIST_CALC_RESIDUAL]  = 0;
        full_dist[DIST_SSIM][DIST_CALC_RESIDUAL] = 0;
        for (uint8_t joint_sign = 0; joint_sign < CFL_JOINT_SIGNS; joint_sign++) {
            best_rd_uv[joint_sign][plane] = MAX_MODE_COST;
            best_c[joint_sign][plane]     = 0;
        }

        // Collect RD stats for an alpha value of zero in this plane.
        // Skip CFL_SIGN_ZERO as (0, 0) is invalid.
        // The two remaining signs are CFL_SIGN_NEG and CFL_SIGN_POS
        // Collect RD stats for CFL_SIGN_NEG
        const uint8_t joint_sign_neg            = PLANE_SIGN_TO_JOINT_SIGN(plane, CFL_SIGN_ZERO, CFL_SIGN_NEG);
        cand_bf->cand->block_mi.cfl_alpha_idx   = 0;
        cand_bf->cand->block_mi.cfl_alpha_signs = joint_sign_neg;
        // Only caculate cfl cost for joint_sign_neg
        av1_cost_calc_cfl(pcs,
                          cand_bf,
                          ctx,
                          (plane == 0) ? COMPONENT_CHROMA_CB : COMPONENT_CHROMA_CR,
                          input_pic,
                          input_cb_origin_in_index,
                          blk_chroma_origin_index,
                          full_dist,
                          &coeff_bits,
                          0);
        if (coeff_bits != INT64_MAX) {
            // Collect RD stats for CFL_SIGN_NEG
            const int32_t alpha_rate_neg      = ctx->md_rate_est_ctx->cfl_alpha_fac_bits[joint_sign_neg][plane][0];
            best_rd_uv[joint_sign_neg][plane] = RDCOST(
                full_lambda, coeff_bits + alpha_rate_neg, full_dist[DIST_SSD][DIST_CALC_RESIDUAL]);

            // Collect RD stats for CFL_SIGN_POS
            const uint8_t joint_sign_pos      = PLANE_SIGN_TO_JOINT_SIGN(plane, CFL_SIGN_ZERO, CFL_SIGN_POS);
            const int32_t alpha_rate_pos      = ctx->md_rate_est_ctx->cfl_alpha_fac_bits[joint_sign_pos][plane][0];
            best_rd_uv[joint_sign_pos][plane] = RDCOST(
                full_lambda, coeff_bits + alpha_rate_pos, full_dist[DIST_SSD][DIST_CALC_RESIDUAL]);
        }
    }

    uint8_t best_joint_sign       = 0;
    bool    best_joint_sign_found = false;

    for (uint8_t plane = 0; plane < CFL_PRED_PLANES; plane++) {
        for (uint8_t pn_sign = CFL_SIGN_NEG; pn_sign < CFL_SIGNS; pn_sign++) {
            uint8_t progress = 0;
            for (uint8_t c = 0; c < CFL_ALPHABET_SIZE; c++) {
                uint8_t flag = 0;
                if (c > ctx->cfl_ctrls.itr_th && progress < c) {
                    break;
                }
                coeff_bits                               = 0;
                full_dist[DIST_SSD][DIST_CALC_RESIDUAL]  = 0;
                full_dist[DIST_SSIM][DIST_CALC_RESIDUAL] = 0;
                for (uint8_t i = 0; i < CFL_SIGNS; i++) {
                    const uint8_t joint_sign = PLANE_SIGN_TO_JOINT_SIGN(plane, pn_sign, i);
                    if (i == 0) {
                        cand_bf->cand->block_mi.cfl_alpha_idx   = (c << CFL_ALPHABET_SIZE_LOG2) + c;
                        cand_bf->cand->block_mi.cfl_alpha_signs = joint_sign;

                        av1_cost_calc_cfl(pcs,
                                          cand_bf,
                                          ctx,
                                          (plane == 0) ? COMPONENT_CHROMA_CB : COMPONENT_CHROMA_CR,
                                          input_pic,
                                          input_cb_origin_in_index,
                                          blk_chroma_origin_index,
                                          full_dist,
                                          &coeff_bits,
                                          0);

                        if (coeff_bits == INT64_MAX) {
                            break;
                        }
                    }

                    const int32_t alpha_rate = ctx->md_rate_est_ctx->cfl_alpha_fac_bits[joint_sign][plane][c];
                    uint64_t      this_rd    = RDCOST(
                        full_lambda, coeff_bits + alpha_rate, full_dist[DIST_SSD][DIST_CALC_RESIDUAL]);
                    if (this_rd >= best_rd_uv[joint_sign][plane]) {
                        continue;
                    }
                    best_rd_uv[joint_sign][plane] = this_rd;
                    best_c[joint_sign][plane]     = c;
                    flag                          = ctx->cfl_ctrls.itr_th;
                    if (best_rd_uv[joint_sign][!plane] == MAX_MODE_COST) {
                        continue;
                    }
                    this_rd += mode_rd + best_rd_uv[joint_sign][!plane];
                    if (this_rd >= best_rd) {
                        continue;
                    }
                    best_rd               = this_rd;
                    best_joint_sign       = joint_sign;
                    best_joint_sign_found = true;
                }
                progress += flag;
            }
        }
    }

    if (best_rd != MAX_MODE_COST) {
        uint8_t ind = 0;
        if (best_joint_sign_found) {
            const uint8_t u = best_c[best_joint_sign][CFL_PRED_U];
            const uint8_t v = best_c[best_joint_sign][CFL_PRED_V];
            ind             = (u << CFL_ALPHABET_SIZE_LOG2) + v;
        }
        *cfl_alpha_idx   = ind;
        *cfl_alpha_signs = best_joint_sign;
    }
    return best_rd;
}

/* Compute the AC components of the luma prediction that are used to generate CFL predictions. */
static void compute_cfl_ac_components(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                      ModeDecisionCandidateBuffer* cand_bf) {
    const BlockGeom* const blk_geom = ctx->blk_geom;

    // 1: recon the Luma
    av1_perform_inverse_transform_recon_luma(pcs, ctx, cand_bf);
    // 2: Form the pred_buf_q3
    // Store recon with block origin offset b/c we may access in other blocks (for 4xN/Nx4 cases where chroma
    // is not allowed for each block)
    const Position blk_org         = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
    const uint32_t rec_luma_offset = (ROUND_UV(blk_org.y) * cand_bf->recon->stride_y) + ROUND_UV(blk_org.x);
    const uint32_t chroma_width    = blk_geom->bwidth_uv;
    const uint32_t chroma_height   = blk_geom->bheight_uv;

    // Down sample Luma
    if (!ctx->hbd_md) {
        svt_cfl_luma_subsampling_420_lbd(
            &(ctx->cfl_temp_luma_recon[rec_luma_offset]),
            cand_bf->recon->stride_y,
            ctx->pred_buf_q3,
            blk_geom->bwidth_uv == blk_geom->bwidth ? (blk_geom->bwidth_uv << 1) : blk_geom->bwidth,
            blk_geom->bheight_uv == blk_geom->bheight ? (blk_geom->bheight_uv << 1) : blk_geom->bheight);
    } else {
        svt_cfl_luma_subsampling_420_hbd(
            ctx->cfl_temp_luma_recon16bit + rec_luma_offset,
            cand_bf->recon->stride_y,
            ctx->pred_buf_q3,
            blk_geom->bwidth_uv == blk_geom->bwidth ? (blk_geom->bwidth_uv << 1) : blk_geom->bwidth,
            blk_geom->bheight_uv == blk_geom->bheight ? (blk_geom->bheight_uv << 1) : blk_geom->bheight);
    }

    const int32_t round_offset = (chroma_width * chroma_height) >> 1;
    svt_subtract_average(ctx->pred_buf_q3,
                         chroma_width,
                         chroma_height,
                         round_offset,
                         svt_log2f(chroma_width) + svt_log2f(chroma_height));
}

/************************************************************************************************
Test CFL:
1: Recon the Luma and form the pred_buf_q3
2: Loop over alphas and find the best CFL params
3: Compare CFL cost to the best non-CFL chroma mode and select best
************************************************************************************************/
static void cfl_prediction(PictureControlSet* pcs, ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx,
                           EbPictureBufferDesc* input_pic, uint32_t input_cb_origin_in_index,
                           uint32_t blk_chroma_origin_index) {
    // If independent chroma data available, just compute CFL and compare to the best chroma
    // OW compute the reference non-CFL cost and select the best of CFL vs. non-CFL
    uint64_t               non_cfl_cost        = MAX_MODE_COST;
    const UvPredictionMode non_cfl_uv_mode     = cand_bf->cand->block_mi.uv_mode == UV_CFL_PRED
            ? UV_DC_PRED
            : cand_bf->cand->block_mi.uv_mode;
    const int8_t           non_cfl_angle_delta = cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_UV];
    const TxType           non_cfl_tx_type     = cand_bf->cand->transform_type_uv;
    if (!ctx->ind_uv_avail) {
        uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
        cand_bf->cand->block_mi.cfl_alpha_idx   = 0;
        cand_bf->cand->block_mi.cfl_alpha_signs = 0;
        const uint64_t fast_rate                = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 0);

        //Cb Residual
        svt_aom_residual_kernel(input_pic->buffer_cb,
                                input_cb_origin_in_index,
                                input_pic->stride_cb,
                                cand_bf->pred->buffer_cb,
                                blk_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                (int16_t*)cand_bf->residual->buffer_cb,
                                blk_chroma_origin_index,
                                cand_bf->residual->stride_cb,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        //Cr Residual
        svt_aom_residual_kernel(input_pic->buffer_cr,
                                input_cb_origin_in_index,
                                input_pic->stride_cr,
                                cand_bf->pred->buffer_cr,
                                blk_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                (int16_t*)cand_bf->residual->buffer_cr,
                                blk_chroma_origin_index,
                                cand_bf->residual->stride_cr,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
        uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
        uint64_t cb_coeff_bits                                   = 0;
        uint64_t cr_coeff_bits                                   = 0;
        uint16_t cb_qindex                                       = ctx->qp_index;
        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             &cb_coeff_bits,
                             &cr_coeff_bits,
                             0);

        non_cfl_cost = RDCOST(
            full_lambda,
            cb_coeff_bits + cr_coeff_bits + fast_rate,
            cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] + cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]);
    }

    // Set CFL settings
    cand_bf->cand->block_mi.uv_mode                    = UV_CFL_PRED;
    cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_UV] = 0;
    cand_bf->cand->transform_type_uv                   = DCT_DCT;

    // Need DC prediction for CFL
    if (non_cfl_uv_mode != UV_DC_PRED) {
        ctx->uv_intra_comp_only = true;
        assert(ctx->mds_do_chroma);
        product_prediction_fun_table[is_inter_mode(cand_bf->cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
    }

    // Compute AC component of CFL prediction
    compute_cfl_ac_components(pcs, ctx, cand_bf);
    // Loop over alphas and find the best CFL params
    uint8_t  cfl_alpha_idx = 0, cfl_alpha_signs = 0;
    uint64_t cfl_rd = md_cfl_rd_pick_alpha(pcs,
                                           cand_bf,
                                           ctx,
                                           input_pic,
                                           input_cb_origin_in_index,
                                           blk_chroma_origin_index,
                                           &cfl_alpha_idx,
                                           &cfl_alpha_signs);

    // If independent chroma results are available, forward CFL to be compared to the best chroma mode
    if (ctx->ind_uv_avail || (cfl_rd != MAX_MODE_COST && cfl_rd < non_cfl_cost)) {
        cand_bf->cand->block_mi.uv_mode         = UV_CFL_PRED;
        cand_bf->cand->block_mi.cfl_alpha_idx   = cfl_alpha_idx;
        cand_bf->cand->block_mi.cfl_alpha_signs = cfl_alpha_signs;
    } else {
        cand_bf->cand->block_mi.uv_mode                    = non_cfl_uv_mode;
        cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_UV] = non_cfl_angle_delta;
        cand_bf->cand->transform_type_uv                   = non_cfl_tx_type;
        cand_bf->cand->block_mi.cfl_alpha_idx              = 0;
        cand_bf->cand->block_mi.cfl_alpha_signs            = 0;
    }

    if (cand_bf->cand->block_mi.uv_mode == UV_CFL_PRED) {
        // Recalculate the prediction and the residual for full TX path
        int32_t alpha_q3_cb = cfl_idx_to_alpha(
            cand_bf->cand->block_mi.cfl_alpha_idx, cand_bf->cand->block_mi.cfl_alpha_signs, CFL_PRED_U);
        int32_t alpha_q3_cr = cfl_idx_to_alpha(
            cand_bf->cand->block_mi.cfl_alpha_idx, cand_bf->cand->block_mi.cfl_alpha_signs, CFL_PRED_V);
        const uint32_t chroma_width  = ctx->blk_geom->bwidth_uv;
        const uint32_t chroma_height = ctx->blk_geom->bheight_uv;
        assert(chroma_height * CFL_BUF_LINE + chroma_width <= CFL_BUF_SQUARE);

        if (!ctx->hbd_md) {
            svt_cfl_predict_lbd(ctx->pred_buf_q3,
                                &(cand_bf->pred->buffer_cb[blk_chroma_origin_index]),
                                cand_bf->pred->stride_cb,
                                &(cand_bf->pred->buffer_cb[blk_chroma_origin_index]),
                                cand_bf->pred->stride_cb,
                                alpha_q3_cb,
                                8,
                                chroma_width,
                                chroma_height);

            svt_cfl_predict_lbd(ctx->pred_buf_q3,
                                &(cand_bf->pred->buffer_cr[blk_chroma_origin_index]),
                                cand_bf->pred->stride_cr,
                                &(cand_bf->pred->buffer_cr[blk_chroma_origin_index]),
                                cand_bf->pred->stride_cr,
                                alpha_q3_cr,
                                8,
                                chroma_width,
                                chroma_height);
        } else {
            svt_cfl_predict_hbd(ctx->pred_buf_q3,
                                ((uint16_t*)cand_bf->pred->buffer_cb) + blk_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                ((uint16_t*)cand_bf->pred->buffer_cb) + blk_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                alpha_q3_cb,
                                10,
                                chroma_width,
                                chroma_height);

            svt_cfl_predict_hbd(ctx->pred_buf_q3,
                                ((uint16_t*)cand_bf->pred->buffer_cr) + blk_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                ((uint16_t*)cand_bf->pred->buffer_cr) + blk_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                alpha_q3_cr,
                                10,
                                chroma_width,
                                chroma_height);
        }
    } else {
        // CFL computed DC pred, so if not using DC, need to redo the prediction
        if (non_cfl_uv_mode != UV_DC_PRED) {
            ctx->uv_intra_comp_only = true;
            assert(ctx->mds_do_chroma);
            product_prediction_fun_table[is_inter_mode(cand_bf->cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
        }
    }
}

int svt_aom_allow_palette(int allow_screen_content_tools, BlockSize bsize);

static void check_best_indepedant_cfl(PictureControlSet* pcs, EbPictureBufferDesc* input_pic, ModeDecisionContext* ctx,
                                      uint32_t input_cb_origin_in_index, uint32_t blk_chroma_origin_index,
                                      ModeDecisionCandidateBuffer* cand_bf, uint8_t cb_qindex,
                                      uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                                      uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* cb_coeff_bits,
                                      uint64_t* cr_coeff_bits) {
    uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    assert(IMPLIES(cand_bf->cand->block_mi.filter_intra_mode != FILTER_INTRA_MODES,
                   cand_bf->cand->block_mi.mode == DC_PRED));
    FrameHeader* frm_hdr     = &pcs->ppcs->frm_hdr;
    uint64_t     chroma_rate = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 1);
    int          coeff_rate  = (int)(*cb_coeff_bits + *cr_coeff_bits);
    int          distortion  = (int)(cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] +
                           cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]);
    int          rate        = (int)(coeff_rate + chroma_rate);
    uint64_t     cfl_uv_cost = RDCOST(full_lambda, rate, distortion);
    // The independent chroma cost assumes luma palette is off. If luma palette is on, the rate for the chroma mode
    // may be different than what is stored, so the rate should be updated.
    int64_t ind_palette_cost_diff = 0;
    if (ctx->ind_uv_avail && ctx->best_uv_mode[cand_bf->cand->block_mi.mode] == UV_DC_PRED &&
        svt_aom_allow_palette(pcs->ppcs->frm_hdr.allow_screen_content_tools, ctx->blk_geom->bsize) &&
        is_chroma_reference(
            ctx->blk_org_y >> MI_SIZE_LOG2, ctx->blk_org_x >> MI_SIZE_LOG2, ctx->blk_geom->bsize, 1, 1)) {
        const int use_palette_y = cand_bf->cand->palette_info && (cand_bf->cand->palette_size[0] > 0);
        if (use_palette_y) {
            const int use_palette_uv = cand_bf->cand->palette_info && (cand_bf->cand->palette_size[1] > 0);
            ind_palette_cost_diff    = (int64_t)RDCOST(
                                        full_lambda,
                                        ctx->md_rate_est_ctx->palette_uv_mode_fac_bits[use_palette_y][use_palette_uv],
                                        0) -
                (int64_t)RDCOST(full_lambda, ctx->md_rate_est_ctx->palette_uv_mode_fac_bits[0][use_palette_uv], 0);
        }
    }
    // cfl vs. best independent
    if (ctx->ind_uv_avail &&
        ((uint64_t)(ctx->best_uv_cost[cand_bf->cand->block_mi.mode] + ind_palette_cost_diff) < cfl_uv_cost)) {
        // Update the current candidate
        cand_bf->cand->block_mi.uv_mode                    = ctx->best_uv_mode[cand_bf->cand->block_mi.mode];
        cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_UV] = ctx->best_uv_angle[cand_bf->cand->block_mi.mode];
        // Re-calculate chroma rate because the rate depends on luma palette, which was not known when the
        // fast chroma rate was computed in the independent chroma search
        cand_bf->fast_chroma_rate        = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 1);
        const TxSize tx_size_uv          = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
        cand_bf->cand->transform_type_uv = svt_aom_get_intra_uv_tx_type(
            ctx->best_uv_mode[cand_bf->cand->block_mi.mode], tx_size_uv, frm_hdr->reduced_tx_set);
        ctx->uv_intra_comp_only = true;
        memset(cand_bf->eob.u, 0, sizeof(cand_bf->eob.u));
        memset(cand_bf->eob.v, 0, sizeof(cand_bf->eob.v));
        cand_bf->u_has_coeff                               = 0;
        cand_bf->v_has_coeff                               = 0;
        cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        cb_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;
        cr_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;

        cb_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
        cr_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = 0;
        cb_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = 0;
        cr_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = 0;

        *cb_coeff_bits = 0;
        *cr_coeff_bits = 0;

        assert(ctx->mds_do_chroma);
        product_prediction_fun_table[is_inter_mode(cand_bf->cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
        // Cb Residual
        svt_aom_residual_kernel(input_pic->buffer_cb,
                                input_cb_origin_in_index,
                                input_pic->stride_cb,
                                cand_bf->pred->buffer_cb,
                                blk_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                (int16_t*)cand_bf->residual->buffer_cb,
                                blk_chroma_origin_index,
                                cand_bf->residual->stride_cb,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        // Cr Residual
        svt_aom_residual_kernel(input_pic->buffer_cr,
                                input_cb_origin_in_index,
                                input_pic->stride_cr,
                                cand_bf->pred->buffer_cr,
                                blk_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                (int16_t*)cand_bf->residual->buffer_cr,
                                blk_chroma_origin_index,
                                cand_bf->residual->stride_cr,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);
        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             cb_coeff_bits,
                             cr_coeff_bits,
                             1);
    } else {
        cand_bf->fast_chroma_rate = chroma_rate;
    }
}

static void av1_intra_luma_prediction(ModeDecisionContext* ctx, PictureControlSet* pcs,
                                      ModeDecisionCandidateBuffer* cand_bf) {
    const uint8_t is_inter = 0; // set to 0 b/c this is an intra path

    const uint16_t txb_origin_x = ctx->blk_org_x +
        tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x;
    const uint16_t txb_origin_y = ctx->blk_org_y +
        tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y;
    const TxSize   tx_size      = tx_depth_to_tx_size[ctx->tx_depth][ctx->blk_geom->bsize];
    const int      tx_width     = tx_size_wide[tx_size];
    const int      tx_height    = tx_size_high[tx_size];
    const uint32_t sb_size_luma = pcs->ppcs->scs->sb_size;

    uint8_t top_neigh_array[(64 * 2 + 1) << 1];
    uint8_t left_neigh_array[(64 * 2 + 1) << 1];

    const bool           is_16bit    = !!ctx->hbd_md;
    const PredictionMode mode        = cand_bf->cand->block_mi.mode;
    const IntraSize      intra_size  = cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_Y] == 0 ? svt_aom_intra_unit[mode]
                                                                                              : (IntraSize){2, 2};
    NeighborArrayUnit*   recon_neigh = is_16bit ? ctx->tx_search_luma_recon_na_16bit : ctx->tx_search_luma_recon_na;
    if (txb_origin_y != 0) {
        svt_memcpy(top_neigh_array + ((uint64_t)1 << is_16bit),
                   recon_neigh->top_array + (txb_origin_x << is_16bit),
                   (tx_width * intra_size.top) << is_16bit);
    }
    if (txb_origin_x != 0) {
        uint16_t multipler = (txb_origin_y % sb_size_luma + tx_height * intra_size.left) > sb_size_luma
            ? 1
            : intra_size.left;
        svt_memcpy(left_neigh_array + ((uint64_t)1 << is_16bit),
                   recon_neigh->left_array + (txb_origin_y << is_16bit),
                   (tx_height * multipler) << is_16bit);
    }
    if (txb_origin_y != 0 && txb_origin_x != 0) {
        if (is_16bit) {
            uint16_t* top_hbd  = (uint16_t*)top_neigh_array;
            uint16_t* left_hbd = (uint16_t*)left_neigh_array;
            top_hbd[0] = left_hbd[0] = ((uint16_t*)(recon_neigh->top_left_array) + recon_neigh->max_pic_h +
                                        txb_origin_x - txb_origin_y)[0];

        } else {
            top_neigh_array[0] = left_neigh_array[0] =
                recon_neigh->top_left_array[recon_neigh->max_pic_h + txb_origin_x - txb_origin_y];
        }
    }

    svt_av1_predict_intra_block(ctx->blk_ptr->av1xd,
                                ctx->blk_geom->bsize,
                                tx_size,
                                mode,
                                cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_Y],
                                cand_bf->cand->palette_info ? (cand_bf->cand->palette_size[0] > 0) : 0,
                                cand_bf->cand->palette_info,
                                cand_bf->cand->block_mi.filter_intra_mode,
                                top_neigh_array + ((uint64_t)1 << is_16bit),
                                left_neigh_array + ((uint64_t)1 << is_16bit),
                                cand_bf->pred,
                                (tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x) >> 2,
                                (tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y) >> 2,
                                AOM_PLANE_Y,
                                ctx->shape,
                                tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x,
                                tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y,
                                &pcs->scs->seq_header,
                                ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT);
}

static void tx_search_update_recon_sample_neighbor_array(NeighborArrayUnit*   lumaReconSampleNeighborArray,
                                                         EbPictureBufferDesc* recon_buffer, uint32_t txb_origin_x,
                                                         uint32_t txb_origin_y, uint32_t input_origin_x,
                                                         uint32_t input_origin_y, uint32_t width, uint32_t height,
                                                         bool hbd) {
    if (hbd) {
        svt_aom_neighbor_array_unit16bit_sample_write(lumaReconSampleNeighborArray,
                                                      (uint16_t*)recon_buffer->buffer_y,
                                                      recon_buffer->stride_y,
                                                      recon_buffer->org_x + txb_origin_x,
                                                      recon_buffer->org_y + txb_origin_y,
                                                      input_origin_x,
                                                      input_origin_y,
                                                      width,
                                                      height,
                                                      NEIGHBOR_ARRAY_UNIT_FULL_MASK);
    } else {
        svt_aom_neighbor_array_unit_sample_write(lumaReconSampleNeighborArray,
                                                 recon_buffer->buffer_y,
                                                 recon_buffer->stride_y,
                                                 recon_buffer->org_x + txb_origin_x,
                                                 recon_buffer->org_y + txb_origin_y,
                                                 input_origin_x,
                                                 input_origin_y,
                                                 width,
                                                 height,
                                                 NEIGHBOR_ARRAY_UNIT_FULL_MASK);
    }

    return;
}

static uint8_t get_end_tx_depth(BlockSize bsize) {
    uint8_t tx_depth = 0;
    if (bsize == BLOCK_64X64 || bsize == BLOCK_32X32 || bsize == BLOCK_16X16 || bsize == BLOCK_64X32 ||
        bsize == BLOCK_32X64 || bsize == BLOCK_16X32 || bsize == BLOCK_32X16 || bsize == BLOCK_16X8 ||
        bsize == BLOCK_8X16 || bsize == BLOCK_64X16 || bsize == BLOCK_16X64 || bsize == BLOCK_32X8 ||
        bsize == BLOCK_8X32 || bsize == BLOCK_16X4 || bsize == BLOCK_4X16) {
        tx_depth = 2;
    } else if (bsize == BLOCK_8X8) {
        tx_depth = 1;
    }
    // tx_depth=0 if BLOCK_8X4, BLOCK_4X8, BLOCK_4X4, BLOCK_128X128, BLOCK_128X64, BLOCK_64X128
    return tx_depth;
}

static void tx_initialize_neighbor_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx, bool is_inter) {
    uint16_t tile_idx = ctx->tile_index;
    // Set recon neighbor array to be used @ intra compensation
    if (!is_inter) {
        if (ctx->hbd_md) {
            ctx->tx_search_luma_recon_na_16bit = ctx->tx_depth == 2
                ? pcs->md_tx_depth_2_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx]
                : ctx->tx_depth == 1 ? pcs->md_tx_depth_1_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx]
                                     : pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        } else {
            ctx->tx_search_luma_recon_na = ctx->tx_depth == 2
                ? pcs->md_tx_depth_2_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx]
                : ctx->tx_depth == 1 ? pcs->md_tx_depth_1_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx]
                                     : pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        }
    }
    // Set luma dc sign level coeff
    ctx->full_loop_luma_dc_sign_level_coeff_na = (ctx->tx_depth)
        ? pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx]
        : pcs->md_y_dcs_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
}

void tx_update_neighbor_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                               bool is_inter) {
    uint16_t tile_idx = ctx->tile_index;
    if (ctx->tx_depth) {
        const TxSize tx_size   = tx_depth_to_tx_size[ctx->tx_depth][ctx->blk_geom->bsize];
        const int    tx_width  = tx_size_wide[tx_size];
        const int    tx_height = tx_size_high[tx_size];
        if (!is_inter) {
            tx_search_update_recon_sample_neighbor_array(
                ctx->hbd_md ? ctx->tx_search_luma_recon_na_16bit : ctx->tx_search_luma_recon_na,
                cand_bf->recon,
                tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x,
                tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y,
                ctx->blk_org_x + tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x,
                ctx->blk_org_y + tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y,
                tx_width,
                tx_height,
                ctx->hbd_md);
        }
        int8_t dc_sign_level_coeff = cand_bf->quant_dc.y[ctx->txb_itr];
        svt_aom_neighbor_array_unit_mode_write(
            pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
            (uint8_t*)&dc_sign_level_coeff,
            ctx->blk_org_x + tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x,
            ctx->blk_org_y + tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y,
            tx_width,
            tx_height,
            NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    }
}

static void tx_reset_neighbor_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx, bool is_inter,
                                     uint8_t tx_depth) {
    int      sb_size  = pcs->ppcs->scs->super_block_size;
    uint16_t tile_idx = ctx->tile_index;
    if (tx_depth) {
        if (!is_inter) {
            if (ctx->hbd_md) {
                if (tx_depth == 2) {
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_2_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth,
                                           ctx->blk_geom->bheight,
                                           NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK);

                    const int org_y = ctx->blk_org_y - ctx->sb_origin_y;
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_2_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth * 2,
                                           MIN(ctx->blk_geom->bheight * 2, sb_size - org_y),
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
                } else {
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_1_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth,
                                           ctx->blk_geom->bheight,
                                           NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK);

                    const int org_y = ctx->blk_org_y - ctx->sb_origin_y;
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_1_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth * 2,
                                           MIN(ctx->blk_geom->bheight * 2, sb_size - org_y),
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
                }
            } else {
                if (tx_depth == 2) {
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_2_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth,
                                           ctx->blk_geom->bheight,
                                           NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK);
                    const int org_y = ctx->blk_org_y - ctx->sb_origin_y;
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_2_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth * 2,
                                           MIN(ctx->blk_geom->bheight * 2, sb_size - org_y),
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
                } else {
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_1_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth,
                                           ctx->blk_geom->bheight,
                                           NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK);
                    const int org_y = ctx->blk_org_y - ctx->sb_origin_y;
                    svt_aom_copy_neigh_arr(pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           pcs->md_tx_depth_1_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->blk_geom->bwidth * 2,
                                           MIN(ctx->blk_geom->bheight * 2, sb_size - org_y),
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
                }
            }
        }
        svt_aom_copy_neigh_arr(pcs->md_y_dcs_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                               pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx],
                               ctx->blk_org_x,
                               ctx->blk_org_y,
                               ctx->blk_geom->bwidth,
                               ctx->blk_geom->bheight,
                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    }
}

static void copy_txt_data(ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx, uint32_t txb_origin_index,
                          TxType best_tx_type) {
    uint8_t      tx_depth      = ctx->tx_depth;
    uint32_t     txb_1d_offset = ctx->txb_1d_offset;
    const TxSize tx_size       = tx_depth_to_tx_size[tx_depth][ctx->blk_geom->bsize];
    const int    tx_width      = tx_size_wide[tx_size];
    const int    tx_height     = tx_size_high[tx_size];
    // copy recon_coeff_ptr
    memcpy(((int32_t*)cand_bf->rec_coeff->buffer_y) + txb_1d_offset,
           ((int32_t*)ctx->recon_coeff_ptr[best_tx_type]->buffer_y) + txb_1d_offset,
           (tx_width * tx_height * sizeof(uint32_t)));
    // copy quant_coeff_ptr
    memcpy(((int32_t*)cand_bf->quant->buffer_y) + txb_1d_offset,
           ((int32_t*)ctx->quant_coeff_ptr[best_tx_type]->buffer_y) + txb_1d_offset,
           (tx_width * tx_height * sizeof(uint32_t)));
    // copy recon_ptr
    EbPictureBufferDesc* recon_ptr = cand_bf->recon;
    if (ctx->hbd_md) {
        for (int j = 0; j < tx_height; ++j) {
            memcpy(((uint16_t*)recon_ptr->buffer_y) + txb_origin_index + j * recon_ptr->stride_y,
                   ((uint16_t*)ctx->recon_ptr[best_tx_type]->buffer_y) + txb_origin_index + j * recon_ptr->stride_y,
                   tx_width * sizeof(uint16_t));
        }
    } else {
        for (int j = 0; j < tx_height; ++j) {
            memcpy(recon_ptr->buffer_y + txb_origin_index + j * recon_ptr->stride_y,
                   ctx->recon_ptr[best_tx_type]->buffer_y + txb_origin_index + j * recon_ptr->stride_y,
                   tx_width);
        }
    }
}

static uint8_t get_tx_type_group(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf, bool only_dct_dct) {
    int tx_group = 1;
    if (!only_dct_dct) {
        const TxSize tx_size   = tx_depth_to_tx_size[ctx->tx_depth][ctx->blk_geom->bsize];
        const int    tx_width  = tx_size_wide[tx_size];
        const int    tx_height = tx_size_high[tx_size];
        if (is_intra_mode(cand_bf->cand->block_mi.mode)) {
            tx_group = (tx_width < 16 || tx_height < 16) ? ctx->txt_ctrls.txt_group_intra_lt_16x16
                                                         : ctx->txt_ctrls.txt_group_intra_gt_eq_16x16;
        } else {
            tx_group = (tx_width < 16 || tx_height < 16) ? ctx->txt_ctrls.txt_group_inter_lt_16x16
                                                         : ctx->txt_ctrls.txt_group_inter_gt_eq_16x16;
        }
    }
    if (ctx->tx_depth == 1) {
        tx_group = MAX(tx_group - ctx->txs_ctrls.depth1_txt_group_offset, 1);
    } else if (ctx->tx_depth == 2) {
        tx_group = MAX(tx_group - ctx->txs_ctrls.depth2_txt_group_offset, 1);
    }
    return tx_group;
}

/*
 **************
*/
static void perform_tx_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                 uint32_t qindex, uint64_t* y_coeff_bits, uint64_t* y_full_distortion) {
    ctx->three_quad_energy = 0;

    TxSize       tx_size           = tx_depth_to_tx_size[0][ctx->blk_geom->bsize];
    uint32_t     txbwidth          = tx_size_wide[tx_size];
    uint32_t     txbheight         = (tx_size_high[tx_size] >> ctx->mds_subres_step);
    const double effective_ac_bias = get_effective_ac_bias(
        pcs->scs->static_config.ac_bias, pcs->slice_type == I_SLICE, pcs->temporal_layer_index);

    if (ctx->mds_subres_step == 2) {
        if (tx_size == TX_64X64) {
            tx_size = TX_64X16;
        } else if (tx_size == TX_32X32) {
            tx_size = TX_32X8;
        } else if (tx_size == TX_16X16) {
            tx_size = TX_16X4;
        } else {
            assert(0);
        }
    } else if (ctx->mds_subres_step == 1) {
        if (tx_size == TX_64X64) {
            tx_size = TX_64X32;
        } else if (tx_size == TX_32X32) {
            tx_size = TX_32X16;
        } else if (tx_size == TX_16X16) {
            tx_size = TX_16X8;
        } else if (tx_size == TX_8X8) {
            tx_size = TX_8X4;
        } else {
            assert(0);
        }
    }
    assert(tx_size < TX_SIZES_ALL);
    const int32_t  tx_type          = DCT_DCT;
    const uint32_t txb_origin_index = 0;

    int32_t* const transf_coeff = &(((int32_t*)ctx->tx_coeffs->buffer_y)[0]);
    int32_t* const recon_coeff  = &(((int32_t*)cand_bf->rec_coeff->buffer_y)[0]);

    EB_TRANS_COEFF_SHAPE pf_shape = ctx->pf_ctrls.pf_shape;

    // Y: T Q i_q
    svt_aom_estimate_transform(pcs,
                               ctx,
                               &(((int16_t*)cand_bf->residual->buffer_y)[txb_origin_index]),
                               cand_bf->residual->stride_y,
                               transf_coeff,
                               NOT_USED_VALUE,
                               tx_size,
                               &ctx->three_quad_energy,
                               EB_EIGHT_BIT,
                               tx_type,
                               PLANE_TYPE_Y,
                               pf_shape);

    svt_aom_quantize_inv_quantize_light(pcs,
                                        transf_coeff,
                                        &(((int32_t*)cand_bf->quant->buffer_y)[0]),
                                        recon_coeff,
                                        MIN(255, qindex + ctx->rate_est_ctrls.lpd0_qp_offset),
                                        tx_size,
                                        &cand_bf->eob.y[0],
                                        EB_EIGHT_BIT,
                                        DCT_DCT);

    // LUMA DISTORTION
    uint32_t bwidth, bheight;
    if (pf_shape) {
        bwidth  = MAX((txbwidth >> pf_shape), 4);
        bheight = (txbheight >> pf_shape);
    } else {
        bwidth  = txbwidth < 64 ? txbwidth : 32;
        bheight = txbheight < 64 ? txbheight : 32;
    }
    svt_aom_picture_full_distortion32_bits_single(transf_coeff,
                                                  recon_coeff,
                                                  txbwidth < 64 ? txbwidth : 32,
                                                  bwidth, // bwidth
                                                  bheight, // bheight
                                                  y_full_distortion,
                                                  cand_bf->eob.y[0]);
    y_full_distortion[DIST_CALC_RESIDUAL] += ctx->three_quad_energy;
    const int32_t shift                   = (MAX_TX_SCALE - av1_get_tx_scale_tab[tx_size]) * 2;
    y_full_distortion[DIST_CALC_RESIDUAL] = RIGHT_SIGNED_SHIFT(y_full_distortion[DIST_CALC_RESIDUAL], shift)
        << ctx->mds_subres_step;
    //LUMA-ONLY

    const uint32_t th = ((bwidth * bheight) >> 5);
    if (ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) {
        uint8_t input_resolution_factor[INPUT_SIZE_COUNT] = {0, 1, 2, 3, 4, 4, 4};
        *y_coeff_bits = 5000 + (input_resolution_factor[pcs->ppcs->input_resolution] * 1600) +
            (cand_bf->eob.y[0] * 100);
    } else if (ctx->rate_est_ctrls.coeff_rate_est_lvl >= 2 && (cand_bf->eob.y[0] < th)) {
        *y_coeff_bits = 6000 + cand_bf->eob.y[0] * 500;
    } else {
        svt_aom_txb_estimate_coeff_bits_light_pd0(
            ctx, cand_bf, ctx->txb_1d_offset, cand_bf->quant, cand_bf->eob.y[0], y_coeff_bits, tx_size);
    }

    if (effective_ac_bias) {
        *y_coeff_bits = svt_psy_adjust_rate_light(recon_coeff, *y_coeff_bits, bwidth, bheight, effective_ac_bias);
    }

    // Needed for generating recon
    cand_bf->y_has_coeff = (cand_bf->eob.y[0] > 0);
}

// Return true if DCT_DCT is the only TX type to search
static INLINE bool search_dct_dct_only(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                       ModeDecisionCandidateBuffer* cand_bf, uint8_t tx_depth, uint8_t is_inter) {
    if (!ctx->mds_do_txt) {
        return 1;
    }

    // If previous MD stages have 0 coeffs, use DCT_DCT only
    if (ctx->md_stage == MD_STAGE_3 && ctx->use_tx_shortcuts_mds3) {
        return 1;
    } else if (ctx->tx_shortcut_ctrls.bypass_tx_th && ctx->md_stage == MD_STAGE_3 && ctx->perform_mds1 &&
               !cand_bf->block_has_coeff &&
               ((cand_bf->luma_fast_dist * ctx->tx_shortcut_ctrls.bypass_tx_th) <
                (uint32_t)(ctx->blk_geom->bheight * ctx->blk_geom->bwidth * ctx->qp_index))) {
        return 1;
    }

    // Turn OFF TXT search for disallowed cases
    const TxSize tx_size   = tx_depth_to_tx_size[tx_depth][ctx->blk_geom->bsize];
    const int    tx_width  = tx_size_wide[tx_size];
    const int    tx_height = tx_size_high[tx_size];

    // get_ext_tx_set() == 0 should correspond to a set with only DCT_DCT and there is no need to send the tx_type
    if (tx_height > 32 || tx_width > 32 ||
        get_ext_tx_types(tx_size, is_inter, pcs->ppcs->frm_hdr.reduced_tx_set) == 1 ||
        get_ext_tx_set(tx_size, is_inter, pcs->ppcs->frm_hdr.reduced_tx_set) == 0) {
        return 1;
    }
    return 0;
}

static int32_t av1_txt_rate_est(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf, bool is_inter,
                                TxSize tx_size, TxType tx_type, bool reduced_tx_set_used) {
    if (get_ext_tx_types(tx_size, is_inter, reduced_tx_set_used) > 1) {
        const TxSize square_tx_size = txsize_sqr_map[tx_size];
        assert(square_tx_size < EXT_TX_SIZES);

        const int32_t ext_tx_set = get_ext_tx_set(tx_size, is_inter, reduced_tx_set_used);
        if (ext_tx_set == 0) {
            return 0;
        }

        if (is_inter) {
            return ctx->md_rate_est_ctx->inter_tx_type_fac_bits[ext_tx_set][square_tx_size][tx_type];
        } else {
            const PredictionMode intra_dir = cand_bf->cand->block_mi.filter_intra_mode != FILTER_INTRA_MODES
                ? fimode_to_intradir[cand_bf->cand->block_mi.filter_intra_mode]
                : cand_bf->cand->block_mi.mode;
            assert(intra_dir < INTRA_MODES);

            return ctx->md_rate_est_ctx->intra_tx_type_fac_bits[ext_tx_set][square_tx_size][intra_dir][tx_type];
        }
    }
    return 0;
}

static INLINE double derive_ssim_threshold_factor_for_tx_type_search(SequenceControlSet* scs) {
    return scs->input_resolution >= INPUT_SIZE_1080p_RANGE ? 1.06 : 1.05;
}

static void tx_type_search(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                           uint32_t qindex, uint8_t tx_search_skip_flag, uint64_t* y_coeff_bits,
                           uint64_t y_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL]) {
    EbPictureBufferDesc* input_pic = ctx->hbd_md ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
    int32_t              seg_qp    = pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled
                        ? pcs->ppcs->frm_hdr.segmentation_params.feature_data[ctx->blk_ptr->segment_id][SEG_LVL_ALT_Q]
                        : 0;

    uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    // Store original tx size and height b/c those may be modified if using subsampled residual
    const TxSize   tx_size_original   = tx_depth_to_tx_size[ctx->tx_depth][ctx->blk_geom->bsize];
    const uint32_t txbheight_original = tx_size_high[tx_size_original];
    TxSize         tx_size            = tx_depth_to_tx_size[ctx->tx_depth][ctx->blk_geom->bsize];
    const uint32_t txbwidth           = tx_size_wide[tx_size];
    const uint32_t txbheight          = (tx_size_high[tx_size] >> ctx->mds_subres_step);
    const bool is_inter = (is_inter_mode(cand_bf->cand->block_mi.mode) || cand_bf->cand->block_mi.use_intrabc) ? true
                                                                                                               : false;
    // Do not turn ON TXT search beyond this point
    const uint8_t only_dct_dct = search_dct_dct_only(pcs, ctx, cand_bf, ctx->tx_depth, is_inter) || tx_search_skip_flag;
    const TxSetType tx_set_type       = get_ext_tx_set_type(tx_size, is_inter, pcs->ppcs->frm_hdr.reduced_tx_set);
    const double    effective_ac_bias = get_effective_ac_bias(
        pcs->scs->static_config.ac_bias, pcs->slice_type == I_SLICE, pcs->temporal_layer_index);

    // resize after checks on allowable TX types
    if (ctx->mds_subres_step == 2) {
        if (tx_size == TX_64X64) {
            tx_size = TX_64X16;
        } else if (tx_size == TX_32X32) {
            tx_size = TX_32X8;
        } else if (tx_size == TX_16X16) {
            tx_size = TX_16X4;
        } else {
            assert(0);
        }
    } else if (ctx->mds_subres_step == 1) {
        if (tx_size == TX_64X64) {
            tx_size = TX_64X32;
        } else if (tx_size == TX_32X32) {
            tx_size = TX_32X16;
        } else if (tx_size == TX_16X16) {
            tx_size = TX_16X8;
        } else if (tx_size == TX_8X8) {
            tx_size = TX_8X4;
        } else {
            assert(0);
        }
    }
    EB_TRANS_COEFF_SHAPE pf_shape = ctx->pf_ctrls.pf_shape;
    if (ctx->md_stage == MD_STAGE_3 && ctx->use_tx_shortcuts_mds3) {
        pf_shape = N4_SHAPE;
    }
    // only have prev. stage coeff info if mds1/2 were performed
    else if (ctx->tx_shortcut_ctrls.apply_pf_on_coeffs && ctx->md_stage == MD_STAGE_3 && ctx->perform_mds1) {
        uint8_t use_pfn4_cond = 0;

        const uint16_t th = (txbwidth >> 4) * (txbheight_original >> 4);
        use_pfn4_cond     = (cand_bf->cnt_nz_coeff < th) || !cand_bf->block_has_coeff ? 1 : 0;
        if (use_pfn4_cond) {
            pf_shape = N4_SHAPE;
        }
    }
    uint64_t best_cost_tx_search = (uint64_t)~0;
    uint64_t dct_dct_cost        = (uint64_t)~0;
    int      best_satd_tx_search = INT_MAX;
    uint16_t satd_early_exit_th  = only_dct_dct ? 0
         : is_inter                             ? ctx->txt_ctrls.satd_early_exit_th_inter
                    : ctx->txt_ctrls.satd_early_exit_th_intra; // only compute satd when using TXT search

    if (ctx->txt_ctrls.satd_th_q_weight) {
        uint32_t q_weight, q_weight_denom;
        svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.txt_qp_based_th_scaling,
                                                &q_weight,
                                                &q_weight_denom,
                                                pcs->scs->static_config.qp);
        satd_early_exit_th = DIVIDE_AND_ROUND(satd_early_exit_th * q_weight, q_weight_denom);
    }
    int32_t  tx_type;
    uint16_t txb_origin_x           = tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x;
    uint16_t txb_origin_y           = tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y;
    uint32_t txb_origin_index       = txb_origin_x + (txb_origin_y * cand_bf->residual->stride_y);
    uint32_t input_txb_origin_index = (ctx->blk_org_x + txb_origin_x + input_pic->org_x) +
        ((ctx->blk_org_y + txb_origin_y + input_pic->org_y) * input_pic->stride_y);
    int32_t cropped_tx_width   = MIN((int)txbwidth, pcs->ppcs->aligned_width - (ctx->blk_org_x + txb_origin_x));
    int32_t cropped_tx_height  = MIN((int)txbheight, pcs->ppcs->aligned_height - (ctx->blk_org_y + txb_origin_y));
    ctx->luma_txb_skip_context = 0;
    ctx->luma_dc_sign_context  = 0;
    if (ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx) {
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_LUMA,
                            ctx->full_loop_luma_dc_sign_level_coeff_na,
                            ctx->blk_org_x + txb_origin_x,
                            ctx->blk_org_y + txb_origin_y,
                            ctx->blk_geom->bsize,
                            tx_size,
                            &ctx->luma_txb_skip_context,
                            &ctx->luma_dc_sign_context);
    }
    TxType best_tx_type = DCT_DCT;
    // local variables for all TX types
    uint16_t        eob_txt[TX_TYPES]                                              = {0};
    uint8_t         quantized_dc_txt[TX_TYPES]                                     = {0};
    uint64_t        y_txb_coeff_bits_txt[TX_TYPES]                                 = {0};
    uint64_t        txb_full_distortion_txt[DIST_TOTAL][TX_TYPES][DIST_CALC_TOTAL] = {{{0}}};
    int32_t         candidate_num                                                  = 0;
    TxType          tx_type_candidate[MAX_TX_TYPE_GROUP * TX_TYPES]                = {0};
    const SsimLevel ssim_level                                                     = ctx->tune_ssim_level;
    if (ssim_level > SSIM_LVL_0) {
        assert(ctx->pd_pass == PD_PASS_1);
        assert(ctx->md_stage == MD_STAGE_3);
    }
    const double cost_threshold_factor = derive_ssim_threshold_factor_for_tx_type_search(pcs->scs);
    int          tx_type_tot_group     = get_tx_type_group(ctx, cand_bf, only_dct_dct);
    for (int tx_type_group_idx = 0; tx_type_group_idx < tx_type_tot_group; ++tx_type_group_idx) {
        uint32_t best_tx_non_coeff = 64 * 64;
        for (int tx_type_idx = 0; tx_type_idx < TX_TYPES; ++tx_type_idx) {
            if (pcs->ppcs->sc_class1) {
                tx_type = tx_type_group_sc[tx_type_group_idx][tx_type_idx];
            } else {
                tx_type = tx_type_group[tx_type_group_idx][tx_type_idx];
            }

            if (tx_type == INVALID_TX_TYPE) {
                break;
            }

            if (only_dct_dct && tx_type != DCT_DCT) {
                continue;
            }
            if (tx_type != DCT_DCT) {
                if (av1_ext_tx_used[tx_set_type][tx_type] == 0) {
                    continue;
                }
                if (ctx->txt_ctrls.txt_rate_cost_th) {
                    const int32_t tx_type_rate = av1_txt_rate_est(
                        ctx, cand_bf, is_inter, tx_size, tx_type, pcs->ppcs->frm_hdr.reduced_tx_set);

                    // if rate cost is too high, skip testing TX type
                    if ((uint64_t)RDCOST(full_lambda, tx_type_rate, 0) * 1000 >
                        dct_dct_cost * ctx->txt_ctrls.txt_rate_cost_th) {
                        continue;
                    }
                }
            }
            // Do not use temporary buffers when TXT is OFF
            EbPictureBufferDesc* recon_coeff_ptr = (tx_type == DCT_DCT) ? cand_bf->rec_coeff
                                                                        : ctx->recon_coeff_ptr[tx_type];
            EbPictureBufferDesc* recon_ptr       = (tx_type == DCT_DCT) ? cand_bf->recon : ctx->recon_ptr[tx_type];
            EbPictureBufferDesc* quant_coeff_ptr = (tx_type == DCT_DCT) ? cand_bf->quant
                                                                        : ctx->quant_coeff_ptr[tx_type];
            ctx->three_quad_energy               = 0;
            if (!tx_search_skip_flag) {
                // Y: T Q i_q
                svt_aom_estimate_transform(pcs,
                                           ctx,
                                           &(((int16_t*)cand_bf->residual->buffer_y)[txb_origin_index]),
                                           cand_bf->residual->stride_y,
                                           &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                                           NOT_USED_VALUE,
                                           tx_size,
                                           &ctx->three_quad_energy,
                                           ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                           tx_type,
                                           PLANE_TYPE_Y,
                                           pf_shape);
                if (satd_early_exit_th) {
                    int satd = svt_aom_satd(&(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                                            (txbwidth * txbheight))
                        << ctx->mds_subres_step;

                    // If SATD of current type is better than the previous best, update best, and continue evaluating tx_type
                    if (satd < best_satd_tx_search) {
                        best_satd_tx_search = satd;
                    } else {
                        // If SATD of current type is much worse than the best then stop evaluating current tx_type
                        if ((satd - best_satd_tx_search) * 100 > best_satd_tx_search * satd_early_exit_th) {
                            continue;
                        }
                    }
                }

                quantized_dc_txt[tx_type] = svt_aom_quantize_inv_quantize(
                    pcs,
                    ctx,
                    &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                    &(((int32_t*)quant_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                    &(((int32_t*)recon_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                    qindex,
                    seg_qp,
                    tx_size,
                    &eob_txt[tx_type],
                    COMPONENT_LUMA,
                    ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                    tx_type,
                    ctx->luma_txb_skip_context,
                    ctx->luma_dc_sign_context,
                    cand_bf->cand->block_mi.mode,
                    full_lambda,
                    false);
            }
            uint32_t y_has_coeff = eob_txt[tx_type] > 0;

            // tx_type not equal to DCT_DCT and no coeff is not an acceptable option in AV1.
            if (y_has_coeff == 0 && tx_type != DCT_DCT) {
                continue;
            }

            // Perform inverse TX if using spatial SSE or INTRA and tx_depth > 0
            if (ctx->mds_do_spatial_sse || (!is_inter && cand_bf->cand->block_mi.tx_depth)) {
                if (y_has_coeff) {
                    svt_aom_inv_transform_recon_wrapper(pcs,
                                                        ctx,
                                                        cand_bf->pred->buffer_y,
                                                        txb_origin_index,
                                                        cand_bf->pred->stride_y,
                                                        recon_ptr->buffer_y,
                                                        txb_origin_index,
                                                        cand_bf->recon->stride_y,
                                                        (int32_t*)recon_coeff_ptr->buffer_y,
                                                        ctx->txb_1d_offset,
                                                        ctx->hbd_md,
                                                        tx_size_original,
                                                        tx_type,
                                                        PLANE_TYPE_Y,
                                                        (uint32_t)eob_txt[tx_type]);
                } else {
                    svt_av1_picture_copy_y(cand_bf->pred,
                                           txb_origin_index,
                                           recon_ptr,
                                           txb_origin_index,
                                           txbwidth,
                                           txbheight_original,
                                           ctx->hbd_md);
                }

                EbSpatialFullDistType spatial_full_dist_type_fun = ctx->hbd_md ? svt_full_distortion_kernel16_bits
                                                                               : svt_spatial_full_distortion_kernel;
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] = spatial_full_dist_type_fun(
                    input_pic->buffer_y,
                    input_txb_origin_index,
                    input_pic->stride_y,
                    cand_bf->pred->buffer_y,
                    (int32_t)txb_origin_index,
                    cand_bf->pred->stride_y,
                    cropped_tx_width,
                    cropped_tx_height);
                if (effective_ac_bias) {
                    txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] += get_svt_psy_full_dist(
                        input_pic->buffer_y,
                        input_txb_origin_index,
                        input_pic->stride_y,
                        cand_bf->pred->buffer_y,
                        (int32_t)txb_origin_index,
                        cand_bf->pred->stride_y,
                        cropped_tx_width,
                        cropped_tx_height,
                        ctx->hbd_md,
                        effective_ac_bias);
                }
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] = spatial_full_dist_type_fun(
                    input_pic->buffer_y,
                    input_txb_origin_index,
                    input_pic->stride_y,
                    recon_ptr->buffer_y,
                    (int32_t)txb_origin_index,
                    cand_bf->recon->stride_y,
                    cropped_tx_width,
                    cropped_tx_height);
                if (effective_ac_bias) {
                    txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] += get_svt_psy_full_dist(
                        input_pic->buffer_y,
                        input_txb_origin_index,
                        input_pic->stride_y,
                        recon_ptr->buffer_y,
                        (int32_t)txb_origin_index,
                        cand_bf->recon->stride_y,
                        cropped_tx_width,
                        cropped_tx_height,
                        ctx->hbd_md,
                        effective_ac_bias);
                }
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] <<= 4;
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] <<= 4;
            } else {
                // LUMA DISTORTION
                uint32_t bwidth, bheight;
                if (pf_shape && !tx_search_skip_flag) {
                    bwidth  = MAX((txbwidth >> pf_shape), 4);
                    bheight = (txbheight >> pf_shape);
                } else {
                    bwidth  = txbwidth < 64 ? txbwidth : 32;
                    bheight = txbheight < 64 ? txbheight : 32;
                }
                svt_aom_picture_full_distortion32_bits_single(
                    &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                    &(((int32_t*)recon_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                    txbwidth < 64 ? txbwidth : 32,
                    bwidth,
                    bheight,
                    txb_full_distortion_txt[DIST_SSD][tx_type],
                    eob_txt[tx_type]);
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] += ctx->three_quad_energy;
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] += ctx->three_quad_energy;
                //assert(ctx->three_quad_energy == 0 && ctx->cu_stats->size < 64);
                const int32_t shift = (MAX_TX_SCALE - av1_get_tx_scale_tab[tx_size]) * 2;
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] = RIGHT_SIGNED_SHIFT(
                    txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL], shift);
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] = RIGHT_SIGNED_SHIFT(
                    txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION], shift);
            }
            txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] =
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL] << ctx->mds_subres_step;
            txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] =
                txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_PREDICTION] << ctx->mds_subres_step;
            // Do not perform rate estimation @ tx_type search if current tx_type dist is higher than best_cost
            uint64_t early_cost = RDCOST(
                full_lambda, 0, txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL]);
            if (early_cost > best_cost_tx_search) {
                continue;
            }
            //LUMA-ONLY
            uint64_t th = (txbwidth * txbheight_original) >> 6;
            if ((ctx->rate_est_ctrls.coeff_rate_est_lvl >= 2 || ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) &&
                (eob_txt[tx_type] < (th))) {
                y_txb_coeff_bits_txt[tx_type] = 6000 + eob_txt[tx_type] * 1000;
            } else if (ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) {
                y_txb_coeff_bits_txt[tx_type] = 3000 + eob_txt[tx_type] * 100;
            } else {
                svt_aom_txb_estimate_coeff_bits(ctx,
                                                0, //allow_update_cdf,
                                                NULL, //FRAME_CONTEXT *ec_ctx,
                                                pcs,
                                                cand_bf,
                                                ctx->txb_1d_offset,
                                                0,
                                                quant_coeff_ptr,
                                                eob_txt[tx_type],
                                                0,
                                                0,
                                                &(y_txb_coeff_bits_txt[tx_type]),
                                                &(y_txb_coeff_bits_txt[tx_type]),
                                                &(y_txb_coeff_bits_txt[tx_type]),
                                                tx_size,
                                                NOT_USED_VALUE,
                                                tx_type,
                                                NOT_USED_VALUE,
                                                COMPONENT_LUMA);
            }
            tx_type_candidate[candidate_num] = tx_type; // tx types which will compute ssim
            ++candidate_num;

            uint64_t cost = RDCOST(full_lambda,
                                   y_txb_coeff_bits_txt[tx_type],
                                   txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL]);
            if (cost < best_cost_tx_search) {
                best_cost_tx_search = cost;
                best_tx_type        = tx_type;
                best_tx_non_coeff   = eob_txt[tx_type];
                if (tx_type == DCT_DCT) {
                    dct_dct_cost = cost;
                }
            }

            // Skip remaining TX types based on absolute cost TH and absolute # of coeffs TH
            if (ssim_level <= SSIM_LVL_1 && !only_dct_dct) {
                uint32_t coeff_th      = ctx->txt_ctrls.early_exit_coeff_th;
                uint32_t dist_err_unit = ctx->txt_ctrls.early_exit_dist_th;
                uint32_t dist_err      = txbwidth * txbheight_original * dist_err_unit;

                uint64_t cost_th = dist_err_unit ? RDCOST(full_lambda, 1, dist_err)
                                                 : 0; // if distortion th=0, set cost_th to 0

                if (best_tx_non_coeff < coeff_th || best_cost_tx_search < cost_th) {
                    tx_type_idx       = TX_TYPES;
                    tx_type_group_idx = tx_type_tot_group;
                }
            }
        }
    }

    if (ssim_level > SSIM_LVL_1) {
        const uint64_t ssd_cost_threshold       = (uint64_t)(cost_threshold_factor * best_cost_tx_search);
        uint64_t       best_ssim_cost_tx_search = (uint64_t)~0;
        for (int i = 0; i < candidate_num; ++i) {
            tx_type           = tx_type_candidate[i];
            uint64_t ssd_cost = RDCOST(full_lambda,
                                       y_txb_coeff_bits_txt[tx_type],
                                       txb_full_distortion_txt[DIST_SSD][tx_type][DIST_CALC_RESIDUAL]);
            if (ssd_cost > ssd_cost_threshold) {
                continue;
            }
            EbPictureBufferDesc* recon_ptr = (tx_type == DCT_DCT) ? cand_bf->recon : ctx->recon_ptr[tx_type];

            txb_full_distortion_txt[DIST_SSIM][tx_type][DIST_CALC_RESIDUAL] = svt_spatial_full_distortion_ssim_kernel(
                input_pic->buffer_y,
                input_txb_origin_index,
                input_pic->stride_y,
                recon_ptr->buffer_y,
                (int32_t)txb_origin_index,
                cand_bf->recon->stride_y,
                cropped_tx_width,
                cropped_tx_height,
                ctx->hbd_md,
                effective_ac_bias);

            txb_full_distortion_txt[DIST_SSIM][tx_type][DIST_CALC_RESIDUAL] <<= 4;

            txb_full_distortion_txt[DIST_SSIM][tx_type][DIST_CALC_RESIDUAL] =
                txb_full_distortion_txt[DIST_SSIM][tx_type][DIST_CALC_RESIDUAL] << ctx->mds_subres_step;

            //
            uint64_t ssim_cost = RDCOST(full_lambda,
                                        y_txb_coeff_bits_txt[tx_type],
                                        txb_full_distortion_txt[DIST_SSIM][tx_type][DIST_CALC_RESIDUAL]);

            if (ssim_cost < best_ssim_cost_tx_search) {
                best_cost_tx_search      = ssd_cost;
                best_ssim_cost_tx_search = ssim_cost;
                best_tx_type             = tx_type;
            } else if (ssim_cost == best_ssim_cost_tx_search) {
                // if two candidates have the same ssim cost, choose the one with lower ssd cost
                if (ssd_cost < best_cost_tx_search) {
                    best_cost_tx_search      = ssd_cost;
                    best_ssim_cost_tx_search = ssim_cost;
                    best_tx_type             = tx_type;
                }
            }
        }
    }

    //  Best Tx Type Pass
    cand_bf->cand->transform_type[ctx->txb_itr] = best_tx_type;
    // update with best_tx_type data
    (*y_coeff_bits) += y_txb_coeff_bits_txt[best_tx_type];
    if (ssim_level == SSIM_LVL_1) {
        EbPictureBufferDesc* recon_ptr      = (best_tx_type == DCT_DCT) ? cand_bf->recon : ctx->recon_ptr[best_tx_type];
        uint64_t             ssim_pred_dist = svt_spatial_full_distortion_ssim_kernel(input_pic->buffer_y,
                                                                          input_txb_origin_index,
                                                                          input_pic->stride_y,
                                                                          cand_bf->pred->buffer_y,
                                                                          (int32_t)txb_origin_index,
                                                                          cand_bf->pred->stride_y,
                                                                          cropped_tx_width,
                                                                          cropped_tx_height,
                                                                          ctx->hbd_md,
                                                                          effective_ac_bias);
        uint64_t             ssim_residual_dist = svt_spatial_full_distortion_ssim_kernel(input_pic->buffer_y,
                                                                              input_txb_origin_index,
                                                                              input_pic->stride_y,
                                                                              recon_ptr->buffer_y,
                                                                              (int32_t)txb_origin_index,
                                                                              cand_bf->recon->stride_y,
                                                                              cropped_tx_width,
                                                                              cropped_tx_height,
                                                                              ctx->hbd_md,
                                                                              effective_ac_bias);
        ssim_pred_dist <<= (4 + ctx->mds_subres_step);
        ssim_residual_dist <<= (4 + ctx->mds_subres_step);

        y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] += ssim_pred_dist;
        y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL] += ssim_residual_dist;
    } else if (ssim_level == SSIM_LVL_3) {
        uint64_t ssim_pred_dist = svt_spatial_full_distortion_ssim_kernel(input_pic->buffer_y,
                                                                          input_txb_origin_index,
                                                                          input_pic->stride_y,
                                                                          cand_bf->pred->buffer_y,
                                                                          (int32_t)txb_origin_index,
                                                                          cand_bf->pred->stride_y,
                                                                          cropped_tx_width,
                                                                          cropped_tx_height,
                                                                          ctx->hbd_md,
                                                                          effective_ac_bias);
        ssim_pred_dist <<= (4 + ctx->mds_subres_step);

        y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL] +=
            txb_full_distortion_txt[DIST_SSIM][best_tx_type][DIST_CALC_RESIDUAL];
        y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] += ssim_pred_dist;
    } else if (ssim_level == SSIM_LVL_2) {
        // it doesn't need to update y_full_distortion[DIST_SSIM] since ssim is only used to select best tx type
    }

    y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] +=
        txb_full_distortion_txt[DIST_SSD][best_tx_type][DIST_CALC_RESIDUAL];
    y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] +=
        txb_full_distortion_txt[DIST_SSD][best_tx_type][DIST_CALC_PREDICTION];

    cand_bf->y_has_coeff |= ((eob_txt[best_tx_type] > 0) << ctx->txb_itr);
    cand_bf->quant_dc.y[ctx->txb_itr] = quantized_dc_txt[best_tx_type];
    cand_bf->eob.y[ctx->txb_itr]      = eob_txt[best_tx_type];
    // Do not copy when the best TXT data is already in cand_bf
    if (best_tx_type != DCT_DCT) {
        // copy best_tx_type data
        copy_txt_data(cand_bf, ctx, txb_origin_index, best_tx_type);
    }
    ctx->txb_1d_offset += txbwidth * txbheight;
    // For Inter blocks, transform type of chroma follows luma transfrom type
    if (is_inter && ctx->txb_itr == 0) {
        const TxSize    tx_size_uv     = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
        const TxSetType tx_set_type_uv = get_ext_tx_set_type(tx_size_uv, is_inter, pcs->ppcs->frm_hdr.reduced_tx_set);
        if (av1_ext_tx_used[tx_set_type_uv][best_tx_type] == 0) {
            cand_bf->cand->transform_type_uv = DCT_DCT;
        } else {
            cand_bf->cand->transform_type_uv = cand_bf->cand->transform_type[ctx->txb_itr];
        }
    }
}

static void init_tx_cand_bf(ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx, uint8_t tx_depth,
                            uint8_t is_inter) {
    uint32_t block_index = 0;
    if (tx_depth == 1) {
        svt_memcpy(ctx->cand_bf_tx_depth_1->cand, cand_bf->cand, sizeof(ModeDecisionCandidate));
        ctx->cand_bf_tx_depth_1->block_has_coeff = cand_bf->block_has_coeff;
        if (is_inter) {
            if (ctx->hbd_md) {
                // Copy pred to tx_depth_1 cand_bf
                {
                    uint16_t* src = &(((uint16_t*)cand_bf->pred->buffer_y)[block_index]);
                    uint16_t* dst = &(((uint16_t*)ctx->cand_bf_tx_depth_1->pred->buffer_y)[block_index]);
                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth * sizeof(uint16_t));
                        src += cand_bf->pred->stride_y;
                        dst += ctx->cand_bf_tx_depth_1->pred->stride_y;
                    }
                }
                // Copy residual to tx_depth_1 cand_bf
                {
                    int16_t* src = &(((int16_t*)cand_bf->residual->buffer_y)[block_index]);
                    int16_t* dst = &(((int16_t*)ctx->cand_bf_tx_depth_1->residual->buffer_y)[block_index]);

                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth << 1);
                        src += cand_bf->residual->stride_y;
                        dst += ctx->cand_bf_tx_depth_1->residual->stride_y;
                    }
                }
            } else {
                // Copy pred to tx_depth_1 cand_bf
                {
                    EbByte src = &(cand_bf->pred->buffer_y[block_index]);
                    EbByte dst = &(ctx->cand_bf_tx_depth_1->pred->buffer_y[block_index]);
                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth);
                        src += cand_bf->pred->stride_y;
                        dst += ctx->cand_bf_tx_depth_1->pred->stride_y;
                    }
                }
                // Copy residual to tx_depth_1 cand_bf
                {
                    int16_t* src = &(((int16_t*)cand_bf->residual->buffer_y)[block_index]);
                    int16_t* dst = &(((int16_t*)ctx->cand_bf_tx_depth_1->residual->buffer_y)[block_index]);

                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth << 1);
                        src += cand_bf->residual->stride_y;
                        dst += ctx->cand_bf_tx_depth_1->residual->stride_y;
                    }
                }
            }
        }
    }
    if (tx_depth == 2) {
        svt_memcpy(ctx->cand_bf_tx_depth_2->cand, cand_bf->cand, sizeof(ModeDecisionCandidate));

        ctx->cand_bf_tx_depth_2->block_has_coeff = cand_bf->block_has_coeff;
        if (is_inter) {
            if (ctx->hbd_md) {
                // Copy pred to tx_depth_1 cand_bf
                {
                    uint16_t* src = &(((uint16_t*)cand_bf->pred->buffer_y)[block_index]);
                    uint16_t* dst = &(((uint16_t*)ctx->cand_bf_tx_depth_2->pred->buffer_y)[block_index]);

                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth * sizeof(uint16_t));
                        src += cand_bf->pred->stride_y;
                        dst += ctx->cand_bf_tx_depth_2->pred->stride_y;
                    }
                }
                // Copy residual to tx_depth_1 cand_bf
                {
                    int16_t* src = &(((int16_t*)cand_bf->residual->buffer_y)[block_index]);
                    int16_t* dst = &(((int16_t*)ctx->cand_bf_tx_depth_2->residual->buffer_y)[block_index]);

                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth << 1);
                        src += cand_bf->residual->stride_y;
                        dst += ctx->cand_bf_tx_depth_2->residual->stride_y;
                    }
                }
            } else {
                // Copy pred to tx_depth_2 cand_bf
                {
                    EbByte src = &(cand_bf->pred->buffer_y[block_index]);
                    EbByte dst = &(ctx->cand_bf_tx_depth_2->pred->buffer_y[block_index]);
                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth);
                        src += cand_bf->pred->stride_y;
                        dst += ctx->cand_bf_tx_depth_2->pred->stride_y;
                    }
                }
                // Copy residual to tx_depth_2 cand_bf
                {
                    int16_t* src = &(((int16_t*)cand_bf->residual->buffer_y)[block_index]);
                    int16_t* dst = &(((int16_t*)ctx->cand_bf_tx_depth_2->residual->buffer_y)[block_index]);

                    for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                        svt_memcpy(dst, src, ctx->blk_geom->bwidth << 1);
                        src += cand_bf->residual->stride_y;
                        dst += ctx->cand_bf_tx_depth_2->residual->stride_y;
                    }
                }
            }
        }
    }
}

void update_tx_cand_bf(ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx, uint8_t best_tx_depth) {
    uint32_t block_index = 0;
    if (best_tx_depth == 1) {
        // Copy depth 1 mode/type/eob ..
        svt_memcpy(cand_bf->cand, ctx->cand_bf_tx_depth_1->cand, sizeof(ModeDecisionCandidate));
        svt_memcpy(cand_bf->eob.y, ctx->cand_bf_tx_depth_1->eob.y, sizeof(uint16_t) * 1 /*copy luma*/ * MAX_TXB_COUNT);
        svt_memcpy(cand_bf->quant_dc.y,
                   ctx->cand_bf_tx_depth_1->quant_dc.y,
                   sizeof(int32_t) * 1 /*copy luma*/ * MAX_TXB_COUNT);

        cand_bf->y_has_coeff = ctx->cand_bf_tx_depth_1->y_has_coeff;
        // Copy depth 1 pred
        if (ctx->hbd_md) {
            uint16_t* src = &(((uint16_t*)ctx->cand_bf_tx_depth_1->pred->buffer_y)[block_index]);
            uint16_t* dst = &(((uint16_t*)cand_bf->pred->buffer_y)[block_index]);
            for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                svt_memcpy(dst, src, ctx->blk_geom->bwidth * sizeof(uint16_t));
                src += ctx->cand_bf_tx_depth_1->pred->stride_y;
                dst += cand_bf->pred->stride_y;
            }
        } else {
            EbByte src = &(ctx->cand_bf_tx_depth_1->pred->buffer_y[block_index]);
            EbByte dst = &(cand_bf->pred->buffer_y[block_index]);
            for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                svt_memcpy(dst, src, ctx->blk_geom->bwidth);
                src += ctx->cand_bf_tx_depth_1->pred->stride_y;
                dst += cand_bf->pred->stride_y;
            }
        }
        // Copy depth 1 recon coeff
        svt_memcpy(cand_bf->rec_coeff->buffer_y,
                   ctx->cand_bf_tx_depth_1->rec_coeff->buffer_y,
                   (ctx->blk_geom->bwidth * ctx->blk_geom->bheight << 2));
        svt_memcpy(cand_bf->quant->buffer_y,
                   ctx->cand_bf_tx_depth_1->quant->buffer_y,
                   (ctx->blk_geom->bwidth * ctx->blk_geom->bheight << 2));
    }
    if (best_tx_depth == 2) {
        // Copy depth 2 mode/type/eob ..
        svt_memcpy(cand_bf->cand, ctx->cand_bf_tx_depth_2->cand, sizeof(ModeDecisionCandidate));
        svt_memcpy(cand_bf->eob.y, ctx->cand_bf_tx_depth_2->eob.y, sizeof(uint16_t) * 1 /*copy luma*/ * MAX_TXB_COUNT);
        svt_memcpy(cand_bf->quant_dc.y,
                   ctx->cand_bf_tx_depth_2->quant_dc.y,
                   sizeof(int32_t) * 1 /*copy luma*/ * MAX_TXB_COUNT);

        cand_bf->y_has_coeff = ctx->cand_bf_tx_depth_2->y_has_coeff;
        // Copy depth 2 pred
        if (ctx->hbd_md) {
            uint16_t* src = &(((uint16_t*)ctx->cand_bf_tx_depth_2->pred->buffer_y)[block_index]);
            uint16_t* dst = &(((uint16_t*)cand_bf->pred->buffer_y)[block_index]);
            for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                svt_memcpy(dst, src, ctx->blk_geom->bwidth * sizeof(uint16_t));
                src += ctx->cand_bf_tx_depth_2->pred->stride_y;
                dst += cand_bf->pred->stride_y;
            }
        } else {
            EbByte src = &(ctx->cand_bf_tx_depth_2->pred->buffer_y[block_index]);
            EbByte dst = &(cand_bf->pred->buffer_y[block_index]);
            for (int i = 0; i < ctx->blk_geom->bheight; i++) {
                svt_memcpy(dst, src, ctx->blk_geom->bwidth);
                src += ctx->cand_bf_tx_depth_2->pred->stride_y;
                dst += cand_bf->pred->stride_y;
            }
        }
        // Copy depth 2 recon coeff
        svt_memcpy(cand_bf->rec_coeff->buffer_y,
                   ctx->cand_bf_tx_depth_2->rec_coeff->buffer_y,
                   (ctx->blk_geom->bwidth * ctx->blk_geom->bheight << 2));
        svt_memcpy(cand_bf->quant->buffer_y,
                   ctx->cand_bf_tx_depth_2->quant->buffer_y,
                   (ctx->blk_geom->bwidth * ctx->blk_geom->bheight << 2));
    }
}

static void perform_tx_partitioning(ModeDecisionCandidateBuffer* cand_bf, ModeDecisionContext* ctx,
                                    PictureControlSet* pcs, uint8_t start_tx_depth, uint8_t end_tx_depth,
                                    uint32_t qindex, uint64_t* y_coeff_bits,
                                    uint64_t y_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL]) {
    uint32_t full_lambda           = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    EbPictureBufferDesc* input_pic = ctx->hbd_md ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
    const bool is_inter = (is_inter_mode(cand_bf->cand->block_mi.mode) || cand_bf->cand->block_mi.use_intrabc) ? true
                                                                                                               : false;
    uint8_t    best_tx_depth    = 0;
    uint64_t   best_cost_search = (uint64_t)~0;
    uint32_t   best_coeff_count = (uint32_t)~0;
    // Transform Depth Loop
    for (ctx->tx_depth = start_tx_depth; ctx->tx_depth <= end_tx_depth; ctx->tx_depth++) {
        if (best_coeff_count < ctx->txs_ctrls.prev_depth_coeff_exit_th) {
            continue;
        }
        if (ctx->tx_depth) {
            init_tx_cand_bf(cand_bf, ctx, ctx->tx_depth, is_inter);
            tx_reset_neighbor_arrays(pcs, ctx, is_inter, ctx->tx_depth);
        }
        ModeDecisionCandidateBuffer* tx_cand_bf = (ctx->tx_depth == 0) ? cand_bf
            : (ctx->tx_depth == 1)                                     ? ctx->cand_bf_tx_depth_1
                                                                       : ctx->cand_bf_tx_depth_2;
        tx_cand_bf->cand->block_mi.tx_depth     = ctx->tx_depth;
        if (ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx || !is_inter) {
            tx_initialize_neighbor_arrays(pcs, ctx, is_inter);
        }

        // Initialize TU Split
        uint64_t tx_y_coeff_bits                                   = 0;
        uint64_t tx_y_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};

        ctx->txb_1d_offset      = 0;
        tx_cand_bf->y_has_coeff = 0;

        const TxSize   tx_size   = tx_depth_to_tx_size[ctx->tx_depth][ctx->blk_geom->bsize];
        const int      tx_width  = tx_size_wide[tx_size];
        const int      tx_height = tx_size_high[tx_size];
        const uint16_t txb_count = tx_blocks_per_depth[ctx->blk_geom->bsize][ctx->tx_depth];

        uint32_t block_has_coeff = false;
        for (ctx->txb_itr = 0; ctx->txb_itr < txb_count; ctx->txb_itr++) {
            // Y Prediction
            if (!is_inter) {
                const uint16_t tx_org_x         = tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].x;
                const uint16_t tx_org_y         = tx_org[ctx->blk_geom->bsize][is_inter][ctx->tx_depth][ctx->txb_itr].y;
                const uint32_t txb_origin_index = tx_org_x + (tx_org_y * tx_cand_bf->residual->stride_y);
                const uint32_t input_txb_origin_index = (ctx->blk_org_x + tx_org_x + input_pic->org_x) +
                    ((ctx->blk_org_y + tx_org_y + input_pic->org_y) * input_pic->stride_y);
                // This check assumes no txs search @ a previous md_stage()
                if (ctx->tx_depth) {
                    av1_intra_luma_prediction(ctx, pcs, tx_cand_bf);
                }

                // Y Residual
                svt_aom_residual_kernel(input_pic->buffer_y,
                                        input_txb_origin_index,
                                        input_pic->stride_y << ctx->mds_subres_step,
                                        tx_cand_bf->pred->buffer_y,
                                        txb_origin_index,
                                        tx_cand_bf->pred->stride_y << ctx->mds_subres_step,
                                        (int16_t*)tx_cand_bf->residual->buffer_y,
                                        txb_origin_index,
                                        tx_cand_bf->residual->stride_y,
                                        ctx->hbd_md,
                                        tx_width,
                                        tx_height >> ctx->mds_subres_step);
            }
            uint8_t tx_search_skip_flag = 0;
            // only have prev. stage coeff info if mds1/2 were performed
            if (ctx->tx_shortcut_ctrls.bypass_tx_th && ctx->md_stage == MD_STAGE_3 && ctx->perform_mds1 &&
                !cand_bf->block_has_coeff &&
                ((cand_bf->luma_fast_dist * ctx->tx_shortcut_ctrls.bypass_tx_th) <
                 (uint32_t)(ctx->blk_geom->bheight * ctx->blk_geom->bwidth * ctx->qp_index))) {
                tx_search_skip_flag = 1;
            }
            tx_type_search(pcs, ctx, tx_cand_bf, qindex, tx_search_skip_flag, &tx_y_coeff_bits, tx_y_full_distortion);

            uint32_t y_has_coeff = tx_cand_bf->eob.y[ctx->txb_itr] > 0;

            if (ctx->tx_depth) {
                tx_update_neighbor_arrays(pcs, ctx, tx_cand_bf, is_inter);
            }

            if (y_has_coeff) {
                block_has_coeff = true;
            }

            uint64_t current_tx_cost = RDCOST(
                full_lambda, tx_y_coeff_bits, tx_y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]);
            if (current_tx_cost > best_cost_search) {
                break;
            }

            if (ctx->txs_ctrls.quadrant_th_sf && ctx->tx_depth > 0) {
                uint64_t normlized_cost = ((ctx->txb_itr + 1) * best_cost_search) / txb_count;

                uint64_t       tx_size_bit_tmp = pcs->ppcs->frm_hdr.tx_mode == TX_MODE_SELECT
                          ? svt_aom_get_tx_size_bits(tx_cand_bf, ctx, pcs, ctx->tx_depth, block_has_coeff)
                          : 0;
                const uint64_t cost_tmp        = RDCOST(
                    full_lambda, tx_y_coeff_bits + tx_size_bit_tmp, tx_y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]);

                if (cost_tmp * 100 > normlized_cost * ctx->txs_ctrls.quadrant_th_sf) {
                    tx_y_coeff_bits                                    = MAX_MODE_COST;
                    tx_y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] = MAX_MODE_COST;
                    break;
                }
            }
        } // Transform Loop

        if (end_tx_depth) {
            const uint64_t tx_size_bit = pcs->ppcs->frm_hdr.tx_mode == TX_MODE_SELECT
                ? svt_aom_get_tx_size_bits(tx_cand_bf, ctx, pcs, ctx->tx_depth, block_has_coeff)
                : 0;

            const uint64_t cost = RDCOST(
                full_lambda, tx_y_coeff_bits + tx_size_bit, tx_y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]);
            if (cost < best_cost_search) {
                best_cost_search                                 = cost;
                best_tx_depth                                    = ctx->tx_depth;
                y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL] = tx_y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL];
                y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] =
                    tx_y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION];

                y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] = tx_y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL];
                y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] =
                    tx_y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION];
                *y_coeff_bits    = tx_y_coeff_bits;
                best_coeff_count = 0;
                for (ctx->txb_itr = 0; ctx->txb_itr < txb_count; ctx->txb_itr++) {
                    best_coeff_count += tx_cand_bf->eob.y[ctx->txb_itr];
                }
            }
        } else {
            y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL]   = tx_y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL];
            y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = tx_y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION];

            y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = tx_y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL];
            y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = tx_y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION];
            *y_coeff_bits                                     = tx_y_coeff_bits;
        }
    } // Transform Depth Loop

    if (best_tx_depth) {
        update_tx_cand_bf(cand_bf, ctx, best_tx_depth);
    }
}

/*
   DCT_DCT path for light PD1
*/
static void perform_dct_dct_tx_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                         ModeDecisionCandidateBuffer* cand_bf, BlockLocation* loc,
                                         uint64_t* y_coeff_bits, uint64_t* y_full_distortion) {
    uint32_t full_lambda           = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    EbPictureBufferDesc* input_pic = ctx->hbd_md ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
    const bool           is_inter  = is_inter_mode(cand_bf->cand->block_mi.mode) ? true : false;
    const double         effective_ac_bias = get_effective_ac_bias(
        pcs->scs->static_config.ac_bias, pcs->slice_type == I_SLICE, pcs->temporal_layer_index);

    ctx->three_quad_energy = 0;
    svt_aom_residual_kernel(input_pic->buffer_y,
                            loc->input_origin_index,
                            input_pic->stride_y,
                            cand_bf->pred->buffer_y,
                            0,
                            cand_bf->pred->stride_y,
                            (int16_t*)cand_bf->residual->buffer_y,
                            0,
                            cand_bf->residual->stride_y,
                            ctx->hbd_md,
                            ctx->blk_geom->bwidth,
                            ctx->blk_geom->bheight);
    const TxSize tx_size   = tx_depth_to_tx_size[0][ctx->blk_geom->bsize];
    const int    txbwidth  = tx_size_wide[tx_size];
    const int    txbheight = tx_size_high[tx_size];
    assert(tx_size < TX_SIZES_ALL);
    EB_TRANS_COEFF_SHAPE pf_shape = ctx->pf_ctrls.pf_shape;
    if (ctx->use_tx_shortcuts_mds3 && ctx->rtc_use_N4_dct_dct_shortcut) {
        pf_shape = N4_SHAPE;
    } else if (ctx->lpd1_tx_ctrls.use_neighbour_info) {
        MacroBlockD* xd = ctx->blk_ptr->av1xd;
        if (xd->left_available && xd->up_available && ctx->blk_geom->sq_size > 16 && ctx->is_subres_safe == 1) {
            const BlockModeInfo* const left_mi  = &xd->left_mbmi->block_mi;
            const BlockModeInfo* const above_mi = &xd->above_mbmi->block_mi;
            if (left_mi->skip && above_mi->skip) {
                ctx->use_tx_shortcuts_mds3 = 1;
                pf_shape                   = N2_SHAPE;
                // PF for MVP cands
                if (is_inter_mode(cand_bf->cand->block_mi.mode) &&
                    (cand_bf->cand->block_mi.mode != NEWMV && cand_bf->cand->block_mi.mode != NEW_NEWMV)) {
                    pf_shape = N4_SHAPE;
                }
                // PF for NRST_NRST
                else if (cand_bf->cand->block_mi.mode == NEAREST_NEARESTMV) {
                    pf_shape = N4_SHAPE;
                }
            }
        }
    }

    EbPictureBufferDesc* const recon_coeff_ptr = cand_bf->rec_coeff;
    EbPictureBufferDesc* const quant_coeff_ptr = cand_bf->quant;

    // Y: T Q i_q
    svt_aom_estimate_transform(pcs,
                               ctx,
                               (int16_t*)cand_bf->residual->buffer_y,
                               cand_bf->residual->stride_y,
                               (int32_t*)ctx->tx_coeffs->buffer_y,
                               NOT_USED_VALUE,
                               tx_size,
                               &ctx->three_quad_energy,
                               ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                               DCT_DCT,
                               PLANE_TYPE_Y,
                               pf_shape);
    cand_bf->quant_dc.y[0] = svt_aom_quantize_inv_quantize(pcs,
                                                           ctx,
                                                           (int32_t*)ctx->tx_coeffs->buffer_y,
                                                           (int32_t*)quant_coeff_ptr->buffer_y,
                                                           (int32_t*)recon_coeff_ptr->buffer_y,
                                                           ctx->blk_ptr->qindex,
                                                           0,
                                                           tx_size,
                                                           &(cand_bf->eob.y[0]),
                                                           COMPONENT_LUMA,
                                                           ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                                           DCT_DCT,
                                                           0,
                                                           0,
                                                           cand_bf->cand->block_mi.mode,
                                                           full_lambda,
                                                           false);
    // LUMA DISTORTION
    uint32_t bwidth, bheight;
    if (pf_shape) {
        bwidth  = MAX((txbwidth >> pf_shape), 4);
        bheight = (txbheight >> pf_shape);
    } else {
        bwidth  = txbwidth < 64 ? txbwidth : 32;
        bheight = txbheight < 64 ? txbheight : 32;
    }

    svt_aom_picture_full_distortion32_bits_single((int32_t*)ctx->tx_coeffs->buffer_y,
                                                  (int32_t*)recon_coeff_ptr->buffer_y,
                                                  txbwidth < 64 ? txbwidth : 32,
                                                  bwidth,
                                                  bheight,
                                                  y_full_distortion,
                                                  cand_bf->eob.y[0]);
    const int32_t shift                   = (MAX_TX_SCALE - av1_get_tx_scale_tab[tx_size]) * 2;
    y_full_distortion[DIST_CALC_RESIDUAL] = RIGHT_SIGNED_SHIFT(
        y_full_distortion[DIST_CALC_RESIDUAL] + ctx->three_quad_energy, shift);
    y_full_distortion[DIST_CALC_PREDICTION] = RIGHT_SIGNED_SHIFT(
        y_full_distortion[DIST_CALC_PREDICTION] + ctx->three_quad_energy, shift);
    //LUMA-ONLY
    const uint32_t th = ((txbwidth * txbheight) >> 6);
    if ((ctx->rate_est_ctrls.coeff_rate_est_lvl >= 2 || ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) &&
        cand_bf->eob.y[0] < th) {
        *y_coeff_bits = 6000 + cand_bf->eob.y[0] * 1000;
    } else if (ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) {
        *y_coeff_bits = 6000 + cand_bf->eob.y[0] * 400;
    } else {
        svt_aom_txb_estimate_coeff_bits(ctx,
                                        0,
                                        NULL,
                                        pcs,
                                        cand_bf,
                                        0,
                                        0,
                                        quant_coeff_ptr,
                                        cand_bf->eob.y[0],
                                        0,
                                        0,
                                        y_coeff_bits,
                                        NULL,
                                        NULL,
                                        tx_size,
                                        NOT_USED_VALUE,
                                        DCT_DCT,
                                        NOT_USED_VALUE,
                                        COMPONENT_LUMA);
    }

    if (effective_ac_bias) {
        *y_coeff_bits = svt_psy_adjust_rate_light(
            (int32_t*)recon_coeff_ptr->buffer_y, *y_coeff_bits, bwidth, bheight, effective_ac_bias);
    }

    //Update with best_tx_type data
    cand_bf->cand->transform_type[0] = DCT_DCT;
    cand_bf->y_has_coeff             = (cand_bf->eob.y[0] > 0);
    // For Inter blocks, transform type of chroma follows luma transfrom type
    if (is_inter) {
        cand_bf->cand->transform_type_uv = DCT_DCT;
    }
}

// TX path when TXT and TXS are off
static void perform_dct_dct_tx(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                               uint8_t tx_search_skip_flag, uint32_t qindex, uint64_t* y_coeff_bits,
                               uint64_t y_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL]) {
    const uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    EbPictureBufferDesc* const input_pic = ctx->hbd_md ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
    const bool is_inter    = (is_inter_mode(cand_bf->cand->block_mi.mode) || cand_bf->cand->block_mi.use_intrabc) ? true
                                                                                                                  : false;
    ctx->tx_depth          = 0;
    ctx->txb_itr           = 0;
    ctx->txb_1d_offset     = 0;
    ctx->three_quad_energy = 0;
    const int tx_depth     = 0;
    const int txb_itr      = 0;
    const int txb_1d_offset = 0;
    const int tx_type       = DCT_DCT;

    TxSize         tx_size                = tx_depth_to_tx_size[tx_depth][ctx->blk_geom->bsize];
    const int      tx_width               = tx_size_wide[tx_size];
    const int      tx_height              = tx_size_high[tx_size];
    const uint16_t tx_org_x               = tx_org[ctx->blk_geom->bsize][is_inter][tx_depth][txb_itr].x;
    const uint16_t tx_org_y               = tx_org[ctx->blk_geom->bsize][is_inter][tx_depth][txb_itr].y;
    const uint32_t txb_origin_index       = tx_org_x + (tx_org_y * cand_bf->residual->stride_y);
    const uint32_t input_txb_origin_index = (ctx->blk_org_x + tx_org_x + input_pic->org_x) +
        ((ctx->blk_org_y + tx_org_y + input_pic->org_y) * input_pic->stride_y);

    const double effective_ac_bias = get_effective_ac_bias(
        pcs->scs->static_config.ac_bias, pcs->slice_type == I_SLICE, pcs->temporal_layer_index);

    // Y Residual
    if (!is_inter) {
        svt_aom_residual_kernel(input_pic->buffer_y,
                                input_txb_origin_index,
                                input_pic->stride_y << ctx->mds_subres_step,
                                cand_bf->pred->buffer_y,
                                txb_origin_index,
                                cand_bf->pred->stride_y << ctx->mds_subres_step,
                                (int16_t*)cand_bf->residual->buffer_y,
                                txb_origin_index,
                                cand_bf->residual->stride_y,
                                ctx->hbd_md,
                                tx_width,
                                tx_height >> ctx->mds_subres_step);
    }

    // TX search

    const int seg_qp = pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled
        ? pcs->ppcs->frm_hdr.segmentation_params.feature_data[ctx->blk_ptr->segment_id][SEG_LVL_ALT_Q]
        : 0;

    if (ctx->mds_subres_step == 2) {
        if (tx_size == TX_64X64) {
            tx_size = TX_64X16;
        } else if (tx_size == TX_32X32) {
            tx_size = TX_32X8;
        } else if (tx_size == TX_16X16) {
            tx_size = TX_16X4;
        } else {
            assert(0);
        }
    } else if (ctx->mds_subres_step == 1) {
        if (tx_size == TX_64X64) {
            tx_size = TX_64X32;
        } else if (tx_size == TX_32X32) {
            tx_size = TX_32X16;
        } else if (tx_size == TX_16X16) {
            tx_size = TX_16X8;
        } else if (tx_size == TX_8X8) {
            tx_size = TX_8X4;
        } else {
            assert(0);
        }
    }
    assert(tx_size < TX_SIZES_ALL);
    EB_TRANS_COEFF_SHAPE pf_shape = ctx->pf_ctrls.pf_shape;
    if (ctx->md_stage == MD_STAGE_3 && ctx->use_tx_shortcuts_mds3 && ctx->rtc_use_N4_dct_dct_shortcut) {
        pf_shape = N4_SHAPE;
    }
    // only have prev. stage coeff info if mds1/2 were performed
    else if (ctx->tx_shortcut_ctrls.apply_pf_on_coeffs && ctx->md_stage == MD_STAGE_3 && ctx->perform_mds1) {
        uint8_t use_pfn4_cond = 0;

        const uint16_t th = ((tx_width >> 4) * (tx_height >> 4));
        use_pfn4_cond     = (cand_bf->cnt_nz_coeff < th) || !cand_bf->block_has_coeff ? 1 : 0;
        if (use_pfn4_cond) {
            pf_shape = N4_SHAPE;
        }
    }
    ctx->luma_txb_skip_context = 0;
    ctx->luma_dc_sign_context  = 0;
    if (ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx) {
        // Arrays updated here only necessary if DC sign context update is enabled,
        // or for intra_luma_preiction when TXS is on. TXS is assumed off in this path
        // so only update if DC sign array is needed
        tx_initialize_neighbor_arrays(pcs, ctx, is_inter);
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_LUMA,
                            ctx->full_loop_luma_dc_sign_level_coeff_na,
                            ctx->blk_org_x + tx_org_x,
                            ctx->blk_org_y + tx_org_y,
                            ctx->blk_geom->bsize,
                            tx_size,
                            &ctx->luma_txb_skip_context,
                            &ctx->luma_dc_sign_context);
    }

    EbPictureBufferDesc* const recon_coeff_ptr = cand_bf->rec_coeff;
    EbPictureBufferDesc* const recon_ptr       = cand_bf->recon;
    EbPictureBufferDesc* const quant_coeff_ptr = cand_bf->quant;

    if (!tx_search_skip_flag) {
        // Y: T Q i_q
        svt_aom_estimate_transform(pcs,
                                   ctx,
                                   &(((int16_t*)cand_bf->residual->buffer_y)[txb_origin_index]),
                                   cand_bf->residual->stride_y,
                                   &(((int32_t*)ctx->tx_coeffs->buffer_y)[txb_1d_offset]),
                                   NOT_USED_VALUE,
                                   tx_size,
                                   &ctx->three_quad_energy,
                                   ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                   tx_type,
                                   PLANE_TYPE_Y,
                                   pf_shape);
        cand_bf->quant_dc.y[txb_itr] = svt_aom_quantize_inv_quantize(
            pcs,
            ctx,
            &(((int32_t*)ctx->tx_coeffs->buffer_y)[txb_1d_offset]),
            &(((int32_t*)quant_coeff_ptr->buffer_y)[txb_1d_offset]),
            &(((int32_t*)recon_coeff_ptr->buffer_y)[txb_1d_offset]),
            qindex,
            seg_qp,
            tx_size,
            &(cand_bf->eob.y[txb_itr]),
            COMPONENT_LUMA,
            ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
            tx_type,
            ctx->luma_txb_skip_context,
            ctx->luma_dc_sign_context,
            cand_bf->cand->block_mi.mode,
            full_lambda,
            false);
    } else {
        // Init params
        cand_bf->quant_dc.y[txb_itr] = 0;
        cand_bf->eob.y[txb_itr]      = 0;
    }

    // Perform inverse TX if using spatial SSE (assumes TX depth is 0 b/c TXS is assumed off)
    if (ctx->mds_do_spatial_sse) {
        const SsimLevel ssim_level = ctx->tune_ssim_level;
        assert(IMPLIES(ssim_level > SSIM_LVL_0, ctx->pd_pass == PD_PASS_1));
        assert(IMPLIES(ssim_level > SSIM_LVL_0, ctx->md_stage == MD_STAGE_3));
        if (cand_bf->eob.y[txb_itr]) {
            svt_aom_inv_transform_recon_wrapper(pcs,
                                                ctx,
                                                cand_bf->pred->buffer_y,
                                                txb_origin_index,
                                                cand_bf->pred->stride_y,
                                                recon_ptr->buffer_y,
                                                txb_origin_index,
                                                cand_bf->recon->stride_y,
                                                (int32_t*)recon_coeff_ptr->buffer_y,
                                                txb_1d_offset,
                                                ctx->hbd_md,
                                                tx_depth_to_tx_size[tx_depth][ctx->blk_geom->bsize],
                                                tx_type,
                                                PLANE_TYPE_Y,
                                                (uint32_t)cand_bf->eob.y[txb_itr]);
        } else {
            svt_av1_picture_copy_y(
                cand_bf->pred, txb_origin_index, recon_ptr, txb_origin_index, tx_width, tx_height, ctx->hbd_md);
        }

        const int32_t cropped_tx_width = MIN((uint8_t)tx_width, pcs->ppcs->aligned_width - (ctx->blk_org_x + tx_org_x));
        const int32_t cropped_tx_height                  = MIN((uint8_t)(tx_height >> ctx->mds_subres_step),
                                              pcs->ppcs->aligned_height - (ctx->blk_org_y + tx_org_y));
        EbSpatialFullDistType spatial_full_dist_type_fun = ctx->hbd_md ? svt_full_distortion_kernel16_bits
                                                                       : svt_spatial_full_distortion_kernel;
        if (ssim_level == SSIM_LVL_1 || ssim_level == SSIM_LVL_3) {
            y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] = svt_spatial_full_distortion_ssim_kernel(
                input_pic->buffer_y,
                input_txb_origin_index,
                input_pic->stride_y,
                cand_bf->pred->buffer_y,
                (int32_t)txb_origin_index,
                cand_bf->pred->stride_y,
                cropped_tx_width,
                cropped_tx_height,
                ctx->hbd_md,
                effective_ac_bias);
            y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL] = svt_spatial_full_distortion_ssim_kernel(
                input_pic->buffer_y,
                input_txb_origin_index,
                input_pic->stride_y,
                recon_ptr->buffer_y,
                (int32_t)txb_origin_index,
                cand_bf->recon->stride_y,
                cropped_tx_width,
                cropped_tx_height,
                ctx->hbd_md,
                effective_ac_bias);
            y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] <<= 4;
            y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL] <<= 4;
        }
        y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = spatial_full_dist_type_fun(input_pic->buffer_y,
                                                                                       input_txb_origin_index,
                                                                                       input_pic->stride_y,
                                                                                       cand_bf->pred->buffer_y,
                                                                                       (int32_t)txb_origin_index,
                                                                                       cand_bf->pred->stride_y,
                                                                                       cropped_tx_width,
                                                                                       cropped_tx_height);
        if (effective_ac_bias) {
            y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] += get_svt_psy_full_dist(input_pic->buffer_y,
                                                                                       input_txb_origin_index,
                                                                                       input_pic->stride_y,
                                                                                       cand_bf->pred->buffer_y,
                                                                                       (int32_t)txb_origin_index,
                                                                                       cand_bf->pred->stride_y,
                                                                                       cropped_tx_width,
                                                                                       cropped_tx_height,
                                                                                       ctx->hbd_md,
                                                                                       effective_ac_bias);
        }

        y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] = spatial_full_dist_type_fun(input_pic->buffer_y,
                                                                                     input_txb_origin_index,
                                                                                     input_pic->stride_y,
                                                                                     recon_ptr->buffer_y,
                                                                                     (int32_t)txb_origin_index,
                                                                                     cand_bf->recon->stride_y,
                                                                                     cropped_tx_width,
                                                                                     cropped_tx_height);
        if (effective_ac_bias) {
            y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] += get_svt_psy_full_dist(input_pic->buffer_y,
                                                                                     input_txb_origin_index,
                                                                                     input_pic->stride_y,
                                                                                     recon_ptr->buffer_y,
                                                                                     (int32_t)txb_origin_index,
                                                                                     cand_bf->recon->stride_y,
                                                                                     cropped_tx_width,
                                                                                     cropped_tx_height,
                                                                                     ctx->hbd_md,
                                                                                     effective_ac_bias);
        }
        y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] <<= 4;
        y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] <<= 4;
    } else {
        // LUMA DISTORTION
        uint32_t txbwidth  = tx_width;
        uint32_t txbheight = (tx_height >> ctx->mds_subres_step);

        uint32_t bwidth, bheight;
        if (pf_shape && !tx_search_skip_flag) {
            bwidth  = MAX((txbwidth >> pf_shape), 4);
            bheight = (txbheight >> pf_shape);
        } else {
            bwidth  = txbwidth < 64 ? txbwidth : 32;
            bheight = txbheight < 64 ? txbheight : 32;
        }

        svt_aom_picture_full_distortion32_bits_single(&(((int32_t*)ctx->tx_coeffs->buffer_y)[txb_1d_offset]),
                                                      &(((int32_t*)recon_coeff_ptr->buffer_y)[txb_1d_offset]),
                                                      txbwidth < 64 ? txbwidth : 32,
                                                      bwidth,
                                                      bheight,
                                                      y_full_distortion[DIST_SSD],
                                                      cand_bf->eob.y[txb_itr]);
        y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] += ctx->three_quad_energy;
        y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] += ctx->three_quad_energy;

        const int32_t shift                             = (MAX_TX_SCALE - av1_get_tx_scale_tab[tx_size]) * 2;
        y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] = RIGHT_SIGNED_SHIFT(
            y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL], shift);
        y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = RIGHT_SIGNED_SHIFT(
            y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION], shift);
    }
    y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] <<= ctx->mds_subres_step;
    y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] <<= ctx->mds_subres_step;

    y_full_distortion[DIST_SSIM][DIST_CALC_RESIDUAL] <<= ctx->mds_subres_step;
    y_full_distortion[DIST_SSIM][DIST_CALC_PREDICTION] <<= ctx->mds_subres_step;

    //LUMA-ONLY
    const uint32_t th = ((tx_width * tx_height) >> 6);
    if ((ctx->rate_est_ctrls.coeff_rate_est_lvl >= 2 || ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) &&
        (cand_bf->eob.y[txb_itr] < th)) {
        *y_coeff_bits = 6000 + cand_bf->eob.y[txb_itr] * 1000;
    } else if (ctx->rate_est_ctrls.coeff_rate_est_lvl == 0) {
        *y_coeff_bits = 6000 + cand_bf->eob.y[txb_itr] * 400;
    } else {
        svt_aom_txb_estimate_coeff_bits(ctx,
                                        0, //allow_update_cdf,
                                        NULL, //FRAME_CONTEXT *ec_ctx,
                                        pcs,
                                        cand_bf,
                                        txb_1d_offset,
                                        0,
                                        quant_coeff_ptr,
                                        cand_bf->eob.y[txb_itr],
                                        0,
                                        0,
                                        y_coeff_bits,
                                        NULL,
                                        NULL,
                                        tx_size,
                                        NOT_USED_VALUE,
                                        tx_type,
                                        NOT_USED_VALUE,
                                        COMPONENT_LUMA);
    }

    // Update with best_tx_type data
    cand_bf->cand->transform_type[txb_itr] = tx_type;
    cand_bf->y_has_coeff                   = ((cand_bf->eob.y[txb_itr] > 0) << txb_itr);
    // For Inter blocks, transform type of chroma follows luma transfrom type
    if (is_inter) {
        cand_bf->cand->transform_type_uv = cand_bf->cand->transform_type[txb_itr];
    }
}

// Compare even/odd lines' SADs to determine if using a subsampled residual is safe
static void check_is_subres_safe(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                 EbPictureBufferDesc* input_pic, uint32_t input_origin_index,
                                 uint32_t blk_origin_index) {
    uint32_t sad_even, sad_odd;
    if (!ctx->hbd_md) {
        sad_even = svt_nxm_sad_kernel(input_pic->buffer_y + input_origin_index,
                                      input_pic->stride_y << 1,
                                      cand_bf->pred->buffer_y + blk_origin_index,
                                      cand_bf->pred->stride_y << 1,
                                      ctx->blk_geom->bheight >> 1,
                                      ctx->blk_geom->bwidth);

        sad_odd = svt_nxm_sad_kernel(input_pic->buffer_y + input_origin_index + input_pic->stride_y,
                                     input_pic->stride_y << 1,
                                     cand_bf->pred->buffer_y + blk_origin_index + cand_bf->pred->stride_y,
                                     cand_bf->pred->stride_y << 1,
                                     ctx->blk_geom->bheight >> 1,
                                     ctx->blk_geom->bwidth);

    } else {
        sad_even = sad_16b_kernel(((uint16_t*)input_pic->buffer_y) + input_origin_index,
                                  input_pic->stride_y << 1,
                                  ((uint16_t*)cand_bf->pred->buffer_y) + blk_origin_index,
                                  cand_bf->pred->stride_y << 1,
                                  ctx->blk_geom->bheight >> 1,
                                  ctx->blk_geom->bwidth);

        sad_odd = sad_16b_kernel(((uint16_t*)input_pic->buffer_y) + input_origin_index + input_pic->stride_y,
                                 input_pic->stride_y << 1,
                                 ((uint16_t*)cand_bf->pred->buffer_y) + blk_origin_index + cand_bf->pred->stride_y,
                                 cand_bf->pred->stride_y << 1,
                                 ctx->blk_geom->bheight >> 1,
                                 ctx->blk_geom->bwidth);
    }

    int deviation = (int)(((int)MAX(sad_even, 1) - (int)MAX(sad_odd, 1)) * 100) / (int)MAX(sad_odd, 1);
    if (ABS(deviation) <= ctx->subres_ctrls.odd_to_even_deviation_th) {
        ctx->is_subres_safe = 1;
    } else {
        ctx->is_subres_safe = 0;
    }
}

static void full_loop_core_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                     ModeDecisionCandidateBuffer* cand_bf, EbPictureBufferDesc* input_pic,
                                     uint32_t input_origin_index, uint32_t blk_origin_index) {
    uint64_t y_full_distortion[DIST_CALC_TOTAL];
    uint64_t y_coeff_bits;
    uint32_t full_lambda = ctx->full_sb_lambda_md[EB_8_BIT_MD];
    if (ctx->subres_ctrls.odd_to_even_deviation_th && ctx->pd_pass == PD_PASS_0 && ctx->md_stage == MD_STAGE_3 &&
        ctx->is_subres_safe == (uint8_t)~0 /* only if invalid*/ && ctx->blk_geom->bheight == 64 &&
        ctx->blk_geom->bwidth == 64) {
        check_is_subres_safe(ctx, cand_bf, input_pic, input_origin_index, blk_origin_index);
    }
    if (ctx->is_subres_safe != 1) {
        ctx->mds_subres_step = 0;
    }

    // If using 4x subsampling, can't have 8x8 b/c no 8x2 transform
    // If using 2x subsampling, can't have 4x4 b/c no 4x2 transform
    // subres tx assumes NSQ is off
    assert(IMPLIES(ctx->mds_subres_step == 2, ctx->blk_geom->sq_size >= 16));
    assert(IMPLIES(ctx->mds_subres_step == 1, ctx->blk_geom->sq_size >= 8));

    //Y Residual
    svt_aom_residual_kernel(input_pic->buffer_y,
                            input_origin_index,
                            input_pic->stride_y << ctx->mds_subres_step,
                            cand_bf->pred->buffer_y,
                            blk_origin_index,
                            cand_bf->pred->stride_y << ctx->mds_subres_step,
                            (int16_t*)cand_bf->residual->buffer_y,
                            blk_origin_index,
                            cand_bf->residual->stride_y,
                            0,
                            ctx->blk_geom->bwidth,
                            ctx->blk_geom->bheight >> ctx->mds_subres_step);

    perform_tx_light_pd0(pcs, ctx, cand_bf, ctx->blk_ptr->qindex, &y_coeff_bits, &y_full_distortion[0]);
    cand_bf->cnt_nz_coeff = cand_bf->eob.y[0];
    svt_aom_full_cost_light_pd0(ctx, cand_bf, y_full_distortion, full_lambda, &y_coeff_bits);
}

extern const uint8_t  svt_aom_eb_av1_var_offs[MAX_SB_SIZE];
static const uint16_t eb_av1_var_offs_hbd[MAX_SB_SIZE] = {
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512,
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512,
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512,
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512,
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512,
    512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512, 512};

// Detect blocks that whose chroma component is important (used as a detector for chroma TX shortcuts in reg. PD1 and LPD1)
// Update ctx->chroma_complexity accordingly
void chroma_complexity_check_pred(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_buffer,
                                  EbPictureBufferDesc* input_pic, BlockLocation* loc, uint8_t use_var) {
    if (ctx->chroma_complexity == COMPONENT_CHROMA) {
        return;
    }

    uint32_t y_dist = 0, cb_dist = 0, cr_dist = 0;
    uint8_t  shift = 0;
    shift          = ctx->blk_geom->bheight_uv > 8 ? 2 : ctx->blk_geom->bheight_uv > 4 ? 1 : 0; // no shift for 4x4

    if (!ctx->hbd_md) {
        y_dist = svt_nxm_sad_kernel(input_pic->buffer_y + loc->input_origin_index,
                                    input_pic->stride_y << shift,
                                    cand_buffer->pred->buffer_y,
                                    cand_buffer->pred->stride_y << shift,
                                    ctx->blk_geom->bheight_uv >> shift,
                                    ctx->blk_geom->bwidth_uv);
        // Only need to check Cb component if not already identified as complex
        if (ctx->chroma_complexity == COMPONENT_LUMA || ctx->chroma_complexity == COMPONENT_CHROMA_CR) {
            cb_dist = svt_nxm_sad_kernel(input_pic->buffer_cb + loc->input_cb_origin_in_index,
                                         input_pic->stride_cb << shift,
                                         cand_buffer->pred->buffer_cb,
                                         cand_buffer->pred->stride_cb << shift,
                                         ctx->blk_geom->bheight_uv >> shift,
                                         ctx->blk_geom->bwidth_uv);
        }
        // Only need to check Cr component if not already identified as complex
        if (ctx->chroma_complexity == COMPONENT_LUMA || ctx->chroma_complexity == COMPONENT_CHROMA_CB) {
            cr_dist = svt_nxm_sad_kernel(input_pic->buffer_cr + loc->input_cb_origin_in_index,
                                         input_pic->stride_cr << shift,
                                         cand_buffer->pred->buffer_cr,
                                         cand_buffer->pred->stride_cr << shift,
                                         ctx->blk_geom->bheight_uv >> shift,
                                         ctx->blk_geom->bwidth_uv);
        }

    } else {
        y_dist = sad_16b_kernel(((uint16_t*)input_pic->buffer_y) + loc->input_origin_index,
                                input_pic->stride_y << shift,
                                (uint16_t*)cand_buffer->pred->buffer_y,
                                cand_buffer->pred->stride_y << shift,
                                ctx->blk_geom->bheight_uv >> shift,
                                ctx->blk_geom->bwidth_uv);
        // Only need to check Cb component if not already identified as complex
        if (ctx->chroma_complexity == COMPONENT_LUMA || ctx->chroma_complexity == COMPONENT_CHROMA_CR) {
            cb_dist = sad_16b_kernel(((uint16_t*)input_pic->buffer_cb) + loc->input_cb_origin_in_index,
                                     input_pic->stride_cb << shift,
                                     (uint16_t*)cand_buffer->pred->buffer_cb,
                                     cand_buffer->pred->stride_cb << shift,
                                     ctx->blk_geom->bheight_uv >> shift,
                                     ctx->blk_geom->bwidth_uv);
        }
        // Only need to check Cr component if not already identified as complex
        if (ctx->chroma_complexity == COMPONENT_LUMA || ctx->chroma_complexity == COMPONENT_CHROMA_CB) {
            cr_dist = sad_16b_kernel(((uint16_t*)input_pic->buffer_cr) + loc->input_cb_origin_in_index,
                                     input_pic->stride_cr << shift,
                                     (uint16_t*)cand_buffer->pred->buffer_cr,
                                     cand_buffer->pred->stride_cr << shift,
                                     ctx->blk_geom->bheight_uv >> shift,
                                     ctx->blk_geom->bwidth_uv);
        }
    }
    y_dist <<= 1;

    if (cb_dist > y_dist && cr_dist > y_dist) {
        ctx->chroma_complexity = COMPONENT_CHROMA;
    } else if (cb_dist > y_dist) {
        ctx->chroma_complexity = (ctx->chroma_complexity == COMPONENT_CHROMA_CR) ? COMPONENT_CHROMA
                                                                                 : COMPONENT_CHROMA_CB;
    } else if (cr_dist > y_dist) {
        ctx->chroma_complexity = (ctx->chroma_complexity == COMPONENT_CHROMA_CB) ? COMPONENT_CHROMA
                                                                                 : COMPONENT_CHROMA_CR;
    }

    if (cb_dist > y_dist || cr_dist > y_dist) {
        ctx->cfl_complexity = COMPONENT_CHROMA;
    }

    if (use_var) {
        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize_uv];
        unsigned int            sse;
        unsigned int            var_cb;
        unsigned int            var_cr;
        if (ctx->hbd_md) {
            var_cb = fn_ptr->vf_hbd_10(
                CONVERT_TO_BYTEPTR(((uint16_t*)input_pic->buffer_cb) + loc->input_cb_origin_in_index),
                input_pic->stride_cb,
                CONVERT_TO_BYTEPTR(eb_av1_var_offs_hbd),
                0,
                &sse);
            var_cr = fn_ptr->vf_hbd_10(
                CONVERT_TO_BYTEPTR(((uint16_t*)input_pic->buffer_cr) + loc->input_cb_origin_in_index),
                input_pic->stride_cr,
                CONVERT_TO_BYTEPTR(eb_av1_var_offs_hbd),
                0,
                &sse);
        } else {
            var_cb = fn_ptr->vf(input_pic->buffer_cb + loc->input_cb_origin_in_index,
                                input_pic->stride_cb,
                                svt_aom_eb_av1_var_offs,
                                0,
                                &sse);
            var_cr = fn_ptr->vf(input_pic->buffer_cr + loc->input_cb_origin_in_index,
                                input_pic->stride_cr,
                                svt_aom_eb_av1_var_offs,
                                0,
                                &sse);
        }

        int block_var_cb = ROUND_POWER_OF_TWO(var_cb, eb_num_pels_log2_lookup[ctx->blk_geom->bsize_uv]);
        int block_var_cr = ROUND_POWER_OF_TWO(var_cr, eb_num_pels_log2_lookup[ctx->blk_geom->bsize_uv]);

        // th controls how safe the detector is (can be changed in the future, or made a parameter)
        uint16_t th = 150;
        if (block_var_cb > th && block_var_cr > th) {
            ctx->chroma_complexity = COMPONENT_CHROMA;
        } else if (block_var_cb > th) {
            ctx->chroma_complexity = (ctx->chroma_complexity == COMPONENT_CHROMA_CR) ? COMPONENT_CHROMA
                                                                                     : COMPONENT_CHROMA_CB;
        } else if (block_var_cr > th) {
            ctx->chroma_complexity = (ctx->chroma_complexity == COMPONENT_CHROMA_CB) ? COMPONENT_CHROMA
                                                                                     : COMPONENT_CHROMA_CR;
        }
        if (block_var_cb > ctx->cfl_ctrls.cplx_th || block_var_cr > ctx->cfl_ctrls.cplx_th) {
            ctx->cfl_complexity = COMPONENT_CHROMA;
        }
    }
}

// Detect blocks that whose chroma component is important (used as a detector for skipping the chroma TX path in LPD1)
static COMPONENT_TYPE chroma_complexity_check(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                              ModeDecisionCandidate* cand, EbPictureBufferDesc* input_pic,
                                              BlockLocation* loc) {
    /* For INTER blocks, compute the luma/chroma full-pel distortions; if chroma distortion is much higher, then block is complex
    in chroma, and chroma should be performed. */
    if (is_inter_mode(cand->block_mi.mode)) {
        MvReferenceFrame rf[2]         = {cand->block_mi.ref_frame[0], cand->block_mi.ref_frame[1]};
        const int8_t     ref_idx_first = get_ref_frame_idx(rf[0]);
        const uint8_t    list_idx      = get_list_idx(rf[0]);
        // use first ref frame only if have bipred candidate
        EbReferenceObject*   ref_obj = (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx][ref_idx_first]->object_ptr;
        EbPictureBufferDesc* ref_pic = svt_aom_get_ref_pic_buffer(pcs, rf[0]);
        int16_t              mv_x    = cand->block_mi.mv[0].x >> 3;
        int16_t              mv_y    = cand->block_mi.mv[0].y >> 3;
        // -------
        // Use scaled references if resolution of the reference is different from that of the input
        // -------
        svt_aom_use_scaled_rec_refs_if_needed(pcs, input_pic, ref_obj, &ref_pic, ctx->hbd_md);

        uint32_t src_y_offset = ref_pic->org_x + ctx->blk_org_x + mv_x +
            (ref_pic->org_y + ctx->blk_org_y + mv_y) * ref_pic->stride_y;
        uint32_t src_cb_offset = ((ref_pic->org_x + ctx->blk_org_x + mv_x) >> 1) +
            (((ref_pic->org_y + ctx->blk_org_y + mv_y) >> 1)) * ref_pic->stride_cb;
        uint32_t src_cr_offset = ((ref_pic->org_x + ctx->blk_org_x + mv_x) >> 1) +
            (((ref_pic->org_y + ctx->blk_org_y + mv_y) >> 1)) * ref_pic->stride_cr;
        uint8_t shift = 0;
        if (ctx->lpd1_tx_ctrls.chroma_detector_level >= 2) {
            shift = ctx->blk_geom->bheight_uv > 8 ? 2 : ctx->blk_geom->bheight_uv > 4 ? 1 : 0; // no shift for 4x4
        } else {
            shift = ctx->blk_geom->bheight_uv > 4 ? 1 : 0; // no shift for 4x4
        }

        uint32_t y_dist, cb_dist, cr_dist;

        if (ctx->hbd_md) {
            uint16_t* src_10b;
            DECLARE_ALIGNED(16, uint16_t, packed_buf[PACKED_BUFFER_SIZE]);
            // pack the reference into temp 16bit buffer
            uint8_t offset = 0;
            int32_t stride;

            svt_aom_pack_block(
                ref_pic->buffer_y + src_y_offset - offset - (offset * (ref_pic->stride_y << shift)),
                ref_pic->stride_y << shift,
                ref_pic->buffer_bit_inc_y + src_y_offset - offset - (offset * (ref_pic->stride_bit_inc_y << shift)),
                ref_pic->stride_bit_inc_y << shift,
                (uint16_t*)packed_buf,
                MAX_SB_SIZE,
                ctx->blk_geom->bwidth_uv + (offset << 1),
                (ctx->blk_geom->bheight_uv >> shift) + (offset << 1));

            src_10b = (uint16_t*)packed_buf + offset + (offset * MAX_SB_SIZE);
            stride  = MAX_SB_SIZE;

            // Y dist only computed over UV size so SADs are comparable
            y_dist = sad_16b_kernel(((uint16_t*)input_pic->buffer_y) + loc->input_origin_index,
                                    input_pic->stride_y << shift,
                                    src_10b,
                                    stride,
                                    ctx->blk_geom->bheight_uv >> shift,
                                    ctx->blk_geom->bwidth_uv);

            // pack the reference into temp 16bit buffer

            svt_aom_pack_block(
                ref_pic->buffer_cb + src_cb_offset - offset - (offset * (ref_pic->stride_cb << shift)),
                ref_pic->stride_cb << shift,
                ref_pic->buffer_bit_inc_cb + src_cb_offset - offset - (offset * (ref_pic->stride_bit_inc_cb << shift)),
                ref_pic->stride_bit_inc_cb << shift,
                (uint16_t*)packed_buf,
                MAX_SB_SIZE,
                ctx->blk_geom->bwidth_uv + (offset << 1),
                (ctx->blk_geom->bheight_uv >> shift) + (offset << 1));

            src_10b = (uint16_t*)packed_buf + offset + (offset * MAX_SB_SIZE);
            stride  = MAX_SB_SIZE;

            cb_dist = sad_16b_kernel(((uint16_t*)input_pic->buffer_cb) + loc->input_cb_origin_in_index,
                                     input_pic->stride_cb << shift,
                                     src_10b,
                                     stride,
                                     ctx->blk_geom->bheight_uv >> shift,
                                     ctx->blk_geom->bwidth_uv);

            // pack the reference into temp 16bit buffer
            svt_aom_pack_block(
                ref_pic->buffer_cr + src_cr_offset - offset - (offset * (ref_pic->stride_cr << shift)),
                ref_pic->stride_cr << shift,
                ref_pic->buffer_bit_inc_cr + src_cr_offset - offset - (offset * (ref_pic->stride_bit_inc_cr << shift)),
                ref_pic->stride_bit_inc_cr << shift,
                (uint16_t*)packed_buf,
                MAX_SB_SIZE,
                ctx->blk_geom->bwidth_uv + (offset << 1),
                (ctx->blk_geom->bheight_uv >> shift) + (offset << 1));

            src_10b = (uint16_t*)packed_buf + offset + (offset * MAX_SB_SIZE);
            stride  = MAX_SB_SIZE;

            cr_dist = sad_16b_kernel(((uint16_t*)input_pic->buffer_cr) + loc->input_cb_origin_in_index,
                                     input_pic->stride_cr << shift,
                                     src_10b,
                                     stride,
                                     ctx->blk_geom->bheight_uv >> shift,
                                     ctx->blk_geom->bwidth_uv);
        } else {
            // Y dist only computed over UV size so SADs are comparable
            y_dist = svt_nxm_sad_kernel(input_pic->buffer_y + loc->input_origin_index,
                                        input_pic->stride_y << shift,
                                        ref_pic->buffer_y + src_y_offset,
                                        ref_pic->stride_y << shift,
                                        ctx->blk_geom->bheight_uv >> shift,
                                        ctx->blk_geom->bwidth_uv);

            cb_dist = svt_nxm_sad_kernel(input_pic->buffer_cb + loc->input_cb_origin_in_index,
                                         input_pic->stride_cb << shift,
                                         ref_pic->buffer_cb + src_cb_offset,
                                         ref_pic->stride_cb << shift,
                                         ctx->blk_geom->bheight_uv >> shift,
                                         ctx->blk_geom->bwidth_uv);

            cr_dist = svt_nxm_sad_kernel(input_pic->buffer_cr + loc->input_cb_origin_in_index,
                                         input_pic->stride_cr << shift,
                                         ref_pic->buffer_cr + src_cr_offset,
                                         ref_pic->stride_cr << shift,
                                         ctx->blk_geom->bheight_uv >> shift,
                                         ctx->blk_geom->bwidth_uv);
        }
        // shift y_dist by to ensure chroma is much higher than luma
        if (ctx->lpd1_tx_ctrls.chroma_detector_level >= 2) {
            y_dist <<= 2;
        } else {
            y_dist <<= 1;
        }

        if (cb_dist > y_dist && cr_dist > y_dist) {
            return COMPONENT_CHROMA;
        } else if (cb_dist > y_dist) {
            return COMPONENT_CHROMA_CB;
        } else if (cr_dist > y_dist) {
            return COMPONENT_CHROMA_CR;
        }
    }

    /* For INTRA blocks, if the chroma variance of the block is high, perform chroma. Can also use variance check as an additional
    check for INTER blocks. */
    if (is_intra_mode(cand->block_mi.mode) || ctx->lpd1_tx_ctrls.chroma_detector_level <= 2) {
        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize_uv];
        unsigned int            sse;
        unsigned int            var_cb;
        unsigned int            var_cr;
        if (ctx->hbd_md) {
            var_cb = fn_ptr->vf_hbd_10(
                CONVERT_TO_BYTEPTR(((uint16_t*)input_pic->buffer_cb) + loc->input_cb_origin_in_index),
                input_pic->stride_cb,
                CONVERT_TO_BYTEPTR(eb_av1_var_offs_hbd),
                0,
                &sse);
            var_cr = fn_ptr->vf_hbd_10(
                CONVERT_TO_BYTEPTR(((uint16_t*)input_pic->buffer_cr) + loc->input_cb_origin_in_index),
                input_pic->stride_cr,
                CONVERT_TO_BYTEPTR(eb_av1_var_offs_hbd),
                0,
                &sse);
        } else {
            var_cb = fn_ptr->vf(input_pic->buffer_cb + loc->input_cb_origin_in_index,
                                input_pic->stride_cb,
                                svt_aom_eb_av1_var_offs,
                                0,
                                &sse);
            var_cr = fn_ptr->vf(input_pic->buffer_cr + loc->input_cb_origin_in_index,
                                input_pic->stride_cr,
                                svt_aom_eb_av1_var_offs,
                                0,
                                &sse);
        }
        int block_var_cb = ROUND_POWER_OF_TWO(var_cb, eb_num_pels_log2_lookup[ctx->blk_geom->bsize_uv]);
        int block_var_cr = ROUND_POWER_OF_TWO(var_cr, eb_num_pels_log2_lookup[ctx->blk_geom->bsize_uv]);

        // th controls how safe the detector is
        uint16_t th = ctx->lpd1_tx_ctrls.chroma_detector_level <= 1 ? 75 : 150;
        if (block_var_cb > th && block_var_cr > th) {
            return COMPONENT_CHROMA;
        } else if (block_var_cb > th) {
            return COMPONENT_CHROMA_CB;
        } else if (block_var_cr > th) {
            return COMPONENT_CHROMA_CR;
        }
    }

    // At end, complex chroma was not detected, so only chroma path can be skipped
    return COMPONENT_LUMA;
}

static bool get_perform_tx_flag(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                ModeDecisionCandidate* cand) {
    bool perform_tx = 1;
    if (ctx->lpd1_allow_skipping_tx) {
        if (ctx->lpd1_skip_inter_tx_level == 2 && is_inter_mode(cand->block_mi.mode)) {
            perform_tx = 0;
        }

        MacroBlockD* xd = ctx->blk_ptr->av1xd;
        if (xd->left_available && xd->up_available) {
            const BlockModeInfo* const left_mi  = &xd->left_mbmi->block_mi;
            const BlockModeInfo* const above_mi = &xd->above_mbmi->block_mi;
            if (left_mi->skip && above_mi->skip &&
                ((left_mi->mode == NEAREST_NEARESTMV && above_mi->mode == NEAREST_NEARESTMV) ||
                 ctx->lpd1_skip_inter_tx_level)) {
                /* For M12 and below, do not skip TX for candidates other than NRST_NRST and do not remove the check on neighbouring
                    coeffs, as that may introduce blocking artifacts in certain clips. */

                // Skip TX for NRST_NRST
                if (ctx->lpd1_tx_ctrls.skip_nrst_nrst_luma_tx && cand->block_mi.mode == NEAREST_NEARESTMV) {
                    perform_tx = 0;
                }
                // Skip TX for INTER - should only be true for M13
                else if (ctx->lpd1_skip_inter_tx_level == 1 && is_inter_mode(cand->block_mi.mode)) {
                    perform_tx = 0;
                }
            }
        }
    }

    if (!perform_tx) {
        return 0;
    }
    if (ctx->lpd1_bypass_tx_th) {
        // MDS0 always performed on 8bit, so use 8bit lambda with the MDS0 distortion
        const uint32_t full_lambda   = ctx->full_lambda_md[EB_8_BIT_MD];
        const uint64_t est_skip_cost = RDCOST(
            full_lambda,
            cand_bf->fast_luma_rate + ((uint64_t)ctx->md_rate_est_ctx->skip_fac_bits[ctx->skip_coeff_ctx][1]),
            cand_bf->full_dist << 4);
        const uint64_t th = RDCOST(full_lambda,
                                   cand_bf->fast_luma_rate +
                                       ((uint64_t)ctx->md_rate_est_ctx->skip_fac_bits[ctx->skip_coeff_ctx][0]) +
                                       INIT_BIT_EST,
                                   (ctx->blk_geom->bheight * ctx->blk_geom->bwidth) << 4);
        if (est_skip_cost * 100 < ctx->lpd1_bypass_tx_th * th) {
            perform_tx = 0;
        }
    }
    return perform_tx;
}

/*
   full loop core for light PD1 path
*/
static void full_loop_core_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                     ModeDecisionCandidateBuffer* cand_bf, EbPictureBufferDesc* input_pic,
                                     BlockLocation* loc) {
    ModeDecisionCandidate* cand                                            = cand_bf->cand;
    uint64_t               y_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL]  = {{0}};
    uint64_t               cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
    uint64_t               cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
    uint64_t               y_coeff_bits;
    uint64_t               cb_coeff_bits;
    uint64_t               cr_coeff_bits;
    cand->block_mi.skip_mode   = false;
    bool          perform_tx   = get_perform_tx_flag(ctx, cand_bf, cand);
    const uint8_t recon_needed = svt_aom_do_md_recon(pcs->ppcs, ctx);

    // If need 10bit prediction, perform luma compensation before TX
    if ((perform_tx || recon_needed) && ctx->hbd_md) {
        ctx->md_stage           = MD_STAGE_0;
        ctx->mds_do_chroma      = false;
        ctx->uv_intra_comp_only = false;
        product_prediction_fun_table_light_pd1[is_inter_mode(cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
        ctx->md_stage           = MD_STAGE_3;
        ctx->uv_intra_comp_only = true;
        ctx->mds_do_chroma      = true;
    }
    if (perform_tx) {
        perform_dct_dct_tx_light_pd1(pcs, ctx, cand_bf, loc, &y_coeff_bits, y_full_distortion[DIST_SSD]);
    } else {
        cand_bf->eob.y[0]                                 = 0;
        cand_bf->quant_dc.y[0]                            = 0;
        cand_bf->y_has_coeff                              = 0;
        y_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
        y_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;
        y_coeff_bits                                      = 6000;
        cand_bf->cand->transform_type[0]                  = DCT_DCT;
        // For Inter blocks, transform type of chroma follows luma transfrom type
        if (is_inter_mode(cand_bf->cand->block_mi.mode)) {
            cand_bf->cand->transform_type_uv = DCT_DCT;
        }
    }
    // Update coeff info based on luma TX so that chroma can take advantage of most accurate info
    cand_bf->block_has_coeff        = (cand_bf->y_has_coeff) ? 1 : 0;
    cand_bf->cnt_nz_coeff           = cand_bf->eob.y[0];
    uint8_t        perform_chroma   = cand_bf->block_has_coeff || !(ctx->lpd1_tx_ctrls.zero_y_coeff_exit);
    COMPONENT_TYPE chroma_component = COMPONENT_CHROMA;
    ctx->chroma_complexity          = COMPONENT_LUMA;

    // If going to skip chroma TX, detect if block is complex in chroma, and if so, force chroma to be performed.
    if (!perform_chroma) {
        if (ctx->lpd1_tx_ctrls.chroma_detector_level) {
            chroma_component = chroma_complexity_check(pcs, ctx, cand, input_pic, loc);

            if (ctx->lpd1_tx_ctrls.chroma_detector_level <= 3) {
                ctx->chroma_complexity = chroma_component;
            }
        } else {
            chroma_component = COMPONENT_LUMA;
        }

        perform_chroma = chroma_component > COMPONENT_LUMA;
        if (chroma_component == COMPONENT_CHROMA_CB || chroma_component == COMPONENT_LUMA) {
            cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
            cr_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;
            cr_coeff_bits                                      = 0;
            cand_bf->v_has_coeff                               = 0;
            cand_bf->eob.v[0]                                  = 0;
        }
        if (chroma_component == COMPONENT_CHROMA_CR || chroma_component == COMPONENT_LUMA) {
            cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL]   = 0;
            cb_full_distortion[DIST_SSD][DIST_CALC_PREDICTION] = 0;
            cb_coeff_bits                                      = 0;
            cand_bf->u_has_coeff                               = 0;
            cand_bf->eob.u[0]                                  = 0;
        }
    }
    ctx->lpd1_chroma_comp = recon_needed ? COMPONENT_CHROMA : chroma_component;
    // If no luma coeffs, may skip chroma TX and full cost calc (skip chroma compensation
    // if not needed for recon)
    if (perform_chroma) {
        // If using chroma pred samples in the next chroma complexity detector, need to generate pred samples for all components
        if (!recon_needed) {
            ctx->lpd1_chroma_comp = ctx->lpd1_tx_ctrls.chroma_detector_level <= 3 ? COMPONENT_CHROMA : chroma_component;
        }
        //Chroma Prediction
        product_prediction_fun_table_light_pd1[is_inter_mode(cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
        // Perform additional check to detect complex chroma blocks
        if (ctx->lpd1_tx_ctrls.chroma_detector_level && ctx->lpd1_tx_ctrls.chroma_detector_level <= 3 &&
            ctx->chroma_complexity != COMPONENT_CHROMA &&
            (ctx->use_tx_shortcuts_mds3 || ctx->lpd1_tx_ctrls.use_uv_shortcuts_on_y_coeffs)) {
            chroma_complexity_check_pred(ctx, cand_bf, input_pic, loc, 0 /*use_var*/);
        }

        //CHROMA
        svt_aom_full_loop_chroma_light_pd1(pcs,
                                           ctx,
                                           cand_bf,
                                           input_pic,
                                           loc->input_cb_origin_in_index,
                                           0,
                                           chroma_component,
                                           ctx->qp_index,
                                           cb_full_distortion[DIST_SSD],
                                           cr_full_distortion[DIST_SSD],
                                           &cb_coeff_bits,
                                           &cr_coeff_bits);
        cand_bf->block_has_coeff = (cand_bf->y_has_coeff || cand_bf->u_has_coeff || cand_bf->v_has_coeff) ? true
                                                                                                          : false;
        svt_aom_full_cost(pcs,
                          ctx,
                          cand_bf,
                          ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD],
                          y_full_distortion,
                          cb_full_distortion,
                          cr_full_distortion,
                          &y_coeff_bits,
                          &cb_coeff_bits,
                          &cr_coeff_bits);
    } else {
        // Only need chroma pred if generating recon
        if (ctx->lpd1_chroma_comp > COMPONENT_LUMA) {
            //Chroma Prediction
            product_prediction_fun_table_light_pd1[is_inter_mode(cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
        }
        cand_bf->u_has_coeff = cand_bf->v_has_coeff = 0;
        if (cand->skip_mode_allowed) {
            cand->block_mi.skip_mode = true;
        }
    }
}

// Derive the start and end TX depths based on block characteristics
static void get_start_end_tx_depth(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                   ModeDecisionCandidateBuffer* cand_bf, uint8_t* start_tx_depth,
                                   uint8_t* end_tx_depth) {
    ModeDecisionCandidate* cand      = cand_bf->cand;
    TxsControls*           txs_ctrls = &ctx->txs_ctrls;
    const BlockGeom* const blk_geom  = ctx->blk_geom;

    if (txs_ctrls->enabled == 0) {
        *start_tx_depth = *end_tx_depth = 0;
    } else if (!ctx->mds_do_txs) {
        *start_tx_depth = *end_tx_depth = cand->block_mi.tx_depth;
    } else {
        *start_tx_depth = 0;
        // end_tx_depth set to zero for blocks which go beyond the picture boundaries
        if ((ctx->blk_org_x + ctx->blk_geom->bwidth <= pcs->ppcs->aligned_width &&
             ctx->blk_org_y + ctx->blk_geom->bheight <= pcs->ppcs->aligned_height)) {
            *end_tx_depth = get_end_tx_depth(blk_geom->bsize);
        } else {
            *end_tx_depth = 0;
        }
    }

    if (ctx->perform_mds1 && ctx->md_stage == MD_STAGE_3 && ctx->tx_shortcut_ctrls.bypass_tx_th &&
        !cand_bf->block_has_coeff &&
        ((cand_bf->luma_fast_dist * ctx->tx_shortcut_ctrls.bypass_tx_th) <
         (uint32_t)(ctx->blk_geom->bheight * ctx->blk_geom->bwidth * ctx->qp_index))) {
        *start_tx_depth = 0;
        *end_tx_depth   = 0;
    }

    *end_tx_depth = MIN(
        *end_tx_depth,
        (is_intra_mode(cand->block_mi.mode)
             ? (ctx->shape == PART_N ? txs_ctrls->intra_class_max_depth_sq : txs_ctrls->intra_class_max_depth_nsq)
             : (ctx->shape == PART_N ? txs_ctrls->inter_class_max_depth_sq : txs_ctrls->inter_class_max_depth_nsq)));
    // Force the use of TX_4X4 for 8x8 block(s)
    if (pcs->mimic_only_tx_4x4 && ctx->blk_geom->sq_size == 8) {
        *start_tx_depth = *end_tx_depth = 1;
    }
}

// Update the MV-diff signaling fast rate. Intended to be called when a unipred MV is changed after a refinement
// stage (e.g. WM/OBMC refinement).
static INLINE void update_refined_mv_fast_rate(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                               ModeDecisionCandidate* cand, Mv default_mv, Mv default_ref_mv) {
    const int32_t default_mv_rate = svt_av1_mv_bit_cost(&default_mv,
                                                        &default_ref_mv,
                                                        ctx->md_rate_est_ctx->nmv_vec_cost,
                                                        ctx->md_rate_est_ctx->nmvcoststack,
                                                        MV_COST_WEIGHT);

    const Mv      mv              = cand->block_mi.mv[0];
    const Mv      ref_mv          = cand->pred_mv[0];
    const int32_t refined_mv_rate = svt_av1_mv_bit_cost(
        &mv, &ref_mv, ctx->md_rate_est_ctx->nmv_vec_cost, ctx->md_rate_est_ctx->nmvcoststack, MV_COST_WEIGHT);

    cand_bf->fast_luma_rate = cand_bf->fast_luma_rate + refined_mv_rate - default_mv_rate;
}

static INLINE void opt_non_translation_motion_mode(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                   ModeDecisionCandidateBuffer* cand_bf, ModeDecisionCandidate* cand) {
    MdStage warp_refine_mds = ctx->wm_ctrls.refine_level == 1 ? MD_STAGE_1
        : ctx->wm_ctrls.refine_level == 2                     ? MD_STAGE_3
                                                              : INVALID_MD_STAGE;

    if (warp_refine_mds != INVALID_MD_STAGE && ctx->pd_pass == PD_PASS_1 && ctx->md_stage == warp_refine_mds &&
        cand->block_mi.motion_mode == WARPED_CAUSAL && cand->block_mi.mode == NEWMV) {
        const Mv                 default_mv           = cand->block_mi.mv[0];
        const Mv                 default_ref_mv       = cand->pred_mv[0];
        const uint8_t            default_drl_idx      = cand->drl_index;
        const WarpedMotionParams default_wm_params    = cand->wm_params_l0;
        const uint8_t            default_num_proj_ref = cand->block_mi.num_proj_ref;

        uint8_t motion_mode_valid = ctx->wm_ctrls.refinement_iterations
            ? svt_aom_wm_motion_refinement(pcs, ctx, cand, ctx->wm_ctrls.shut_approx_if_not_mds0)
            : 1;

        if (motion_mode_valid && (default_mv.as_int != cand->block_mi.mv[0].as_int)) {
            // Update wm_params and num_proj_ref for the chosen MV. This call is not
            // part of a search, so disable the shortcuts.
            svt_aom_warped_motion_parameters(ctx,
                                             cand->block_mi.mv[0],
                                             ctx->blk_geom,
                                             cand->block_mi.ref_frame[0], // WM only allowed for unipred
                                             &cand->wm_params_l0,
                                             &cand->block_mi.num_proj_ref,
                                             0,
                                             0,
                                             1);

            update_refined_mv_fast_rate(ctx, cand_bf, cand, default_mv, default_ref_mv);
            cand_bf->valid_luma_pred = 0;
        } else {
            // If refined mode was not valid, or the MV was not changed, reset the original settings to proceed with processing the candidate
            cand->block_mi.mv[0].as_int = default_mv.as_int;
            cand->pred_mv[0].as_int     = default_ref_mv.as_int;
            cand->wm_params_l0          = default_wm_params;
            cand->block_mi.num_proj_ref = default_num_proj_ref;
            cand->drl_index             = default_drl_idx;
        }
    }
    MdStage obmc_refine_mds = (ctx->obmc_ctrls.refine_level == 1 || ctx->obmc_ctrls.refine_level == 2) ? MD_STAGE_1
        : (ctx->obmc_ctrls.refine_level == 3 || ctx->obmc_ctrls.refine_level == 4)                     ? MD_STAGE_3
                                                                                   : INVALID_MD_STAGE;

    if (obmc_refine_mds != INVALID_MD_STAGE && ctx->pd_pass == PD_PASS_1 && ctx->md_stage == obmc_refine_mds &&
        cand->block_mi.motion_mode == OBMC_CAUSAL && cand->block_mi.mode == NEWMV) {
        const Mv      default_mv      = cand->block_mi.mv[0];
        const Mv      default_ref_mv  = cand->pred_mv[0];
        const uint8_t default_drl_idx = cand->drl_index;

#if CONFIG_ENABLE_OBMC
        uint8_t motion_mode_valid = svt_aom_obmc_motion_refinement(pcs, ctx, cand, ctx->obmc_ctrls.refine_level);
        if (motion_mode_valid) {
            if (default_mv.as_int != cand->block_mi.mv[0].as_int) {
                update_refined_mv_fast_rate(ctx, cand_bf, cand, default_mv, default_ref_mv);
                cand_bf->valid_luma_pred = 0;
            }
        } else
#endif // CONFIG_ENABLE_OBMC
        {
            // If refined mode was not valid, reset the original settings to proceed with processing the candidate
            cand->block_mi.mv[0].as_int = default_mv.as_int;
            cand->pred_mv[0].as_int     = default_ref_mv.as_int;
            cand->drl_index             = default_drl_idx;
        }
    }
}

static void full_loop_core(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                           EbPictureBufferDesc* input_pic, BlockLocation* loc) {
    ModeDecisionCandidate* cand                     = cand_bf->cand;
    const uint32_t         input_origin_index       = loc->input_origin_index;
    const uint32_t         input_cb_origin_in_index = loc->input_cb_origin_in_index;

    uint64_t y_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL];
    uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL];
    uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL];
    memset(y_full_distortion, 0, sizeof(y_full_distortion));
    memset(cb_full_distortion, 0, sizeof(cb_full_distortion));
    memset(cr_full_distortion, 0, sizeof(cr_full_distortion));

    uint64_t      y_coeff_bits  = 0;
    uint64_t      cb_coeff_bits = 0;
    uint64_t      cr_coeff_bits = 0;
    uint32_t      full_lambda   = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    const int32_t is_inter      = (is_inter_mode(cand->block_mi.mode) || cand->block_mi.use_intrabc) ? true : false;
    cand_bf->full_dist          = 0;
    // Set Skip Flag
    cand->block_mi.skip_mode = false;
    if (is_inter_mode(cand->block_mi.mode)) {
        opt_non_translation_motion_mode(pcs, ctx, cand_bf, cand);
        if (ctx->mds_do_chroma || ctx->mds_do_ifs || cand_bf->valid_luma_pred == 0 || ctx->need_hbd_comp_mds3) {
            // Perform INTER prediction
            product_prediction_fun_table[1](ctx->hbd_md, ctx, pcs, cand_bf);
            cand_bf->valid_luma_pred = 1;
        }
    } else if (ctx->mds_do_chroma || ctx->need_hbd_comp_mds3) {
        /* For intra, the luma pred should be valid from MDS0(there is no refinement - ifs, obmc / wm, etc. - that causes
         * inter to sometimes be invalid. If the encoder is changed so that luma pred becomes invalid, we should add that
         * check above to ensure the prediction is always valid before the transform.*/
        assert(cand_bf->valid_luma_pred);
        ctx->uv_intra_comp_only = ctx->need_hbd_comp_mds3 ? false : true;
        // Here, the mode is INTRA, but if intra_bc is used, must use inter prediction function
        product_prediction_fun_table[cand_bf->cand->block_mi.use_intrabc](ctx->hbd_md, ctx, pcs, cand_bf);
    }
    // Initialize luma CBF
    cand_bf->y_has_coeff   = 0;
    cand_bf->u_has_coeff   = 0;
    cand_bf->v_has_coeff   = 0;
    uint8_t start_tx_depth = 0;
    uint8_t end_tx_depth   = 0;
    get_start_end_tx_depth(pcs, ctx, cand_bf, &start_tx_depth, &end_tx_depth);
    if (ctx->subres_ctrls.odd_to_even_deviation_th && ctx->pd_pass == PD_PASS_0 && ctx->md_stage == MD_STAGE_3 &&
        ctx->is_subres_safe == (uint8_t)~0 /* only if invalid*/ && ctx->blk_geom->bheight == 64 &&
        ctx->blk_geom->bwidth == 64) {
        check_is_subres_safe(ctx, cand_bf, input_pic, input_origin_index, 0);
    }

    ctx->mds_subres_step = (ctx->is_subres_safe == 1) ? ctx->mds_subres_step : 0;
    //Y Residual: residual for INTRA is computed inside the TU loop
    if (is_inter) {
        //Y Residual
        svt_aom_residual_kernel(input_pic->buffer_y,
                                input_origin_index,
                                input_pic->stride_y << ctx->mds_subres_step,
                                cand_bf->pred->buffer_y,
                                0,
                                cand_bf->pred->stride_y << ctx->mds_subres_step,
                                (int16_t*)cand_bf->residual->buffer_y,
                                0,
                                cand_bf->residual->stride_y,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth,
                                ctx->blk_geom->bheight >> ctx->mds_subres_step);
    }
    // Check if should perform TX type search
    if (ctx->blk_geom->sq_size <= 64 && start_tx_depth == 0 && end_tx_depth == 0 && // TXS off
        !pcs->ppcs->sc_class1 && // Can't be SC b/c SC tries DCT_DCT and IDTX when only_dct_dct is 1
        search_dct_dct_only(pcs,
                            ctx,
                            cand_bf,
                            0 /*tx_depth*/,
                            is_inter)) { // TXT off

        uint8_t tx_search_skip_flag = 0;
        if (ctx->perform_mds1 && ctx->md_stage == MD_STAGE_3 && ctx->tx_shortcut_ctrls.bypass_tx_th &&
            !cand_bf->block_has_coeff &&
            ((cand_bf->luma_fast_dist * ctx->tx_shortcut_ctrls.bypass_tx_th) <
             (uint32_t)(ctx->blk_geom->bheight * ctx->blk_geom->bwidth * ctx->qp_index))) {
            tx_search_skip_flag = 1;
        }
        perform_dct_dct_tx(
            pcs, ctx, cand_bf, tx_search_skip_flag, ctx->blk_ptr->qindex, &y_coeff_bits, y_full_distortion);
    } else {
        perform_tx_partitioning(
            cand_bf, ctx, pcs, start_tx_depth, end_tx_depth, ctx->blk_ptr->qindex, &y_coeff_bits, y_full_distortion);
    }
    // Update coeff info based on luma TX so that chroma can take advantage of most accurate info
    cand_bf->block_has_coeff = (cand_bf->y_has_coeff) ? 1 : 0;

    const uint16_t txb_count = tx_blocks_per_depth[ctx->blk_geom->bsize][cand->block_mi.tx_depth];
    cand_bf->cnt_nz_coeff    = 0;
    for (uint8_t txb_itr = 0; txb_itr < txb_count; txb_itr++) {
        cand_bf->cnt_nz_coeff += cand_bf->eob.y[txb_itr];
    }
    //CHROMA
    if (ctx->mds_do_chroma) {
        assert(ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1);
        ctx->chroma_complexity = COMPONENT_LUMA;
        ctx->cfl_complexity    = COMPONENT_LUMA;
        if ((ctx->cfl_ctrls.enabled && ctx->cfl_ctrls.cplx_th) ||
            (ctx->tx_shortcut_ctrls.chroma_detector_level && ctx->md_stage == MD_STAGE_3 &&
             (ctx->tx_shortcut_ctrls.apply_pf_on_coeffs || ctx->use_tx_shortcuts_mds3))) {
            chroma_complexity_check_pred(ctx, cand_bf, input_pic, loc, 1 /*use_var*/);
        }

        const uint16_t cb_qindex     = ctx->qp_index;
        bool           cfl_performed = false;
        if (!ctx->cfl_ctrls.cplx_th || ctx->cfl_complexity == COMPONENT_CHROMA) {
            if (!is_inter && ctx->md_stage == MD_STAGE_3 && ctx->cfl_ctrls.enabled &&
                MAX(ctx->blk_geom->bheight, ctx->blk_geom->bwidth) <= 32) {
                // Test CFL if allowable:
                // 1: Recon the Luma and form the pred_buf_q3
                // 2: Loop over alphas and find the best CFL params
                // 3: Compare CFL cost to the best non-CFL chroma mode and select best
                cfl_prediction(pcs, cand_bf, ctx, input_pic, input_cb_origin_in_index, 0);

                cfl_performed = true;
            }
        }
        //Cb Residual
        svt_aom_residual_kernel(input_pic->buffer_cb,
                                input_cb_origin_in_index,
                                input_pic->stride_cb,
                                cand_bf->pred->buffer_cb,
                                0,
                                cand_bf->pred->stride_cb,
                                (int16_t*)cand_bf->residual->buffer_cb,
                                0,
                                cand_bf->residual->stride_cb,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        //Cr Residual
        svt_aom_residual_kernel(input_pic->buffer_cr,
                                input_cb_origin_in_index,
                                input_pic->stride_cr,
                                cand_bf->pred->buffer_cr,
                                0,
                                cand_bf->pred->stride_cr,
                                (int16_t*)cand_bf->residual->buffer_cr,
                                0,
                                cand_bf->residual->stride_cr,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);
        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             &cb_coeff_bits,
                             &cr_coeff_bits,
                             1);
        // If CFL is performed, check independent chroma vs. cfl.
        // If independent chroma data is unavailable, update the chroma fast rate, since the rate computed
        // at MDS0 assumes UV_DC_PRED is used.
        if (cfl_performed) {
            if (ctx->ind_uv_avail) {
                // If palette is used for the chroma mode (currently not supported) the intra_chroma_mode must be UV_DC_PRED
                assert(cand->palette_info == NULL || cand->palette_size[1] == 0);
                check_best_indepedant_cfl(pcs,
                                          input_pic,
                                          ctx,
                                          input_cb_origin_in_index,
                                          0,
                                          cand_bf,
                                          (uint8_t)cb_qindex,
                                          cb_full_distortion,
                                          cr_full_distortion,
                                          &cb_coeff_bits,
                                          &cr_coeff_bits);
            } else {
                cand_bf->fast_chroma_rate = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 1);
            }
        }
    }
    cand_bf->block_has_coeff = (cand_bf->y_has_coeff || cand_bf->u_has_coeff || cand_bf->v_has_coeff) ? true : false;
    svt_aom_full_cost(pcs,
                      ctx,
                      cand_bf,
                      full_lambda,
                      y_full_distortion,
                      cb_full_distortion,
                      cr_full_distortion,
                      &y_coeff_bits,
                      &cb_coeff_bits,
                      &cr_coeff_bits);
}

static void md_stage_1(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                       BlockLocation* loc) {
    ModeDecisionCandidateBuffer** cand_bf_ptr_array = &(ctx->cand_bf_ptr_array[0]);

    // Set MD Staging full_loop_core settings
    ctx->mds_do_txs = false;
    ctx->mds_do_txt = false;

    ctx->mds_do_spatial_sse       = ctx->spatial_sse_ctrls.level <= SSSE_MDS1;
    ctx->mds_fast_coeff_est_level = (ctx->pd_pass == PD_PASS_1) ? 1 : ctx->rate_est_ctrls.pd0_fast_coeff_est_level;
    ctx->mds_subres_step          = ctx->subres_ctrls.step;
    ctx->mds_do_chroma            = false;
    ctx->mds_do_ifs               = (ctx->ifs_ctrls.level == IFS_MDS1);
    ctx->mds_do_rdoq              = false;
    for (uint32_t cand_cnt = 0; cand_cnt < ctx->md_stage_1_count[ctx->target_class]; cand_cnt++) {
        const uint32_t               cand_bf_index = ctx->cand_buff_indices[ctx->target_class][cand_cnt];
        ModeDecisionCandidateBuffer* cand_bf       = cand_bf_ptr_array[cand_bf_index];
        full_loop_core(pcs, ctx, cand_bf, input_pic, loc);
    }
}

static void md_stage_2(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                       BlockLocation* loc) {
    ModeDecisionCandidateBuffer** cand_bf_ptr_array = &(ctx->cand_bf_ptr_array[0]);

    // If IFS is set to MDS1, but MDS1 is bypassed, perform IFS in MDS2 instead. Note if ctx->perform_mds1 is false, MDS2 is also skipped
    ctx->mds_do_ifs         = (ctx->ifs_ctrls.level == IFS_MDS2 ||
                       (ctx->ifs_ctrls.level == IFS_MDS1 && (!ctx->perform_mds1 || ctx->bypass_md_stage_1)));
    ctx->mds_do_txs         = false;
    ctx->mds_do_rdoq        = false;
    ctx->mds_do_spatial_sse = ctx->spatial_sse_ctrls.level <= SSSE_MDS2;
    // If MDS2 is the first MD stage to use spatial SSE, we must test every candidate (typically
    // inter candidates are skipped in MDS2 because the features are the same as those used in MDS1.
    const bool first_ssse_mds     = ctx->spatial_sse_ctrls.level == SSSE_MDS2;
    ctx->mds_fast_coeff_est_level = (ctx->pd_pass == PD_PASS_1) ? 1 : ctx->rate_est_ctrls.pd0_fast_coeff_est_level;
    ctx->mds_subres_step          = (ctx->pd_pass == PD_PASS_1) ? 0 : ctx->subres_ctrls.step;
    ctx->mds_do_chroma            = false;
    // Set MD Staging full_loop_core settings
    for (uint32_t cand_cnt = 0; cand_cnt < ctx->md_stage_2_count[ctx->target_class]; cand_cnt++) {
        uint32_t                     cand_bf_idx = ctx->cand_buff_indices[ctx->target_class][cand_cnt];
        ModeDecisionCandidateBuffer* cand_bf     = cand_bf_ptr_array[cand_bf_idx];
        ModeDecisionCandidate*       cand        = cand_bf->cand;
        // There is no difference between MDS1/MDS2 for inter candidates, so no need to recompute.
        // If spatial SSE is used in MDS2 (but not MDS1) we must check all candidates, otherwise the inter vs. intra cost
        // will be inaccurate. If IFS is performed in MDS2, we much test all inter candidates.
        if (is_inter_mode(cand->block_mi.mode) && !ctx->mds_do_ifs && !first_ssse_mds) {
            continue;
        }
        ctx->mds_do_txt = svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) ? 0
            : is_intra_mode(cand->block_mi.mode)                                     ? ctx->txt_ctrls.enabled
                                                                                     : 0;

        full_loop_core(pcs, ctx, cand_bf, input_pic, loc);
    }
}

static void update_intra_chroma_mode(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                     ModeDecisionCandidateBuffer* cand_bf) {
    ModeDecisionCandidate* cand     = cand_bf->cand;
    const uint8_t          is_inter = (is_inter_mode(cand->block_mi.mode) || cand->block_mi.use_intrabc);
    if (!is_inter && ctx->blk_geom->sq_size < 128 && ctx->has_uv) {
        // If palette is used for the chroma mode (currently not supported) the intra_chroma_mode must be UV_DC_PRED
        assert(cand->palette_info == NULL || cand->palette_size[1] == 0);

        const UvPredictionMode intra_chroma_mode = ctx->best_uv_mode[cand->block_mi.mode];
        const int32_t          angle_delta       = ctx->best_uv_angle[cand->block_mi.mode];

        if (cand->block_mi.uv_mode != intra_chroma_mode || cand->block_mi.angle_delta[PLANE_TYPE_UV] != angle_delta) {
            // Update intra_chroma_mode
            cand->block_mi.uv_mode                    = intra_chroma_mode;
            cand->block_mi.angle_delta[PLANE_TYPE_UV] = angle_delta;

            // Update transform_type_uv
            const TxSize tx_size_uv          = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
            cand_bf->cand->transform_type_uv = svt_aom_get_intra_uv_tx_type(
                cand->block_mi.uv_mode, tx_size_uv, pcs->ppcs->frm_hdr.reduced_tx_set);

            // Update fast_chroma_rate
            cand_bf->fast_chroma_rate = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 1);
        }
    }
}

static void md_stage_3_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                 uint32_t input_origin_index, uint32_t blk_origin_index) {
    ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[ctx->mds0_best_idx];

    // For 8x8 blocks, can't use 4x subsampling b/c no 8x2 transform
    ctx->mds_subres_step = ctx->blk_geom->sq_size >= 16 ? ctx->subres_ctrls.step
                                                        : MIN(1, ctx->subres_ctrls.step); //ON  !!!

    assert(IMPLIES(ctx->mds_subres_step == 2, ctx->blk_geom->sq_size >= 16));
    assert(IMPLIES(ctx->mds_subres_step == 1, ctx->blk_geom->sq_size >= 8));
    svt_aom_assert_err(IMPLIES(!ctx->disallow_4x4, ctx->mds_subres_step == 0),
                       "residual subsampling cannot be used with 4x4 blocks");

    full_loop_core_light_pd0(pcs, ctx, cand_bf, input_pic, input_origin_index, blk_origin_index);
}

/*
   md stage 3 for light PD1 path
*/
static void md_stage_3_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                 BlockLocation* loc) {
    ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[ctx->mds0_best_idx];

    // chroma is performed for all blocks b/c LDP1 assumes 4xN/Nx4 blocks are disabled (blk_geom->has_uv assumed true)
    ctx->mds_do_chroma      = true;
    ctx->uv_intra_comp_only = true;
    // If EncDec is bypassed, disable features affecting the TX that are usually disabled in EncDec
    if (ctx->bypass_encdec) {
        ctx->rdoq_ctrls.skip_uv      = 0;
        ctx->rdoq_ctrls.dct_dct_only = 0;
    }
    ctx->mds_do_rdoq              = true;
    ctx->mds_fast_coeff_est_level = 1;
    ctx->mds_subres_step          = 0;
    full_loop_core_light_pd1(pcs, ctx, cand_bf, input_pic, loc);
}

static void md_stage_3(PictureControlSet* pcs, ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                       BlockLocation* loc, uint32_t cand_total_cnt) {
    ModeDecisionCandidateBuffer** cand_bf_ptr_array = &(ctx->cand_bf_ptr_array[0]);
    // If EncDec is bypassed, disable features affecting the TX that are usually disabled in EncDec
    if (ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1) {
        ctx->pf_ctrls.pf_shape       = DEFAULT_SHAPE;
        ctx->rdoq_ctrls.skip_uv      = 0;
        ctx->rdoq_ctrls.dct_dct_only = 0;
    }

    // If IFS is set to a previous MD stage, but that MD stage is bypassed, perform IFS in MDS3 instead
    ctx->mds_do_ifs               = (ctx->ifs_ctrls.level == IFS_MDS3 ||
                       (ctx->ifs_ctrls.level == IFS_MDS1 &&
                        (!ctx->perform_mds1 || (ctx->bypass_md_stage_1 && ctx->bypass_md_stage_2))) ||
                       (ctx->ifs_ctrls.level == IFS_MDS2 && (!ctx->perform_mds1 || ctx->bypass_md_stage_2)));
    ctx->mds_do_txs               = ctx->txs_ctrls.enabled;
    ctx->mds_do_rdoq              = true;
    ctx->mds_do_spatial_sse       = ctx->spatial_sse_ctrls.level <= SSSE_MDS3;
    ctx->mds_fast_coeff_est_level = (ctx->pd_pass == PD_PASS_1) ? 1 : ctx->rate_est_ctrls.pd0_fast_coeff_est_level;
    ctx->mds_subres_step          = (ctx->pd_pass == PD_PASS_1) ? 0 : ctx->subres_ctrls.step;
    ctx->mds_do_chroma            = (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) ? true : false;
    for (uint32_t cand_cnt = 0; cand_cnt < cand_total_cnt; cand_cnt++) {
        uint32_t                     cand_bf_index = ctx->best_candidate_index_array[cand_cnt];
        ModeDecisionCandidateBuffer* cand_bf       = cand_bf_ptr_array[cand_bf_index];
        ModeDecisionCandidate*       cand          = cand_bf->cand;
        if (cand_bf->cand->block_mi.mode == DC_PRED) {
            if (ctx->scale_palette) {
                if (cand->palette_info != NULL && cand->palette_size[0] > 0) {
                    // if MD is done on 8bit( when HBD is 0 and bypass encdec is ON)
                    // Scale  palette colors to 10bit
                    for (uint8_t col = 0; col < cand->palette_size[0]; col++) {
                        cand->palette_info->pmi.palette_colors[col] *= 4;
                    }
                }
            }
        }
        ctx->mds_do_txt = svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) ? 0 : ctx->txt_ctrls.enabled;

        // If independent chroma search was performed before the last MD stage, update the chroma data
        if (ctx->ind_uv_avail && ctx->uv_ctrls.ind_uv_last_mds) {
            update_intra_chroma_mode(pcs, ctx, cand_bf);
        }

        full_loop_core(pcs, ctx, cand_bf, input_pic, loc);
    }
}

void svt_aom_move_blk_data(PictureControlSet* pcs, EncDecContext* ctx, BlkStruct* src, EcBlkStruct* dst) {
    dst->palette_size[0] = src->palette_size[0];
    dst->palette_size[1] = src->palette_size[1];
    if (svt_av1_allow_palette(pcs->ppcs->palette_level, ctx->blk_geom->bsize)) {
        svt_memcpy(&dst->palette_info->pmi, &src->palette_info->pmi, sizeof(PaletteModeInfo));
        assert(dst->palette_info->color_idx_map != NULL && "palette: Not-Enough-Memory");
        if (dst->palette_info->color_idx_map != NULL) {
            svt_memcpy(dst->palette_info->color_idx_map, src->palette_info->color_idx_map, MAX_PALETTE_SQUARE);
        } else {
            SVT_ERROR("palette: Not-Enough-Memory\n");
        }
    }

    svt_memcpy(&dst->eob, &src->eob, sizeof(EobData));
    svt_memcpy(dst->tx_type, src->tx_type, sizeof(src->tx_type[0]) * MAX_TXB_COUNT);
    dst->tx_type_uv = src->tx_type_uv;

    dst->overlappable_neighbors = src->overlappable_neighbors;

    dst->qindex = src->qindex;

    //CHKN    MacroBlockD*  av1xd;
    // Don't copy if dest. is NULL
    if (dst->av1xd != NULL) {
        svt_memcpy(dst->av1xd, src->av1xd, sizeof(MacroBlockD));
    }

    dst->inter_mode_ctx = src->inter_mode_ctx;
    //CHKN uint8_t  drl_index;
    //CHKN PredictionMode               pred_mode;
    dst->drl_index = src->drl_index;

    //CHKN IntMv  predmv[2];

    svt_memcpy(dst->predmv, src->predmv, 2 * sizeof(Mv));

    dst->mds_idx         = src->mds_idx;
    dst->drl_ctx[0]      = src->drl_ctx[0];
    dst->drl_ctx[1]      = src->drl_ctx[1];
    dst->drl_ctx_near[0] = src->drl_ctx_near[0];
    dst->drl_ctx_near[1] = src->drl_ctx_near[1];
}

static void move_blk_data_redund(PictureControlSet* pcs, ModeDecisionContext* ctx, BlkStruct* src, BlkStruct* dst) {
    dst->segment_id = src->segment_id;
    if (svt_av1_allow_palette(pcs->ppcs->palette_level, ctx->blk_geom->bsize)) {
        svt_memcpy(&dst->palette_info->pmi, &src->palette_info->pmi, sizeof(PaletteModeInfo));
        svt_memcpy(dst->palette_info->color_idx_map, src->palette_info->color_idx_map, MAX_PALETTE_SQUARE);
        dst->palette_size[0] = src->palette_size[0];
        dst->palette_size[1] = src->palette_size[1];
    }

    svt_memcpy(&dst->block_mi, &src->block_mi, sizeof(BlockModeInfo));
    svt_memcpy(&dst->eob, &src->eob, sizeof(EobData));
    svt_memcpy(dst->tx_type, src->tx_type, sizeof(src->tx_type[0]) * MAX_TXB_COUNT);
    dst->tx_type_uv = src->tx_type_uv;
    svt_memcpy(&dst->quant_dc, &src->quant_dc, sizeof(QuantDcData));
    dst->y_has_coeff            = src->y_has_coeff;
    dst->u_has_coeff            = src->u_has_coeff;
    dst->v_has_coeff            = src->v_has_coeff;
    dst->overlappable_neighbors = src->overlappable_neighbors;
    dst->block_has_coeff        = src->block_has_coeff;
    dst->qindex                 = src->qindex;
    svt_memcpy(dst->av1xd, src->av1xd, sizeof(MacroBlockD));

    dst->inter_mode_ctx = src->inter_mode_ctx;
    dst->drl_index      = src->drl_index;

    svt_memcpy(dst->predmv, src->predmv, 2 * sizeof(Mv));
    dst->drl_ctx[0]      = src->drl_ctx[0];
    dst->drl_ctx[1]      = src->drl_ctx[1];
    dst->drl_ctx_near[0] = src->drl_ctx_near[0];
    dst->drl_ctx_near[1] = src->drl_ctx_near[1];
    dst->cnt_nz_coeff    = src->cnt_nz_coeff;
    dst->full_dist       = src->full_dist;
    dst->cost            = src->cost;
    // only for MD
    uint16_t bwidth     = ctx->blk_geom->bwidth;
    uint16_t bheight    = ctx->blk_geom->bheight;
    uint16_t bwidth_uv  = ctx->blk_geom->bwidth_uv;
    uint16_t bheight_uv = ctx->blk_geom->bheight_uv;
    // if using 8bit MD and bypassing encdec, need to save 8bit and 10bit recon
    const bool save_both_recon = ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md &&
        ctx->pd_pass == PD_PASS_1;
    if (save_both_recon || !ctx->hbd_md) {
        svt_memcpy(dst->neigh_left_recon[0], src->neigh_left_recon[0], bheight);
        svt_memcpy(dst->neigh_left_recon[1], src->neigh_left_recon[1], bheight_uv);
        svt_memcpy(dst->neigh_left_recon[2], src->neigh_left_recon[2], bheight_uv);
        svt_memcpy(dst->neigh_top_recon[0], src->neigh_top_recon[0], bwidth);
        svt_memcpy(dst->neigh_top_recon[1], src->neigh_top_recon[1], bwidth_uv);
        svt_memcpy(dst->neigh_top_recon[2], src->neigh_top_recon[2], bwidth_uv);
    }
    if (save_both_recon || ctx->hbd_md) {
        uint16_t sz = sizeof(uint16_t);
        svt_memcpy(dst->neigh_left_recon_16bit[0], src->neigh_left_recon_16bit[0], bheight * sz);
        svt_memcpy(dst->neigh_left_recon_16bit[1], src->neigh_left_recon_16bit[1], bheight_uv * sz);
        svt_memcpy(dst->neigh_left_recon_16bit[2], src->neigh_left_recon_16bit[2], bheight_uv * sz);
        svt_memcpy(dst->neigh_top_recon_16bit[0], src->neigh_top_recon_16bit[0], bwidth * sz);
        svt_memcpy(dst->neigh_top_recon_16bit[1], src->neigh_top_recon_16bit[1], bwidth_uv * sz);
        svt_memcpy(dst->neigh_top_recon_16bit[2], src->neigh_top_recon_16bit[2], bwidth_uv * sz);
    }

    // wm
    dst->wm_params_l0 = src->wm_params_l0;
    dst->wm_params_l1 = src->wm_params_l1;
}

/*
Perform search for the best chroma mode (intra modes only). The search is performed only on the intra luma
modes that will be tested in MDS3 (plus DC is always tested). The search involves the following main parts:

1. Prepare all the chroma candidates to be tested
2. Perform prediction and the full loop (TX, quant, inv. quant)
3. Compute the full cost for the remaining candidates and select the best chroma mode (to be combined
   with luma modes in future MD stages).

*/
static void search_best_mds3_uv_mode(PictureControlSet* pcs, EbPictureBufferDesc* input_pic,
                                     uint32_t input_cb_origin_in_index, uint32_t input_cr_origin_in_index,
                                     uint32_t cu_chroma_origin_index, ModeDecisionContext* ctx,
                                     uint32_t full_cand_count) {
    PictureParentControlSet* ppcs        = pcs->ppcs;
    FrameHeader*             frm_hdr     = &ppcs->frm_hdr;
    uint32_t                 full_lambda = ctx->full_lambda_md[ctx->hbd_md ? EB_10_BIT_MD : EB_8_BIT_MD];

    uint64_t coeff_rate[UV_PAETH_PRED + 1][(MAX_ANGLE_DELTA << 1) + 1];
    uint64_t distortion[UV_PAETH_PRED + 1][(MAX_ANGLE_DELTA << 1) + 1];

    ModeDecisionCandidate* cand_array              = ctx->fast_cand_array;
    uint32_t               start_fast_buffer_index = ppcs->max_can_count;
    uint32_t               start_full_buffer_index = ctx->max_nics;
    unsigned int           uv_mode_total_count     = start_fast_buffer_index;
    const TxSize           tx_size_uv              = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);

    ModeDecisionCandidateBuffer** cand_bf_ptr_array_base = ctx->cand_bf_ptr_array;
    ModeDecisionCandidateBuffer** cand_bf_ptr_array      = &(cand_bf_ptr_array_base[0]);

    uint8_t tested_uv_modes[UV_PAETH_PRED + 1][(MAX_ANGLE_DELTA << 1) + 1] = {{0}};
    // Prepare the candidates to test
    // The search will only be over the chroma modes that are to be tested in MDS3 plus UV_DC_PRED, which will always be tested
    for (uint32_t full_loop_cand_idx = 0; full_loop_cand_idx < full_cand_count + 1; ++full_loop_cand_idx) {
        ModeDecisionCandidateBuffer* full_cand_bf;
        ModeDecisionCandidate*       full_cand;

        if (full_loop_cand_idx < full_cand_count) {
            uint32_t full_cand_index = ctx->best_candidate_index_array[full_loop_cand_idx];
            full_cand_bf             = cand_bf_ptr_array[full_cand_index];
            full_cand                = full_cand_bf->cand;

            /* Don't consider candidate if it's inter. UV_DC_PRED will always be tested and will be injected automatically as the final
            candidate, so don't need to add it. */
            if (is_inter_mode(full_cand->block_mi.mode) || full_cand->block_mi.use_intrabc ||
                full_cand->block_mi.uv_mode == UV_DC_PRED) {
                continue;
            }

            // CFL is tested in MDS3, so the intra_chroma_mode should not be CFL at this stage
            assert(full_cand->block_mi.uv_mode != UV_CFL_PRED);
            // Don't add duplicate types
            if (tested_uv_modes[full_cand->block_mi.uv_mode]
                               [MAX_ANGLE_DELTA + full_cand->block_mi.angle_delta[PLANE_TYPE_UV]]) {
                continue;
            }

            tested_uv_modes[full_cand->block_mi.uv_mode]
                           [MAX_ANGLE_DELTA + full_cand->block_mi.angle_delta[PLANE_TYPE_UV]] = 1;

            // Inject all intra chroma modes for candidates that made it to MDS3. Set here instead of below for sanitizer
            cand_array[uv_mode_total_count].block_mi.uv_mode = full_cand->block_mi.uv_mode;
            cand_array[uv_mode_total_count].block_mi.angle_delta[PLANE_TYPE_UV] =
                full_cand->block_mi.angle_delta[PLANE_TYPE_UV];
        } else {
            // Always test DC, which is injected during the last pass
            assert(full_loop_cand_idx == full_cand_count);
            cand_array[uv_mode_total_count].block_mi.uv_mode                    = UV_DC_PRED;
            cand_array[uv_mode_total_count].block_mi.angle_delta[PLANE_TYPE_UV] = 0;
        }

        cand_array[uv_mode_total_count].block_mi.use_intrabc               = 0;
        cand_array[uv_mode_total_count].block_mi.angle_delta[PLANE_TYPE_Y] = 0;
        cand_array[uv_mode_total_count].block_mi.mode                      = DC_PRED;
        cand_array[uv_mode_total_count].block_mi.tx_depth                  = 0;
        cand_array[uv_mode_total_count].palette_info                       = NULL;
        cand_array[uv_mode_total_count].block_mi.filter_intra_mode         = FILTER_INTRA_MODES;
        cand_array[uv_mode_total_count].block_mi.cfl_alpha_signs           = 0;
        cand_array[uv_mode_total_count].block_mi.cfl_alpha_idx             = 0;
        cand_array[uv_mode_total_count].transform_type[0]                  = DCT_DCT;
        cand_array[uv_mode_total_count].block_mi.ref_frame[0]              = INTRA_FRAME;
        cand_array[uv_mode_total_count].block_mi.ref_frame[1]              = NONE_FRAME;
        cand_array[uv_mode_total_count].block_mi.motion_mode               = SIMPLE_TRANSLATION;
        cand_array[uv_mode_total_count].transform_type_uv                  = svt_aom_get_intra_uv_tx_type(
            cand_array[uv_mode_total_count].block_mi.uv_mode, tx_size_uv, frm_hdr->reduced_tx_set);
        if (svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) &&
            cand_array[uv_mode_total_count].transform_type_uv != DCT_DCT) {
            continue;
        }
        uv_mode_total_count++;
    }
    uv_mode_total_count = uv_mode_total_count - start_fast_buffer_index;

    ctx->mds_do_rdoq              = true;
    ctx->mds_do_spatial_sse       = ctx->spatial_sse_ctrls.level <= SSSE_MDS3;
    ctx->mds_fast_coeff_est_level = 1;
    ctx->uv_intra_comp_only       = true;
    // This function is only for searching chroma, so it is expected that chroma is valid for this block
    assert(ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1);
    ctx->mds_do_chroma = true;

    // Perform full-loop search for all UV modes
    for (unsigned int uv_mode_count = 0; uv_mode_count < uv_mode_total_count; uv_mode_count++) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_mode_count + start_full_buffer_index];
        ModeDecisionCandidate* cand = cand_bf->cand = &ctx->fast_cand_array[uv_mode_count + start_fast_buffer_index];

        product_prediction_fun_table[is_inter_mode(cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);

        uint16_t cb_qindex                                       = ctx->qp_index;
        uint64_t cb_coeff_bits                                   = 0;
        uint64_t cr_coeff_bits                                   = 0;
        uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
        uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};

        //Cb Residual
        svt_aom_residual_kernel(input_pic->buffer_cb,
                                input_cb_origin_in_index,
                                input_pic->stride_cb,
                                cand_bf->pred->buffer_cb,
                                cu_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                (int16_t*)cand_bf->residual->buffer_cb,
                                cu_chroma_origin_index,
                                cand_bf->residual->stride_cb,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        //Cr Residual
        svt_aom_residual_kernel(input_pic->buffer_cr,
                                input_cr_origin_in_index,
                                input_pic->stride_cr,
                                cand_bf->pred->buffer_cr,
                                cu_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                (int16_t*)cand_bf->residual->buffer_cr,
                                cu_chroma_origin_index,
                                cand_bf->residual->stride_cr,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             &cb_coeff_bits,
                             &cr_coeff_bits,
                             1);

        coeff_rate[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]] =
            cb_coeff_bits + cr_coeff_bits;
        distortion[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]] =
            cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] + cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL];

    } // End full loop

    // Loop over all available luma intra modes, then loop over all uv_modes to derive the best uv_mode for a given intra mode
    memset(tested_uv_modes, 0, (UV_PAETH_PRED + 1) * ((MAX_ANGLE_DELTA << 1) + 1) * sizeof(tested_uv_modes[0][0]));
    for (uint32_t full_loop_cand_idx = 0; full_loop_cand_idx < full_cand_count; ++full_loop_cand_idx) {
        ModeDecisionCandidateBuffer* full_cand_bf;
        ModeDecisionCandidate*       full_cand;

        uint32_t full_cand_index = ctx->best_candidate_index_array[full_loop_cand_idx];
        full_cand_bf             = cand_bf_ptr_array[full_cand_index];
        full_cand                = full_cand_bf->cand;
        /* Don't consider candidate if it's inter. UV_DC_PRED will always be tested and will be injected automatically as the final
        candidate, so don't need to add it. */
        if (is_inter_mode(full_cand->block_mi.mode) || full_cand->block_mi.use_intrabc) {
            continue;
        }

        // Don't need to re-check modes as cost will be the same.  Cost will only depend on luma intra mode, not angle delta,
        // so no need to repeat for different angle deltas
        if (tested_uv_modes[full_cand->block_mi.mode][0]) {
            continue;
        }

        tested_uv_modes[full_cand->block_mi.mode][0] = 1;

        PredictionMode intra_mode = full_cand->block_mi.mode;
        // uv mode loop
        ctx->best_uv_cost[intra_mode] = (uint64_t)~0;
        for (unsigned int uv_mode_count = 0; uv_mode_count < uv_mode_total_count; uv_mode_count++) {
            ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_mode_count + start_full_buffer_index];
            ModeDecisionCandidate*       cand    = cand_bf->cand =
                &ctx->fast_cand_array[uv_mode_count + start_fast_buffer_index];

            // Update the luma intra mode, as it affects the chroma mode rate
            cand->block_mi.mode       = intra_mode;
            cand_bf->fast_chroma_rate = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 0);

            const uint64_t rate =
                coeff_rate[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]] +
                cand_bf->fast_chroma_rate;

            const uint64_t uv_cost = RDCOST(
                full_lambda,
                rate,
                distortion[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]]);

            if (uv_cost < ctx->best_uv_cost[intra_mode]) {
                ctx->best_uv_mode[intra_mode]  = cand->block_mi.uv_mode;
                ctx->best_uv_angle[intra_mode] = cand->block_mi.angle_delta[PLANE_TYPE_UV];
                ctx->best_uv_cost[intra_mode]  = uv_cost;
            }
        }
    }

    ctx->ind_uv_avail = 1;
}

/*
Perform search for the best chroma mode (intra modes only).  The search involves
the following main parts:

1. Prepare all the chroma candidates to be tested
2. Perform compensation and compute the distortion for each candidate
3. Sort the candidates, and for the best n candidates, perform the full loop (TX, quant, inv. quant)
4. Compute the full cost for the remaining candidates and select the best chroma mode (to be combined
   with luma modes in future MD stages).

*/
static void search_best_independent_uv_mode(PictureControlSet* pcs, EbPictureBufferDesc* input_pic,
                                            uint32_t input_cb_origin_in_index, uint32_t input_cr_origin_in_index,
                                            uint32_t cu_chroma_origin_index, ModeDecisionContext* ctx) {
    PictureParentControlSet* ppcs        = pcs->ppcs;
    FrameHeader*             frm_hdr     = &ppcs->frm_hdr;
    uint32_t                 full_lambda = ctx->full_lambda_md[ctx->hbd_md ? EB_10_BIT_MD : EB_8_BIT_MD];
    const TxSize             tx_size_uv  = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);

    uint64_t coeff_rate[UV_PAETH_PRED + 1][(MAX_ANGLE_DELTA << 1) + 1];
    uint64_t distortion[UV_PAETH_PRED + 1][(MAX_ANGLE_DELTA << 1) + 1];

    ModeDecisionCandidate* cand_array              = ctx->fast_cand_array;
    uint32_t               start_fast_buffer_index = ppcs->max_can_count;
    uint32_t               start_full_buffer_index = ctx->max_nics;
    unsigned int           uv_mode_total_count     = start_fast_buffer_index;
    UvPredictionMode       uv_mode_end             = (UvPredictionMode)ctx->intra_ctrls.intra_mode_end;
    uint8_t                uv_mode_start           = UV_DC_PRED;
    bool      use_angle_delta = ctx->intra_ctrls.angular_pred_level ? av1_use_angle_delta(ctx->blk_geom->bsize) : 0;
    uint8_t   disable_angle_prediction                = (ctx->intra_ctrls.angular_pred_level == 0);
    const int uv_angle_delta_shift                    = 1;
    uint8_t   directional_mode_skip_mask[INTRA_MODES] = {0};
    // For aggressive angular levels, don't test angular candidate for certain modes
    if (ctx->intra_ctrls.angular_pred_level >= 4) {
        for (uint8_t i = D45_PRED; i < INTRA_MODE_END; i++) {
            directional_mode_skip_mask[i] = 1;
        }
    }

    // Prepare chroma candidates to be tested
    for (UvPredictionMode uv_mode = uv_mode_start; uv_mode <= uv_mode_end; ++uv_mode) {
        // If mode is not directional, or is enabled directional mode, proceed with injection
        if (!av1_is_directional_mode((PredictionMode)uv_mode) ||
            (!disable_angle_prediction && directional_mode_skip_mask[(PredictionMode)uv_mode] == 0)) {
            int uv_angle_delta_candidate_count = (use_angle_delta && av1_is_directional_mode((PredictionMode)uv_mode) &&
                                                  ctx->intra_ctrls.angular_pred_level <= 2)
                ? 7
                : 1;

            for (int uv_angle_delta_counter = 0; uv_angle_delta_counter < uv_angle_delta_candidate_count;
                 ++uv_angle_delta_counter) {
                int32_t uv_angle_delta = CLIP(uv_angle_delta_shift *
                                                  (uv_angle_delta_candidate_count == 1 ? 0
                                                                                       : uv_angle_delta_counter -
                                                           (uv_angle_delta_candidate_count >> 1)),
                                              -MAX_ANGLE_DELTA,
                                              MAX_ANGLE_DELTA);
                if (ctx->intra_ctrls.angular_pred_level >= 2 &&
                    (uv_angle_delta == -1 || uv_angle_delta == 1 || uv_angle_delta == -2 || uv_angle_delta == 2)) {
                    continue;
                }
                cand_array[uv_mode_total_count].block_mi.use_intrabc                = 0;
                cand_array[uv_mode_total_count].block_mi.angle_delta[PLANE_TYPE_UV] = 0;
                cand_array[uv_mode_total_count].block_mi.mode                       = DC_PRED;
                cand_array[uv_mode_total_count].block_mi.uv_mode                    = uv_mode;
                cand_array[uv_mode_total_count].block_mi.angle_delta[PLANE_TYPE_UV] = uv_angle_delta;
                cand_array[uv_mode_total_count].block_mi.tx_depth                   = 0;
                cand_array[uv_mode_total_count].palette_info                        = NULL;
                cand_array[uv_mode_total_count].block_mi.filter_intra_mode          = FILTER_INTRA_MODES;
                cand_array[uv_mode_total_count].block_mi.cfl_alpha_signs            = 0;
                cand_array[uv_mode_total_count].block_mi.cfl_alpha_idx              = 0;
                cand_array[uv_mode_total_count].transform_type[0]                   = DCT_DCT;
                cand_array[uv_mode_total_count].block_mi.ref_frame[0]               = INTRA_FRAME;
                cand_array[uv_mode_total_count].block_mi.ref_frame[1]               = NONE_FRAME;
                cand_array[uv_mode_total_count].block_mi.motion_mode                = SIMPLE_TRANSLATION;
                cand_array[uv_mode_total_count].transform_type_uv                   = svt_aom_get_intra_uv_tx_type(
                    uv_mode, tx_size_uv, frm_hdr->reduced_tx_set);
                if (svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) &&
                    cand_array[uv_mode_total_count].transform_type_uv != DCT_DCT) {
                    continue;
                }
                uv_mode_total_count++;
            }
        }
    }
    uv_mode_total_count = uv_mode_total_count - start_fast_buffer_index;

    // Prepare fast-loop search settings
    ctx->mds_do_rdoq              = true;
    ctx->mds_do_spatial_sse       = ctx->spatial_sse_ctrls.level <= SSSE_MDS3;
    ctx->mds_fast_coeff_est_level = 1;
    ctx->uv_intra_comp_only       = true;
    // This function is only for searching chroma, so it is expected that chroma is valid for this block
    assert(ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1);
    ctx->mds_do_chroma = true;

    // Perform fast-loop search for all candidates
    for (unsigned int uv_mode_count = 0; uv_mode_count < uv_mode_total_count; uv_mode_count++) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_mode_count + start_full_buffer_index];
        cand_bf->cand                        = &ctx->fast_cand_array[uv_mode_count + start_fast_buffer_index];
        product_prediction_fun_table[is_inter_mode(cand_bf->cand->block_mi.mode)](ctx->hbd_md, ctx, pcs, cand_bf);
        uint32_t chroma_fast_distortion;
        if (!ctx->hbd_md) {
            const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize_uv];
            unsigned int            sse;
            uint8_t*                pred_cb = cand_bf->pred->buffer_cb + cu_chroma_origin_index;
            uint8_t*                src_cb  = input_pic->buffer_cb + input_cb_origin_in_index;
            chroma_fast_distortion = fn_ptr->vf(pred_cb, cand_bf->pred->stride_cb, src_cb, input_pic->stride_cb, &sse);

            uint8_t* pred_cr = cand_bf->pred->buffer_cr + cu_chroma_origin_index;
            uint8_t* src_cr  = input_pic->buffer_cr + input_cr_origin_in_index;
            chroma_fast_distortion += fn_ptr->vf(pred_cr, cand_bf->pred->stride_cr, src_cr, input_pic->stride_cr, &sse);
        } else {
            const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize_uv];
            unsigned int            sse;
            uint16_t*               pred_cb = ((uint16_t*)cand_bf->pred->buffer_cb) + cu_chroma_origin_index;
            uint16_t*               src_cb  = ((uint16_t*)input_pic->buffer_cb) + input_cb_origin_in_index;
            chroma_fast_distortion          = fn_ptr->vf_hbd_10(CONVERT_TO_BYTEPTR(pred_cb),
                                                       cand_bf->pred->stride_cb,
                                                       CONVERT_TO_BYTEPTR(src_cb),
                                                       input_pic->stride_cb,
                                                       &sse);

            uint16_t* pred_cr = ((uint16_t*)cand_bf->pred->buffer_cr) + cu_chroma_origin_index;
            uint16_t* src_cr  = ((uint16_t*)input_pic->buffer_cr) + input_cr_origin_in_index;
            chroma_fast_distortion += fn_ptr->vf_hbd_10(CONVERT_TO_BYTEPTR(pred_cr),
                                                        cand_bf->pred->stride_cr,
                                                        CONVERT_TO_BYTEPTR(src_cr),
                                                        input_pic->stride_cr,
                                                        &sse);
        }
        // Do not consider rate @ this stage
        *(cand_bf->fast_cost) = chroma_fast_distortion;
    }

    // Sort uv_mode candidates (in terms of distortion only)
    uint32_t* uv_cand_buff_indices;
    EB_MALLOC_ARRAY_NO_CHECK(uv_cand_buff_indices, ctx->max_nics_uv);
    memset(uv_cand_buff_indices, 0xFF, ctx->max_nics_uv * sizeof(*uv_cand_buff_indices));

    sort_fast_cost_based_candidates(
        ctx,
        start_full_buffer_index,
        uv_mode_total_count, //how many cand buffers to sort. one of the buffers can have max cost.
        uv_cand_buff_indices);

    // Reset *(cand_bf->fast_cost)
    for (unsigned int uv_mode_count = 0; uv_mode_count < uv_mode_total_count; uv_mode_count++) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_mode_count + start_full_buffer_index];
        *(cand_bf->fast_cost)                = MAX_CU_COST;
    }

    // Set number of UV candidates to be tested in the full loop
    unsigned int uv_mode_nfl_count = pcs->scs->allintra ? ppcs->is_highest_layer ? 16 : 32
        : pcs->slice_type == I_SLICE                    ? 64
        : !ppcs->is_highest_layer                       ? 32
                                                        : 16;
    uv_mode_nfl_count              = MAX(1, DIVIDE_AND_ROUND(uv_mode_nfl_count * ctx->uv_ctrls.uv_nic_scaling_num, 16));
    uv_mode_nfl_count              = MIN(uv_mode_nfl_count, uv_mode_total_count);
    uv_mode_nfl_count              = MAX(uv_mode_nfl_count, 1);
    // Always test UV_DC_PRED in the full loop
    unsigned int uv_mode_count = 0;
    for (; uv_mode_count < MIN(uv_mode_total_count, uv_mode_nfl_count); uv_mode_count++) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_cand_buff_indices[uv_mode_count]];
        ModeDecisionCandidate*       cand    = cand_bf->cand =
            &ctx->fast_cand_array[uv_cand_buff_indices[uv_mode_count] - start_full_buffer_index +
                                  start_fast_buffer_index];
        if (cand->block_mi.uv_mode == UV_DC_PRED) {
            break;
        }
    }
    if (uv_mode_count == uv_mode_nfl_count) {
        // Add DC to be tested at fast loop
        uv_cand_buff_indices[uv_mode_nfl_count] = start_full_buffer_index; // DC candidate
        uv_mode_nfl_count += 1;
        assert(ctx->fast_cand_array[uv_cand_buff_indices[uv_mode_count] - start_full_buffer_index +
                                    start_fast_buffer_index]
                   .block_mi.uv_mode == UV_DC_PRED);
        // Re-check bounds of uv_mode_nfl_count
        uv_mode_nfl_count = MIN(uv_mode_nfl_count, uv_mode_total_count);
        uv_mode_nfl_count = MAX(uv_mode_nfl_count, 1);
    }
    // Full-loop search uv_mode
    for (uv_mode_count = 0; uv_mode_count < MIN(uv_mode_total_count, uv_mode_nfl_count); uv_mode_count++) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_cand_buff_indices[uv_mode_count]];
        ModeDecisionCandidate*       cand    = cand_bf->cand =
            &ctx->fast_cand_array[uv_cand_buff_indices[uv_mode_count] - start_full_buffer_index +
                                  start_fast_buffer_index];
        uint16_t cb_qindex                                       = ctx->qp_index;
        uint64_t cb_coeff_bits                                   = 0;
        uint64_t cr_coeff_bits                                   = 0;
        uint64_t cb_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};
        uint64_t cr_full_distortion[DIST_TOTAL][DIST_CALC_TOTAL] = {{0}};

        //Cb Residual
        svt_aom_residual_kernel(input_pic->buffer_cb,
                                input_cb_origin_in_index,
                                input_pic->stride_cb,
                                cand_bf->pred->buffer_cb,
                                cu_chroma_origin_index,
                                cand_bf->pred->stride_cb,
                                (int16_t*)cand_bf->residual->buffer_cb,
                                cu_chroma_origin_index,
                                cand_bf->residual->stride_cb,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);

        //Cr Residual
        svt_aom_residual_kernel(input_pic->buffer_cr,
                                input_cr_origin_in_index,
                                input_pic->stride_cr,
                                cand_bf->pred->buffer_cr,
                                cu_chroma_origin_index,
                                cand_bf->pred->stride_cr,
                                (int16_t*)cand_bf->residual->buffer_cr,
                                cu_chroma_origin_index,
                                cand_bf->residual->stride_cr,
                                ctx->hbd_md,
                                ctx->blk_geom->bwidth_uv,
                                ctx->blk_geom->bheight_uv);
        svt_aom_full_loop_uv(pcs,
                             ctx,
                             cand_bf,
                             input_pic,
                             COMPONENT_CHROMA,
                             cb_qindex,
                             cb_full_distortion,
                             cr_full_distortion,
                             &cb_coeff_bits,
                             &cr_coeff_bits,
                             1);

        coeff_rate[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]] =
            cb_coeff_bits + cr_coeff_bits;
        distortion[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]] =
            cb_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL] + cr_full_distortion[DIST_SSD][DIST_CALC_RESIDUAL];
    }

    // Loop over all intra modes, then loop over all uv_modes to derive the best uv_mode for a given intra mode (in term of rate)
    uint8_t intra_mode_end = ctx->intra_ctrls.intra_mode_end;
    // intra_mode loop (luma mode loop)
    for (PredictionMode intra_mode = DC_PRED; intra_mode <= intra_mode_end; ++intra_mode) {
        // If mode is not directional, or is enabled directional mode, proceed with injection
        if (av1_is_directional_mode(intra_mode) &&
            (disable_angle_prediction || directional_mode_skip_mask[intra_mode])) {
            continue;
        }

        // uv mode loop
        ctx->best_uv_cost[intra_mode] = (uint64_t)~0;
        for (uv_mode_count = 0; uv_mode_count < MIN(uv_mode_total_count, uv_mode_nfl_count); uv_mode_count++) {
            ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[uv_cand_buff_indices[uv_mode_count]];
            ModeDecisionCandidate*       cand    = &(ctx->fast_cand_array[uv_cand_buff_indices[uv_mode_count] -
                                                                 start_full_buffer_index + start_fast_buffer_index]);
            // Update the luma intra mode, as it affects the chroma mode rate
            cand->block_mi.mode       = intra_mode;
            cand_bf->fast_chroma_rate = svt_aom_get_intra_uv_fast_rate(pcs, ctx, cand_bf, 0);

            const uint64_t rate =
                coeff_rate[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]] +
                cand_bf->fast_chroma_rate;

            const uint64_t uv_cost = RDCOST(
                full_lambda,
                rate,
                distortion[cand->block_mi.uv_mode][MAX_ANGLE_DELTA + cand->block_mi.angle_delta[PLANE_TYPE_UV]]);

            if (uv_cost < ctx->best_uv_cost[intra_mode]) {
                ctx->best_uv_mode[intra_mode]  = cand->block_mi.uv_mode;
                ctx->best_uv_angle[intra_mode] = cand->block_mi.angle_delta[PLANE_TYPE_UV];
                ctx->best_uv_cost[intra_mode]  = uv_cost;
            }
        }
    }
    EB_FREE_ARRAY(uv_cand_buff_indices);
    ctx->ind_uv_avail = 1;
}

// Perform the NIC class pruning and candidate pruning after MSD0
static void post_mds0_nic_pruning(PictureControlSet* pcs, ModeDecisionContext* ctx, uint64_t best_md_stage_cost) {
    const struct NicPruningCtrls pruning_ctrls = ctx->nic_ctrls.pruning_ctrls;
    uint32_t                     q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.nic_pruning_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->ppcs->scs->static_config.qp);
    uint64_t mds1_class_th = (pruning_ctrls.mds1_class_th == (uint64_t)~0)
        ? pruning_ctrls.mds1_class_th
        : DIVIDE_AND_ROUND(pruning_ctrls.mds1_class_th * q_weight, q_weight_denom);

    uint8_t  mds1_band_cnt            = pruning_ctrls.mds1_band_cnt;
    uint16_t mds1_cand_th_rank_factor = pruning_ctrls.mds1_cand_th_rank_factor;

    uint64_t mds1_cand_base_th_intra = (pruning_ctrls.mds1_cand_base_th_intra == (uint64_t)~0)
        ? pruning_ctrls.mds1_cand_base_th_intra
        : DIVIDE_AND_ROUND(pruning_ctrls.mds1_cand_base_th_intra * q_weight, q_weight_denom);

    uint64_t                      mds1_cand_base_th_inter = (pruning_ctrls.mds1_cand_base_th_inter == (uint64_t)~0)
                             ? pruning_ctrls.mds1_cand_base_th_inter
                             : DIVIDE_AND_ROUND(pruning_ctrls.mds1_cand_base_th_inter * q_weight, q_weight_denom);
    ModeDecisionCandidateBuffer** cand_bf_arr             = ctx->cand_bf_ptr_array;
    for (CandClass cidx = CAND_CLASS_0; cidx < CAND_CLASS_TOTAL; cidx++) {
        const uint64_t mds1_cand_th = is_intra_class(cidx) ? mds1_cand_base_th_intra : mds1_cand_base_th_inter;
        if ((mds1_cand_th != (uint64_t)~0 || mds1_class_th != (uint64_t)~0) && ctx->md_stage_0_count[cidx] > 0 &&
            ctx->md_stage_1_count[cidx] > 0) {
            const uint32_t* cand_buff = ctx->cand_buff_indices[cidx];
            const uint64_t  best_cost = *cand_bf_arr[cand_buff[0]]->fast_cost;
            // inter class pruning
            if (best_cost && best_md_stage_cost && best_cost != best_md_stage_cost) {
                if (mds1_class_th == 0) {
                    ctx->md_stage_1_count[cidx] = 0;
                    continue;
                }
                uint64_t dev = ((best_cost - best_md_stage_cost) * 100) / best_md_stage_cost;
                if (dev) {
                    if (dev >= mds1_class_th) {
                        ctx->md_stage_1_count[cidx] = 0;
                        continue;
                    }
                    if (mds1_band_cnt >= 3 && ctx->md_stage_1_count[cidx] > 1) {
                        const uint8_t band_idx      = (uint8_t)(dev * (mds1_band_cnt - 1) / mds1_class_th);
                        ctx->md_stage_1_count[cidx] = DIVIDE_AND_ROUND(ctx->md_stage_1_count[cidx], band_idx + 1);
                    }
                }
            }
            // intra class pruning
            uint32_t cand_count = 1;
            if (best_cost) {
                while (cand_count < ctx->md_stage_1_count[cidx] &&
                       (*cand_bf_arr[cand_buff[cand_count]]->fast_cost - best_cost) * 100 / best_cost <
                           mds1_cand_th / (mds1_cand_th_rank_factor ? mds1_cand_th_rank_factor * cand_count : 1)) {
                    cand_count++;
                }
            }
            ctx->md_stage_1_count[cidx] = cand_count;
        }
        ctx->md_stage_1_total_count += ctx->md_stage_1_count[cidx];
    }

    // If enabled, skip MDS1 when we have only one candidate
    if (pruning_ctrls.enable_skipping_mds1 && ctx->md_stage_1_total_count == 1) {
        ctx->perform_mds1 = 0;
    }
}

// Perform the NIC class pruning and candidate pruning after MSD1
static void post_mds1_nic_pruning(PictureControlSet* pcs, ModeDecisionContext* ctx, uint64_t best_md_stage_cost) {
    const struct NicPruningCtrls pruning_ctrls = ctx->nic_ctrls.pruning_ctrls;

    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.nic_pruning_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->ppcs->scs->static_config.qp);
    const uint64_t mds2_cand_th = (pruning_ctrls.mds2_cand_base_th == (uint64_t)~0)
        ? pruning_ctrls.mds2_cand_base_th
        : DIVIDE_AND_ROUND(pruning_ctrls.mds2_cand_base_th * q_weight, q_weight_denom);

    const uint64_t                mds2_class_th        = (pruning_ctrls.mds2_class_th == (uint64_t)~0)
                              ? pruning_ctrls.mds2_class_th
                              : DIVIDE_AND_ROUND(pruning_ctrls.mds2_class_th * q_weight, q_weight_denom);
    const uint8_t                 mds2_band_cnt        = pruning_ctrls.mds2_band_cnt;
    const uint16_t                mds2_relative_dev_th = pruning_ctrls.mds2_relative_dev_th;
    ModeDecisionCandidateBuffer** cand_bf_arr          = ctx->cand_bf_ptr_array;
    for (CandClass cidx = CAND_CLASS_0; cidx < CAND_CLASS_TOTAL; cidx++) {
        if ((mds2_cand_th != (uint64_t)~0 || mds2_class_th != (uint64_t)~0) && ctx->md_stage_1_count[cidx] > 0 &&
            ctx->md_stage_2_count[cidx] > 0) {
            const uint32_t* cand_buff = ctx->cand_buff_indices[cidx];
            const uint64_t  best_cost = *cand_bf_arr[cand_buff[0]]->full_cost;

            // class pruning
            if (best_cost && best_md_stage_cost && best_cost != best_md_stage_cost) {
                if (mds2_class_th == 0) {
                    ctx->md_stage_2_count[cidx] = 0;
                    continue;
                }
                uint64_t dev = ((best_cost - best_md_stage_cost) * 100) / best_md_stage_cost;
                if (dev) {
                    if (dev >= mds2_class_th) {
                        ctx->md_stage_2_count[cidx] = 0;
                        continue;
                    }
                    if (mds2_band_cnt >= 3 && ctx->md_stage_2_count[cidx] > 1) {
                        uint8_t band_idx            = (uint8_t)(dev * (mds2_band_cnt - 1) / mds2_class_th);
                        ctx->md_stage_2_count[cidx] = DIVIDE_AND_ROUND(ctx->md_stage_2_count[cidx], band_idx + 1);
                    }
                }
            }
            // intra class pruning
            // candidate pruning
            if (ctx->md_stage_2_count[cidx] > 0) {
                uint32_t cand_count = 1;
                if (best_cost && cand_count < ctx->md_stage_2_count[cidx]) {
                    uint16_t mds2_cand_th_rank_factor = pruning_ctrls.mds2_cand_th_rank_factor;
                    // When enabled, modify the rank factor based on info from previous MD stages
                    if (mds2_cand_th_rank_factor) {
                        if (cidx != ctx->mds1_best_class_it) {
                            mds2_cand_th_rank_factor += 3;
                        } else if (ctx->mds0_best_idx == ctx->mds1_best_idx) {
                            mds2_cand_th_rank_factor += 2;
                        }
                    }
                    uint64_t dev      = (*cand_bf_arr[cand_buff[cand_count]]->full_cost - best_cost) * 100 / best_cost;
                    uint64_t prev_dev = dev;
                    while (
                        (!mds2_relative_dev_th || dev <= prev_dev + mds2_relative_dev_th) &&
                        (dev < mds2_cand_th / (mds2_cand_th_rank_factor ? mds2_cand_th_rank_factor * cand_count : 1))) {
                        cand_count++;
                        // Break out of loop if reached max cand_count to avoid accessing unallocated candidate buffer
                        if (cand_count >= ctx->md_stage_2_count[cidx]) {
                            break;
                        }
                        prev_dev = dev;
                        dev      = (*cand_bf_arr[cand_buff[cand_count]]->full_cost - best_cost) * 100 / best_cost;
                    }
                }
                ctx->md_stage_2_count[cidx] = cand_count;
            }
        }
        ctx->md_stage_2_total_count += ctx->md_stage_2_count[cidx];
    }
}

// Perform the NIC class pruning and candidate pruning after MSD2
static void post_mds2_nic_pruning(PictureControlSet* pcs, ModeDecisionContext* ctx, uint64_t best_md_stage_cost) {
    const struct NicPruningCtrls pruning_ctrls = ctx->nic_ctrls.pruning_ctrls;

    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.nic_pruning_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->ppcs->scs->static_config.qp);
    const uint64_t mds3_cand_th = (pruning_ctrls.mds3_cand_base_th == (uint64_t)~0)
        ? pruning_ctrls.mds3_cand_base_th
        : DIVIDE_AND_ROUND(pruning_ctrls.mds3_cand_base_th * q_weight, q_weight_denom);

    const uint64_t                mds3_class_th = (pruning_ctrls.mds3_class_th == (uint64_t)~0)
                       ? pruning_ctrls.mds3_class_th
                       : DIVIDE_AND_ROUND(pruning_ctrls.mds3_class_th * q_weight, q_weight_denom);
    const uint8_t                 mds3_band_cnt = pruning_ctrls.mds3_band_cnt;
    ModeDecisionCandidateBuffer** cand_bf_arr   = ctx->cand_bf_ptr_array;
    ctx->md_stage_3_total_count                 = 0;
    for (CandClass cidx = CAND_CLASS_0; cidx < CAND_CLASS_TOTAL; cidx++) {
        if ((mds3_cand_th != (uint64_t)~0 || mds3_class_th != (uint64_t)~0) && ctx->md_stage_2_count[cidx] > 0 &&
            // Perform pruning with post-MDS2 THs to preserve the onion ring
            ctx->md_stage_3_count[cidx] > 0) {
            const uint32_t* cand_buff = ctx->cand_buff_indices[cidx];
            const uint64_t  best_cost = *cand_bf_arr[cand_buff[0]]->full_cost;

            // inter class pruning
            if (best_cost && best_md_stage_cost && best_cost != best_md_stage_cost) {
                if (mds3_class_th == 0) {
                    ctx->md_stage_3_count[cidx] = 0;
                    continue;
                }
                uint64_t dev = ((best_cost - best_md_stage_cost) * 100) / best_md_stage_cost;
                if (dev) {
                    if (dev >= mds3_class_th) {
                        ctx->md_stage_3_count[cidx] = 0;
                        continue;
                    }
                    if (mds3_band_cnt >= 3 && ctx->md_stage_3_count[cidx] > 1) {
                        const uint8_t band_idx      = (uint8_t)(dev * (mds3_band_cnt - 1) / mds3_class_th);
                        ctx->md_stage_3_count[cidx] = DIVIDE_AND_ROUND(ctx->md_stage_3_count[cidx], band_idx + 1);
                    }
                }
            }
            // intra class pruning
            uint32_t cand_count = 1;
            if (best_cost) {
                while (
                    cand_count < ctx->md_stage_3_count[cidx] &&
                    (((*cand_bf_arr[cand_buff[cand_count]]->full_cost - best_cost) * 100) / best_cost < mds3_cand_th)) {
                    cand_count++;
                }
            }
            ctx->md_stage_3_count[cidx] = cand_count;
        }
        ctx->md_stage_3_total_count += ctx->md_stage_3_count[cidx];
    }
}

int      svt_aom_get_reference_mode_context_new(const MacroBlockD* xd);
uint64_t estimate_ref_frame_type_bits(ModeDecisionContext* ctx, BlkStruct* blk_ptr, uint8_t ref_frame_type,
                                      bool is_compound);

/*
 * Estimate the rate of signaling all available ref_frame_type
 */
static void estimate_ref_frames_num_bits(ModeDecisionContext* ctx, PictureControlSet* pcs) {
    uint64_t     comp_inter_fac_bits_uni = 0;
    uint64_t     comp_inter_fac_bits_bi  = 0;
    FrameHeader* frm_hdr                 = &pcs->ppcs->frm_hdr;
    // does the feature use compound prediction or not
    // (if not specified at the frame/segment level)
    if (frm_hdr->reference_mode == REFERENCE_MODE_SELECT) {
        if (MIN(ctx->blk_geom->bwidth, ctx->blk_geom->bheight) >= 8) {
            int32_t reference_mode_context;
            // aom_write_symbol(w, is_compound, svt_aom_get_reference_mode_cdf(blk_ptr->av1xd), 2);
            reference_mode_context  = svt_aom_get_reference_mode_context_new(ctx->blk_ptr->av1xd);
            comp_inter_fac_bits_uni = ctx->md_rate_est_ctx->comp_inter_fac_bits[reference_mode_context][0];
            comp_inter_fac_bits_bi  = ctx->md_rate_est_ctx->comp_inter_fac_bits[reference_mode_context][1];
        }
    }
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        //single ref/list
        if (rf[1] == NONE_FRAME) {
            MvReferenceFrame ref_frame_type                   = rf[0];
            ctx->estimate_ref_frames_num_bits[ref_frame_type] = estimate_ref_frame_type_bits(
                                                                    ctx, ctx->blk_ptr, ref_frame_type, 0) +
                comp_inter_fac_bits_uni;
        } else {
            ctx->estimate_ref_frames_num_bits[ref_pair] = estimate_ref_frame_type_bits(ctx, ctx->blk_ptr, ref_pair, 1) +
                comp_inter_fac_bits_bi;
        }
    }
}

static void calc_scr_to_recon_dist_per_quadrant(ModeDecisionContext* ctx, EbPictureBufferDesc* input_pic,
                                                const uint32_t               input_origin_index,
                                                const uint32_t               input_cb_origin_in_index,
                                                ModeDecisionCandidateBuffer* cand_bf, const uint32_t blk_origin_index,
                                                const uint32_t blk_chroma_origin_index) {
    EbPictureBufferDesc* recon_ptr = cand_bf->recon;

    EbSpatialFullDistType spatial_full_dist_type_fun = ctx->hbd_md ? svt_full_distortion_kernel16_bits
                                                                   : svt_spatial_full_distortion_kernel;

    uint8_t r, c;
    int32_t quadrant_size = ctx->blk_geom->sq_size >> 1;

    for (r = 0; r < 2; r++) {
        for (c = 0; c < 2; c++) {
            ctx->rec_dist_per_quadrant[c + (r << 1)] = spatial_full_dist_type_fun(
                input_pic->buffer_y,
                input_origin_index + c * quadrant_size + (r * quadrant_size) * input_pic->stride_y,
                input_pic->stride_y,
                recon_ptr->buffer_y,
                blk_origin_index + c * quadrant_size + (r * quadrant_size) * recon_ptr->stride_y,
                recon_ptr->stride_y,
                (uint32_t)quadrant_size,
                (uint32_t)quadrant_size);
            // If quadrant_size == 4 then rec_dist_per_quadrant will have luma only because spatial_full_dist_type_fun does not support smaller than 4x4
            if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1 && quadrant_size > 4) {
                ctx->rec_dist_per_quadrant[c + (r << 1)] += spatial_full_dist_type_fun(
                    input_pic->buffer_cb,
                    input_cb_origin_in_index + c * (quadrant_size >> 1) +
                        (r * (quadrant_size >> 1)) * input_pic->stride_cb,
                    input_pic->stride_cb,
                    recon_ptr->buffer_cb,
                    blk_chroma_origin_index + c * (quadrant_size >> 1) +
                        (r * (quadrant_size >> 1)) * recon_ptr->stride_cb,
                    recon_ptr->stride_cb,
                    (uint32_t)(quadrant_size >> 1),
                    (uint32_t)(quadrant_size >> 1));

                ctx->rec_dist_per_quadrant[c + (r << 1)] += spatial_full_dist_type_fun(
                    input_pic->buffer_cr,
                    input_cb_origin_in_index + c * (quadrant_size >> 1) +
                        (r * (quadrant_size >> 1)) * input_pic->stride_cr,
                    input_pic->stride_cr,
                    recon_ptr->buffer_cr,
                    blk_chroma_origin_index + c * (quadrant_size >> 1) +
                        (r * (quadrant_size >> 1)) * recon_ptr->stride_cr,
                    recon_ptr->stride_cr,
                    (uint32_t)(quadrant_size >> 1),
                    (uint32_t)(quadrant_size >> 1));
            }
        }
    }
}

static uint8_t is_intra_bordered(const ModeDecisionContext* ctx) {
    MacroBlockD* xd = ctx->blk_ptr->av1xd;

    const MbModeInfo* const above_mbmi = xd->above_mbmi;
    const MbModeInfo* const left_mbmi  = xd->left_mbmi;
    const int               has_above  = xd->up_available;
    const int               has_left   = xd->left_available;

    if (has_above && has_left) {
        if ((!is_inter_block(&above_mbmi->block_mi)) && (!is_inter_block(&left_mbmi->block_mi))) {
            return 1;
        } else {
            return 0;
        }
    } else {
        return 0;
    }
}

#if FTR_VLPD0
static const uint8_t var_log2_lut[4] = {6, 5, 4, 3};
static const uint8_t var_grid_lut[4] = {1, 2, 4, 8};
static const uint8_t var_base_lut[4] = {0, 1, 5, 21};

void svt_aom_get_blk_var_map(int block_size, int org_x, int org_y, int* blk_idx, int sub_idx[4]) {
    // Map block size to level: 64->0, 32->1, 16->2, 8->3
    const int lvl = 6 - svt_log2f(block_size);

    // Parent block
    const int shift = var_log2_lut[lvl];
    const int grid  = var_grid_lut[lvl];
    const int base  = var_base_lut[lvl];

    const int gx = org_x >> shift;
    const int gy = org_y >> shift;

    *blk_idx = base + gy * grid + gx;

    // Sub-blocks
    const int sub_lvl   = lvl + 1;
    const int sub_shift = var_log2_lut[sub_lvl];
    const int sub_base  = var_base_lut[sub_lvl];
    const int sub_grid  = var_grid_lut[sub_lvl];

    const int sx = org_x >> sub_shift;
    const int sy = org_y >> sub_shift;

    sub_idx[0] = sub_base + (sy + 0) * sub_grid + (sx + 0);
    sub_idx[1] = sub_base + (sy + 0) * sub_grid + (sx + 1);
    sub_idx[2] = sub_base + (sy + 1) * sub_grid + (sx + 0);
    sub_idx[3] = sub_base + (sy + 1) * sub_grid + (sx + 1);
}

/*
 * Compute a variance-based cost for VERY_LIGHT_PD0 (VLPD0, all-intra),
 * using QP-adaptive absolute variance and sub-block variance spread.
 * The cost is normalized by block area and used during inter-depth
 * decisions, with early exits and split-rate estimation disabled.
 */
static INLINE uint32_t compute_vlpd0_cost_allintra(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    // QP scaling
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.lpd0_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->scs->static_config.qp);

    // Block variance lookup
    int blk_idx;
    int sub_idx[4];
    // Get origin of the block relative to SB origin
    const Position blk_org = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
    svt_aom_get_blk_var_map(ctx->blk_geom->sq_size, blk_org.x, blk_org.y, &blk_idx, sub_idx);

    uint16_t* sb_var  = pcs->ppcs->variance[ctx->sb_index];
    uint32_t  blk_var = sb_var[blk_idx];

    uint32_t blk_size = ctx->blk_geom->sq_size;
    uint32_t area     = blk_size * blk_size;
    uint32_t bias     = 1000;

    // Bias logic
    if (blk_size == 64) {
        uint32_t abs_th = DIVIDE_AND_ROUND(100 * q_weight, q_weight_denom);
        bias += 50 * MIN(blk_var / abs_th, 10);

    } else if (blk_size >= 16) {
        uint32_t min_var = UINT32_MAX;
        uint32_t max_var = 0;

        for (int i = 0; i < 4; i++) {
            uint32_t v = sb_var[sub_idx[i]];
            min_var    = MIN(min_var, v);
            max_var    = MAX(max_var, v);
        }

        uint32_t spread_var = max_var - min_var;

        uint32_t abs_th = DIVIDE_AND_ROUND(400 * q_weight, q_weight_denom);
        bias += 25 * MIN(blk_var / abs_th, 10);

        uint32_t peak_th = DIVIDE_AND_ROUND(25 * q_weight, q_weight_denom);
        bias += 10 * MIN(spread_var / peak_th, 10);

    } else {
        uint32_t abs_th = DIVIDE_AND_ROUND(25 * q_weight, q_weight_denom);
        bias += 40 * MIN(blk_var / abs_th, 10);
    }

    return (area * bias) / 1000;
}

#endif
static void md_encode_block_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                      EbPictureBufferDesc* input_pic) {
    const BlockGeom* blk_geom = ctx->blk_geom;
#if FTR_VLPD0 // score
    BlkStruct* blk_ptr = ctx->blk_ptr;
    if (pcs->scs->allintra && ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0) {
        blk_ptr->cost = compute_vlpd0_cost_allintra(pcs, ctx);
        return;
    }
#endif
    uint32_t       fast_candidate_total_count;
    const uint32_t input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
        (ctx->blk_org_x + input_pic->org_x);
    const uint32_t blk_origin_index = 0;
#if !FTR_VLPD0
    BlkStruct* blk_ptr = ctx->blk_ptr;
#endif
    if (!ctx->skip_intra) {
        svt_aom_init_xd(pcs, ctx);
        ctx->mds_do_chroma      = false;
        ctx->uv_intra_comp_only = false;
    }
    if (pcs->slice_type != I_SLICE) {
        ctx->me_sb_addr      = ctx->sb_ptr->index;
        ctx->me_block_offset = svt_aom_get_me_block_offset(
            ctx->blk_org_x, ctx->blk_org_y, ctx->blk_geom->bsize, pcs->ppcs->enable_me_8x8, pcs->ppcs->enable_me_16x16);
        ctx->me_cand_offset = ctx->me_block_offset * pcs->ppcs->pa_me_data->max_cand;
    }

    generate_md_stage_0_cand_light_pd0(ctx, &fast_candidate_total_count, pcs);

    if (ctx->lpd0_use_src_samples) {
        uint8_t* src_y = input_pic->buffer_y + input_origin_index;
        svt_memcpy(ctx->recon_neigh_y->top_array + ctx->blk_org_x, src_y - input_pic->stride_y, ctx->blk_geom->bwidth);

        for (uint32_t row_idx = 0; row_idx < ctx->blk_geom->bheight; ++row_idx) {
            ctx->recon_neigh_y->left_array[ctx->blk_org_y + row_idx] = *(src_y + row_idx * input_pic->stride_y - 1);
        }

        ctx->recon_neigh_y->top_left_array[ctx->recon_neigh_y->max_pic_h + ctx->blk_org_x - ctx->blk_org_y] = *(
            src_y - input_pic->stride_y - 1);
    }
    ctx->md_stage       = MD_STAGE_0;
    ctx->mds0_best_idx  = 0;
    ctx->mds0_best_cost = (uint64_t)~0;
    assert(fast_candidate_total_count <= ctx->max_nics && "not enough cand buffers");
    // If only one candidate, only need to perform compensation, not distortion calc
    // unless if VLPD0 where mds0 will become the last stage and SSD is needed
    if (fast_candidate_total_count == 1 && ctx->lpd0_ctrls.pd0_level != VERY_LIGHT_PD0) {
        ModeDecisionCandidateBuffer* cand_bf = ctx->cand_bf_ptr_array[0];
        cand_bf->cand                        = &ctx->fast_cand_array[0];
        cand_bf->cand->block_mi.tx_depth     = 0;
        product_prediction_fun_table_light_pd0[is_inter_mode(cand_bf->cand->block_mi.mode)](0, ctx, pcs, cand_bf);
    } else {
        md_stage_0_light_pd0(pcs, ctx, fast_candidate_total_count, input_pic, input_origin_index, blk_origin_index);
    }

    if (ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0) {
        uint32_t rate      = ctx->md_rate_est_ctx->partition_fac_bits[0][PARTITION_NONE];
        uint64_t dist      = ctx->mds0_best_cost;
        ctx->blk_ptr->cost = RDCOST(ctx->full_sb_lambda_md[EB_8_BIT_MD], rate, dist);
    } else {
        ctx->md_stage = MD_STAGE_3;
        md_stage_3_light_pd0(pcs, ctx, input_pic, input_origin_index, blk_origin_index);
        // Update the cost
        ctx->blk_ptr->cost = *(ctx->cand_bf_ptr_array[ctx->mds0_best_idx]->full_cost);
    }
    assert(ctx->lpd1_ctrls.pd1_level < LPD1_LEVELS);

    // Save info needed for depth refinement and/or LPD1
    blk_ptr->block_mi.mode = ctx->cand_bf_ptr_array[ctx->mds0_best_idx]->cand->block_mi.mode;

    // Save info used by depth refinemetn and the light-PD1 detector (detector uses 64x64 block info only)
    if (ctx->lpd0_ctrls.pd0_level != VERY_LIGHT_PD0) {
        // Save info needed only for LPD1 detector
        if (ctx->lpd1_ctrls.pd1_level > REGULAR_PD1 && ctx->lpd1_ctrls.use_lpd1_detector[ctx->lpd1_ctrls.pd1_level] &&
            blk_geom->sq_size == 64) {
            ModeDecisionCandidate* cand = ctx->cand_bf_ptr_array[ctx->mds0_best_idx]->cand;
            if (is_inter_mode(cand->block_mi.mode)) {
                blk_ptr->block_mi.ref_frame[0] = cand->block_mi.ref_frame[0];
                blk_ptr->block_mi.ref_frame[1] = cand->block_mi.ref_frame[1];
                // Set MVs
                blk_ptr->block_mi.mv[0].as_int = cand->block_mi.mv[0].as_int;
                if (has_second_ref(&blk_ptr->block_mi)) {
                    blk_ptr->block_mi.mv[1].as_int = cand->block_mi.mv[1].as_int;
                }
            }
        }

        // Save info needed for depth refinement
        ctx->blk_ptr->cnt_nz_coeff = ctx->cand_bf_ptr_array[ctx->mds0_best_idx]->cnt_nz_coeff;
    }
    // If intra is used, generate recon and copy to necessary buffers
    if (!ctx->skip_intra && !ctx->lpd0_use_src_samples) {
        uint32_t                     candidate_index = ctx->mds0_best_idx;
        ModeDecisionCandidateBuffer* cand_bf         = ctx->cand_bf_ptr_array[candidate_index];

        // Update the variables needed for recon
        cand_bf->cand->transform_type[0] = DCT_DCT;
        ctx->blk_ptr->y_has_coeff        = cand_bf->y_has_coeff;
        // generate recon
        av1_perform_inverse_transform_recon(pcs, ctx, cand_bf, ctx->blk_geom);

        //copy neigh recon data in blk_ptr
        uint32_t             j;
        EbPictureBufferDesc* recon_ptr       = cand_bf->recon;
        uint32_t             rec_luma_offset = 0;

        svt_memcpy(ctx->blk_ptr->neigh_top_recon[0],
                   recon_ptr->buffer_y + rec_luma_offset + (ctx->blk_geom->bheight - 1) * recon_ptr->stride_y,
                   ctx->blk_geom->bwidth);

        for (j = 0; j < ctx->blk_geom->bheight; ++j) {
            ctx->blk_ptr->neigh_left_recon[0][j] =
                recon_ptr->buffer_y[rec_luma_offset + ctx->blk_geom->bwidth - 1 + j * recon_ptr->stride_y];
        }
    }
}

int svt_aom_get_comp_group_idx_context_enc(const MacroBlockD* xd);

// Copy the recon samples to update the neighbour arrays
static void copy_recon_md(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf) {
    const BlockGeom* blk_geom = ctx->blk_geom;
    if (!ctx->has_uv && ctx->cfl_ctrls.enabled) {
        // Store the luma data for 4x* and *x4 blocks to be used for CFL
        // Store recon with block origin offset b/c we may access in other blocks (for 4xN/Nx4 cases where chroma
        // is not allowed for each block).
        const Position       blk_org = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
        uint32_t             dst_offset      = blk_org.x + blk_org.y * cand_bf->recon->stride_y;
        uint32_t             dst_stride      = cand_bf->recon->stride_y;
        EbPictureBufferDesc* recon_ptr       = cand_bf->recon;
        uint32_t             rec_luma_offset = 0;
        if (ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1) {
            // If using a fixed partition structure (only pred depth and no NSQ, can copy directly to final buffer
            // b/c no d1 or d2 decision
            if (ctx->fixed_partition) {
                svt_aom_get_recon_pic(pcs, &recon_ptr, ctx->hbd_md);
                rec_luma_offset = (recon_ptr->org_y + ctx->blk_org_y) * recon_ptr->stride_y +
                    (recon_ptr->org_x + ctx->blk_org_x);
            } else {
                recon_ptr       = ctx->blk_ptr->recon_tmp;
                rec_luma_offset = 0;
            }
        }
        // if using 8bit MD and bypassing encdec, need to save 8bit and 10bit recon
        if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md && ctx->pd_pass == PD_PASS_1) {
            // copy 10bit
            if (ctx->fixed_partition) {
                svt_aom_get_recon_pic(pcs, &recon_ptr, 1);
            } else {
                recon_ptr = ctx->blk_ptr->recon_tmp;
            }

            for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
                svt_memcpy(ctx->cfl_temp_luma_recon16bit + dst_offset + j * dst_stride,
                           ((uint16_t*)recon_ptr->buffer_y) + (rec_luma_offset + j * recon_ptr->stride_y),
                           sizeof(uint16_t) * blk_geom->bwidth);
            }

            // Copy 8bit
            // 8bit recon must be stored in the pic buffers, because the blk_ptr->recon_tmp contains the 10bit recon
            svt_aom_get_recon_pic(pcs, &recon_ptr, 0);
            rec_luma_offset = (recon_ptr->org_y + ctx->blk_org_y) * recon_ptr->stride_y +
                (recon_ptr->org_x + ctx->blk_org_x);

            for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
                svt_memcpy(&ctx->cfl_temp_luma_recon[dst_offset + j * dst_stride],
                           recon_ptr->buffer_y + rec_luma_offset + j * recon_ptr->stride_y,
                           blk_geom->bwidth);
            }
        } else if (ctx->hbd_md) {
            for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
                svt_memcpy(ctx->cfl_temp_luma_recon16bit + dst_offset + j * dst_stride,
                           ((uint16_t*)recon_ptr->buffer_y) + (rec_luma_offset + j * recon_ptr->stride_y),
                           sizeof(uint16_t) * blk_geom->bwidth);
            }
        } else {
            for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
                svt_memcpy(&ctx->cfl_temp_luma_recon[dst_offset + j * dst_stride],
                           recon_ptr->buffer_y + rec_luma_offset + j * recon_ptr->stride_y,
                           blk_geom->bwidth);
            }
        }
    } // END CFL COPIES
    //copy neigh recon data in blk_ptr
    EbPictureBufferDesc* recon_ptr       = cand_bf->recon;
    uint32_t             rec_luma_offset = 0;
    uint32_t             rec_cb_offset   = 0;
    uint32_t             rec_cr_offset   = 0;

    // If bypassing MD, recon is stored in different buffer; need to update the buffer to copy from
    if (ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1) {
        // If using a fixed partition structure (only pred depth and no NSQ, can copy directly to final buffer
        // b/c no d1 or d2 decision
        if (ctx->fixed_partition) {
            svt_aom_get_recon_pic(pcs, &recon_ptr, ctx->hbd_md);
            rec_luma_offset = (recon_ptr->org_y + ctx->blk_org_y) * recon_ptr->stride_y +
                (recon_ptr->org_x + ctx->blk_org_x);

            uint32_t round_origin_x = ROUND_UV(ctx->blk_org_x); // for Chroma blocks with size of 4
            uint32_t round_origin_y = ROUND_UV(ctx->blk_org_y); // for Chroma blocks with size of 4
            rec_cb_offset           = rec_cr_offset =
                ((round_origin_x + recon_ptr->org_x + (round_origin_y + recon_ptr->org_y) * recon_ptr->stride_cb) >> 1);
        } else {
            recon_ptr       = ctx->blk_ptr->recon_tmp;
            rec_luma_offset = rec_cb_offset = rec_cr_offset = 0;
        }
    }
    // if using 8bit MD and bypassing encdec, need to save 8bit and 10bit recon
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md && ctx->pd_pass == PD_PASS_1) {
        // copy 16bit recon
        if (ctx->fixed_partition) {
            svt_aom_get_recon_pic(pcs, &recon_ptr, 1);
        } else {
            recon_ptr = ctx->blk_ptr->recon_tmp;
        }
        uint16_t sz = sizeof(uint16_t);
        // Copy bottom row (used for intra pred of the below block)
        svt_memcpy(ctx->blk_ptr->neigh_top_recon_16bit[0],
                   recon_ptr->buffer_y + sz * (rec_luma_offset + (blk_geom->bheight - 1) * recon_ptr->stride_y),
                   sz * blk_geom->bwidth);

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_memcpy(ctx->blk_ptr->neigh_top_recon_16bit[1],
                       recon_ptr->buffer_cb + sz * (rec_cb_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cb),
                       sz * blk_geom->bwidth_uv);

            svt_memcpy(ctx->blk_ptr->neigh_top_recon_16bit[2],
                       recon_ptr->buffer_cr + sz * (rec_cr_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cr),
                       sz * blk_geom->bwidth_uv);
        }

        // Copy right column (used for intra pred of the right block)
        for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
            ctx->blk_ptr->neigh_left_recon_16bit[0][j] =
                ((uint16_t*)recon_ptr->buffer_y)[rec_luma_offset + blk_geom->bwidth - 1 + j * recon_ptr->stride_y];
        }

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            for (uint32_t j = 0; j < blk_geom->bheight_uv; ++j) {
                ctx->blk_ptr->neigh_left_recon_16bit[1][j] =
                    ((uint16_t*)
                         recon_ptr->buffer_cb)[rec_cb_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cb];
                ctx->blk_ptr->neigh_left_recon_16bit[2][j] =
                    ((uint16_t*)
                         recon_ptr->buffer_cr)[rec_cr_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cr];
            }
        }

        // Copy 8bit recon
        // 8bit recon must be stored in the pic buffers, because the blk_ptr->recon_tmp contains the 10bit recon
        svt_aom_get_recon_pic(pcs, &recon_ptr, 0);
        rec_luma_offset = (recon_ptr->org_y + ctx->blk_org_y) * recon_ptr->stride_y +
            (recon_ptr->org_x + ctx->blk_org_x);
        uint32_t round_origin_x = (ctx->blk_org_x >> 3) << 3; // for Chroma blocks with size of 4
        uint32_t round_origin_y = (ctx->blk_org_y >> 3) << 3; // for Chroma blocks with size of 4
        rec_cb_offset           = rec_cr_offset =
            ((round_origin_x + recon_ptr->org_x + (round_origin_y + recon_ptr->org_y) * recon_ptr->stride_cb) >> 1);

        // Copy bottom row (used for intra pred of the below block)
        svt_memcpy(ctx->blk_ptr->neigh_top_recon[0],
                   recon_ptr->buffer_y + rec_luma_offset + (blk_geom->bheight - 1) * recon_ptr->stride_y,
                   blk_geom->bwidth);

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_memcpy(ctx->blk_ptr->neigh_top_recon[1],
                       recon_ptr->buffer_cb + rec_cb_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cb,
                       blk_geom->bwidth_uv);
            svt_memcpy(ctx->blk_ptr->neigh_top_recon[2],
                       recon_ptr->buffer_cr + rec_cr_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cr,
                       blk_geom->bwidth_uv);
        }

        // Copy right column (used for intra pred of the right block)
        for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
            ctx->blk_ptr->neigh_left_recon[0][j] =
                recon_ptr->buffer_y[rec_luma_offset + blk_geom->bwidth - 1 + j * recon_ptr->stride_y];
        }

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            for (uint32_t j = 0; j < blk_geom->bheight_uv; ++j) {
                ctx->blk_ptr->neigh_left_recon[1][j] =
                    recon_ptr->buffer_cb[rec_cb_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cb];
                ctx->blk_ptr->neigh_left_recon[2][j] =
                    recon_ptr->buffer_cr[rec_cr_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cr];
            }
        }
    } else if (!ctx->hbd_md) {
        // Copy 8bit recon
        // Copy bottom row (used for intra pred of the below block)
        svt_memcpy(ctx->blk_ptr->neigh_top_recon[0],
                   recon_ptr->buffer_y + rec_luma_offset + (blk_geom->bheight - 1) * recon_ptr->stride_y,
                   blk_geom->bwidth);

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_memcpy(ctx->blk_ptr->neigh_top_recon[1],
                       recon_ptr->buffer_cb + rec_cb_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cb,
                       blk_geom->bwidth_uv);
            svt_memcpy(ctx->blk_ptr->neigh_top_recon[2],
                       recon_ptr->buffer_cr + rec_cr_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cr,
                       blk_geom->bwidth_uv);
        }

        // Copy right column (used for intra pred of the right block)
        for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
            ctx->blk_ptr->neigh_left_recon[0][j] =
                recon_ptr->buffer_y[rec_luma_offset + blk_geom->bwidth - 1 + j * recon_ptr->stride_y];
        }

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            for (uint32_t j = 0; j < blk_geom->bheight_uv; ++j) {
                ctx->blk_ptr->neigh_left_recon[1][j] =
                    recon_ptr->buffer_cb[rec_cb_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cb];
                ctx->blk_ptr->neigh_left_recon[2][j] =
                    recon_ptr->buffer_cr[rec_cr_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cr];
            }
        }
    } else {
        // Copy 16bit recon
        uint16_t sz = sizeof(uint16_t);

        // Copy bottom row (used for intra pred of the below block)
        svt_memcpy(ctx->blk_ptr->neigh_top_recon_16bit[0],
                   recon_ptr->buffer_y + sz * (rec_luma_offset + (blk_geom->bheight - 1) * recon_ptr->stride_y),
                   sz * blk_geom->bwidth);

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            svt_memcpy(ctx->blk_ptr->neigh_top_recon_16bit[1],
                       recon_ptr->buffer_cb + sz * (rec_cb_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cb),
                       sz * blk_geom->bwidth_uv);
            svt_memcpy(ctx->blk_ptr->neigh_top_recon_16bit[2],
                       recon_ptr->buffer_cr + sz * (rec_cr_offset + (blk_geom->bheight_uv - 1) * recon_ptr->stride_cr),
                       sz * blk_geom->bwidth_uv);
        }

        // Copy right column (used for intra pred of the right block)
        for (uint32_t j = 0; j < blk_geom->bheight; ++j) {
            ctx->blk_ptr->neigh_left_recon_16bit[0][j] =
                ((uint16_t*)recon_ptr->buffer_y)[rec_luma_offset + blk_geom->bwidth - 1 + j * recon_ptr->stride_y];
        }

        if (ctx->has_uv && ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
            for (uint32_t j = 0; j < blk_geom->bheight_uv; ++j) {
                ctx->blk_ptr->neigh_left_recon_16bit[1][j] =
                    ((uint16_t*)
                         recon_ptr->buffer_cb)[rec_cb_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cb];
                ctx->blk_ptr->neigh_left_recon_16bit[2][j] =
                    ((uint16_t*)
                         recon_ptr->buffer_cr)[rec_cr_offset + blk_geom->bwidth_uv - 1 + j * recon_ptr->stride_cr];
            }
        }
    } // END RECON COPIES
}

// Since light-PD1 uses pred_depth_only, the recon pixels can be copied directly to the recon buffer (no need
// to copy to a temp buffer and copy after d2 decision)
static void copy_recon_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                 ModeDecisionCandidateBuffer* cand_bf) {
    const uint32_t blk_org_x       = ctx->blk_org_x;
    const uint32_t blk_org_y       = ctx->blk_org_y;
    const uint32_t bwidth          = ctx->blk_geom->bwidth;
    const uint32_t bheight         = ctx->blk_geom->bheight;
    const uint32_t blk_origin_x_uv = ctx->round_origin_x >> 1;
    const uint32_t blk_origin_y_uv = ctx->round_origin_y >> 1;
    const uint32_t bwidth_uv       = ctx->blk_geom->bwidth_uv;
    const uint32_t bheight_uv      = ctx->blk_geom->bheight_uv;

    //copy neigh recon data in blk_ptr
    uint32_t             j;
    EbPictureBufferDesc* recon_ptr;
    uint32_t             rec_luma_offset;
    uint32_t             rec_cb_offset;
    uint32_t             rec_cr_offset;

    // If bypassing MD, recon is stored in different buffer; need to update the buffer to copy from
    if (ctx->bypass_encdec) {
        // If using only pred depth and no NSQ, can copy directly to final buffer b/c no d1 or d2
        // decision Assume non-16bit
        recon_ptr       = (pcs->ppcs->is_ref)
                  ? ((EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr)->reference_picture
                  : pcs->ppcs->enc_dec_ptr->recon_pic;
        rec_luma_offset = (recon_ptr->org_y + blk_org_y) * recon_ptr->stride_y + (recon_ptr->org_x + blk_org_x);
        rec_cb_offset   = rec_cr_offset =
            ((blk_org_x + recon_ptr->org_x + (blk_org_y + recon_ptr->org_y) * recon_ptr->stride_cb) >> 1);

    } else {
        recon_ptr       = cand_bf->recon;
        rec_luma_offset = 0;
        rec_cb_offset   = 0;
        rec_cr_offset   = 0;
    }

    // Y
    // Copy top and bottom rows
    uint8_t* dst_ptr_top_left = ctx->recon_neigh_y->top_array +
        get_neighbor_array_unit_top_index(ctx->recon_neigh_y, blk_org_x) * ctx->recon_neigh_y->unit_size;
    uint8_t* dst_ptr_bot_right = ctx->recon_neigh_y->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(ctx->recon_neigh_y, blk_org_x, blk_org_y + (bheight - 1)) *
            ctx->recon_neigh_y->unit_size;
    uint8_t* src_ptr = recon_ptr->buffer_y + rec_luma_offset + (bheight - 1) * recon_ptr->stride_y;
    svt_memcpy(dst_ptr_top_left, src_ptr, bwidth);
    svt_memcpy(dst_ptr_bot_right, src_ptr, bwidth);

    // Copy right and left columns
    dst_ptr_top_left = ctx->recon_neigh_y->left_array +
        get_neighbor_array_unit_left_index(ctx->recon_neigh_y, blk_org_y) * ctx->recon_neigh_y->unit_size;
    dst_ptr_bot_right = ctx->recon_neigh_y->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(ctx->recon_neigh_y, blk_org_x + (bwidth - 1), blk_org_y) *
            ctx->recon_neigh_y->unit_size;
    src_ptr = recon_ptr->buffer_y + rec_luma_offset + bwidth - 1;
    for (j = 0; j < bheight; ++j) {
        *dst_ptr_bot_right = dst_ptr_top_left[j] = src_ptr[j * recon_ptr->stride_y];
        dst_ptr_bot_right -= 1;
    }

    // Cb
    // Copy top and bottom rows
    dst_ptr_top_left = ctx->recon_neigh_cb->top_array +
        get_neighbor_array_unit_top_index(ctx->recon_neigh_cb, blk_origin_x_uv) * ctx->recon_neigh_cb->unit_size;
    dst_ptr_bot_right = ctx->recon_neigh_cb->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(
            ctx->recon_neigh_cb, blk_origin_x_uv, blk_origin_y_uv + (bheight_uv - 1)) *
            ctx->recon_neigh_cb->unit_size;
    src_ptr = recon_ptr->buffer_cb + rec_cb_offset + (bheight_uv - 1) * recon_ptr->stride_cb;
    svt_memcpy(dst_ptr_top_left, src_ptr, bwidth_uv);
    svt_memcpy(dst_ptr_bot_right, src_ptr, bwidth_uv);

    // Copy right and left columns
    dst_ptr_top_left = ctx->recon_neigh_cb->left_array +
        get_neighbor_array_unit_left_index(ctx->recon_neigh_cb, blk_origin_y_uv) * ctx->recon_neigh_cb->unit_size;
    dst_ptr_bot_right = ctx->recon_neigh_cb->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(
            ctx->recon_neigh_cb, blk_origin_x_uv + (bwidth_uv - 1), blk_origin_y_uv) *
            ctx->recon_neigh_cb->unit_size;
    src_ptr = recon_ptr->buffer_cb + rec_cb_offset + bwidth_uv - 1;
    for (j = 0; j < bheight_uv; ++j) {
        *dst_ptr_bot_right = dst_ptr_top_left[j] = src_ptr[j * recon_ptr->stride_cb];
        dst_ptr_bot_right -= 1;
    }

    // Cr
    // Copy top and bottom rows
    dst_ptr_top_left = ctx->recon_neigh_cr->top_array +
        get_neighbor_array_unit_top_index(ctx->recon_neigh_cr, blk_origin_x_uv) * ctx->recon_neigh_cr->unit_size;
    dst_ptr_bot_right = ctx->recon_neigh_cr->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(
            ctx->recon_neigh_cr, blk_origin_x_uv, blk_origin_y_uv + (bheight_uv - 1)) *
            ctx->recon_neigh_cr->unit_size;
    src_ptr = recon_ptr->buffer_cr + rec_cr_offset + (bheight_uv - 1) * recon_ptr->stride_cr;
    svt_memcpy(dst_ptr_top_left, src_ptr, bwidth_uv);
    svt_memcpy(dst_ptr_bot_right, src_ptr, bwidth_uv);

    // Copy right and left columns
    dst_ptr_top_left = ctx->recon_neigh_cr->left_array +
        get_neighbor_array_unit_left_index(ctx->recon_neigh_cr, blk_origin_y_uv) * ctx->recon_neigh_cr->unit_size;
    dst_ptr_bot_right = ctx->recon_neigh_cr->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(
            ctx->recon_neigh_cr, blk_origin_x_uv + (bwidth_uv - 1), blk_origin_y_uv) *
            ctx->recon_neigh_cr->unit_size;
    src_ptr = recon_ptr->buffer_cr + rec_cr_offset + bwidth_uv - 1;
    for (j = 0; j < bheight_uv; ++j) {
        *dst_ptr_bot_right = dst_ptr_top_left[j] = src_ptr[j * recon_ptr->stride_cr];
        dst_ptr_bot_right -= 1;
    }

    // If bypassing EncDec for 10bit, need to save 8bit and 10bit recon
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec) {
        svt_aom_get_recon_pic(pcs, &recon_ptr, 1);
        // Y
        // Copy top and bottom rows
        uint16_t* dst_ptr_top_left_16bit = (uint16_t*)(ctx->luma_recon_na_16bit->top_array +
                                                       get_neighbor_array_unit_top_index(ctx->luma_recon_na_16bit,
                                                                                         blk_org_x) *
                                                           ctx->luma_recon_na_16bit->unit_size);
        uint16_t* dst_ptr_bot_right_16bit =
            (uint16_t*)(ctx->luma_recon_na_16bit->top_left_array +
                        svt_aom_get_neighbor_array_unit_top_left_index(
                            ctx->luma_recon_na_16bit, blk_org_x, blk_org_y + (bheight - 1)) *
                            ctx->luma_recon_na_16bit->unit_size);
        uint16_t* src_ptr_16bit = ((uint16_t*)recon_ptr->buffer_y) + rec_luma_offset +
            (bheight - 1) * recon_ptr->stride_y;
        svt_memcpy(dst_ptr_top_left_16bit, src_ptr_16bit, bwidth * sizeof(uint16_t));
        svt_memcpy(dst_ptr_bot_right_16bit, src_ptr_16bit, bwidth * sizeof(uint16_t));

        // Copy right and left columns
        dst_ptr_top_left_16bit  = (uint16_t*)(ctx->luma_recon_na_16bit->left_array +
                                             get_neighbor_array_unit_left_index(ctx->luma_recon_na_16bit, blk_org_y) *
                                                 ctx->luma_recon_na_16bit->unit_size);
        dst_ptr_bot_right_16bit = (uint16_t*)(ctx->luma_recon_na_16bit->top_left_array +
                                              svt_aom_get_neighbor_array_unit_top_left_index(
                                                  ctx->luma_recon_na_16bit, blk_org_x + (bwidth - 1), blk_org_y) *
                                                  ctx->luma_recon_na_16bit->unit_size);
        src_ptr_16bit           = ((uint16_t*)recon_ptr->buffer_y) + rec_luma_offset + bwidth - 1;
        for (j = 0; j < bheight; ++j) {
            *dst_ptr_bot_right_16bit = dst_ptr_top_left_16bit[j] = src_ptr_16bit[j * recon_ptr->stride_y];
            dst_ptr_bot_right_16bit -= 1;
        }

        // Cb
        // Copy top and bottom rows
        dst_ptr_top_left_16bit = (uint16_t*)(ctx->cb_recon_na_16bit->top_array +
                                             get_neighbor_array_unit_top_index(ctx->cb_recon_na_16bit,
                                                                               blk_origin_x_uv) *
                                                 ctx->cb_recon_na_16bit->unit_size);
        dst_ptr_bot_right_16bit =
            (uint16_t*)(ctx->cb_recon_na_16bit->top_left_array +
                        svt_aom_get_neighbor_array_unit_top_left_index(
                            ctx->cb_recon_na_16bit, blk_origin_x_uv, blk_origin_y_uv + (bheight_uv - 1)) *
                            ctx->cb_recon_na_16bit->unit_size);
        src_ptr_16bit = ((uint16_t*)recon_ptr->buffer_cb) + rec_cb_offset + (bheight_uv - 1) * recon_ptr->stride_cb;
        svt_memcpy(dst_ptr_top_left_16bit, src_ptr_16bit, bwidth_uv * sizeof(uint16_t));
        svt_memcpy(dst_ptr_bot_right_16bit, src_ptr_16bit, bwidth_uv * sizeof(uint16_t));

        // Copy right and left columns
        dst_ptr_top_left_16bit = (uint16_t*)(ctx->cb_recon_na_16bit->left_array +
                                             get_neighbor_array_unit_left_index(ctx->cb_recon_na_16bit,
                                                                                blk_origin_y_uv) *
                                                 ctx->cb_recon_na_16bit->unit_size);
        dst_ptr_bot_right_16bit =
            (uint16_t*)(ctx->cb_recon_na_16bit->top_left_array +
                        svt_aom_get_neighbor_array_unit_top_left_index(
                            ctx->cb_recon_na_16bit, blk_origin_x_uv + (bwidth_uv - 1), blk_origin_y_uv) *
                            ctx->cb_recon_na_16bit->unit_size);
        src_ptr_16bit = ((uint16_t*)recon_ptr->buffer_cb) + rec_cb_offset + bwidth_uv - 1;
        for (j = 0; j < bheight_uv; ++j) {
            *dst_ptr_bot_right_16bit = dst_ptr_top_left_16bit[j] = src_ptr_16bit[j * recon_ptr->stride_cb];
            dst_ptr_bot_right_16bit -= 1;
        }

        // Cr
        // Copy top and bottom rows
        dst_ptr_top_left_16bit = (uint16_t*)(ctx->cr_recon_na_16bit->top_array +
                                             get_neighbor_array_unit_top_index(ctx->cr_recon_na_16bit,
                                                                               blk_origin_x_uv) *
                                                 ctx->cr_recon_na_16bit->unit_size);
        dst_ptr_bot_right_16bit =
            (uint16_t*)(ctx->cr_recon_na_16bit->top_left_array +
                        svt_aom_get_neighbor_array_unit_top_left_index(
                            ctx->cr_recon_na_16bit, blk_origin_x_uv, blk_origin_y_uv + (bheight_uv - 1)) *
                            ctx->cr_recon_na_16bit->unit_size);
        src_ptr_16bit = ((uint16_t*)recon_ptr->buffer_cr) + rec_cr_offset + (bheight_uv - 1) * recon_ptr->stride_cr;
        svt_memcpy(dst_ptr_top_left_16bit, src_ptr_16bit, bwidth_uv * sizeof(uint16_t));
        svt_memcpy(dst_ptr_bot_right_16bit, src_ptr_16bit, bwidth_uv * sizeof(uint16_t));

        // Copy right and left columns
        dst_ptr_top_left_16bit = (uint16_t*)(ctx->cr_recon_na_16bit->left_array +
                                             get_neighbor_array_unit_left_index(ctx->cr_recon_na_16bit,
                                                                                blk_origin_y_uv) *
                                                 ctx->cr_recon_na_16bit->unit_size);
        dst_ptr_bot_right_16bit =
            (uint16_t*)(ctx->cr_recon_na_16bit->top_left_array +
                        svt_aom_get_neighbor_array_unit_top_left_index(
                            ctx->cr_recon_na_16bit, blk_origin_x_uv + (bwidth_uv - 1), blk_origin_y_uv) *
                            ctx->cr_recon_na_16bit->unit_size);
        src_ptr_16bit = ((uint16_t*)recon_ptr->buffer_cr) + rec_cr_offset + bwidth_uv - 1;
        for (j = 0; j < bheight_uv; ++j) {
            *dst_ptr_bot_right_16bit = dst_ptr_top_left_16bit[j] = src_ptr_16bit[j * recon_ptr->stride_cr];
            dst_ptr_bot_right_16bit -= 1;
        }
    }
}

/*
 * Convert the recon picture from 16bit to 8bit when bypassing EncDec.
 */
static void convert_md_recon_16bit_to_8bit(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EbPictureBufferDesc* recon_buffer_16bit;
    EbPictureBufferDesc* recon_buffer_8bit;
    uint32_t             pred_buf_x_offest_16bit;
    uint32_t             pred_buf_y_offest_16bit;
    uint32_t             pred_buf_x_offest_16bit_uv;
    uint32_t             pred_buf_y_offest_16bit_uv;

    // 8bit recon must be stored under the pcs because the temp storage in the blk_ptr is used by the 16bit recon.
    // The 8bit recon picture is only needed temporarily to copy the pixels for intra prediction.
    svt_aom_get_recon_pic(pcs, &recon_buffer_8bit, 0);
    uint32_t pred_buf_x_offest_8bit    = ctx->blk_org_x;
    uint32_t pred_buf_y_offest_8bit    = ctx->blk_org_y;
    uint32_t pred_buf_x_offest_8bit_uv = ROUND_UV(ctx->blk_org_x) >> 1;
    uint32_t pred_buf_y_offest_8bit_uv = ROUND_UV(ctx->blk_org_y) >> 1;

    if (ctx->fixed_partition) {
        svt_aom_get_recon_pic(pcs, &recon_buffer_16bit, 1);
        pred_buf_x_offest_16bit    = ctx->blk_org_x;
        pred_buf_y_offest_16bit    = ctx->blk_org_y;
        pred_buf_x_offest_16bit_uv = ROUND_UV(ctx->blk_org_x) >> 1;
        pred_buf_y_offest_16bit_uv = ROUND_UV(ctx->blk_org_y) >> 1;
    } else {
        recon_buffer_16bit         = ctx->blk_ptr->recon_tmp;
        pred_buf_x_offest_16bit    = 0;
        pred_buf_y_offest_16bit    = 0;
        pred_buf_x_offest_16bit_uv = 0;
        pred_buf_y_offest_16bit_uv = 0;
    }

    // Y
    uint16_t* dst_16bit = (uint16_t*)(recon_buffer_16bit->buffer_y) + pred_buf_x_offest_16bit +
        recon_buffer_16bit->org_x +
        (pred_buf_y_offest_16bit + recon_buffer_16bit->org_y) * recon_buffer_16bit->stride_y;

    int32_t dst_stride_16bit = recon_buffer_16bit->stride_y;

    uint8_t* dst;
    int32_t  dst_stride;

    dst = recon_buffer_8bit->buffer_y + pred_buf_x_offest_8bit + recon_buffer_8bit->org_x +
        (pred_buf_y_offest_8bit + recon_buffer_8bit->org_y) * recon_buffer_8bit->stride_y;
    dst_stride = recon_buffer_8bit->stride_y;

    uint8_t* dst_nbit        = recon_buffer_8bit->buffer_bit_inc_y
               ? recon_buffer_8bit->buffer_bit_inc_y + pred_buf_x_offest_8bit + recon_buffer_8bit->org_x +
            (pred_buf_y_offest_8bit + recon_buffer_8bit->org_y) * recon_buffer_8bit->stride_y
               : recon_buffer_8bit->buffer_bit_inc_y;
    int32_t  dst_nbit_stride = recon_buffer_8bit->stride_bit_inc_y;

    svt_aom_un_pack2d(dst_16bit,
                      dst_stride_16bit,
                      dst,
                      dst_stride,
                      dst_nbit,
                      dst_nbit_stride,
                      ctx->blk_geom->bwidth,
                      ctx->blk_geom->bheight);
    // CB
    dst_16bit = (uint16_t*)(recon_buffer_16bit->buffer_cb) + pred_buf_x_offest_16bit_uv +
        recon_buffer_16bit->org_x / 2 +
        (pred_buf_y_offest_16bit_uv + recon_buffer_16bit->org_y / 2) * recon_buffer_16bit->stride_cb;
    dst_stride_16bit = recon_buffer_16bit->stride_cb;

    dst = recon_buffer_8bit->buffer_cb + pred_buf_x_offest_8bit_uv + recon_buffer_8bit->org_x / 2 +
        (pred_buf_y_offest_8bit_uv + recon_buffer_8bit->org_y / 2) * recon_buffer_8bit->stride_cb;
    dst_stride = recon_buffer_8bit->stride_cb;

    dst_nbit        = recon_buffer_8bit->buffer_bit_inc_cb
               ? recon_buffer_8bit->buffer_bit_inc_cb + pred_buf_x_offest_8bit_uv + recon_buffer_8bit->org_x / 2 +
            (pred_buf_y_offest_8bit_uv + recon_buffer_8bit->org_y / 2) * recon_buffer_8bit->stride_cb
               : recon_buffer_8bit->buffer_bit_inc_cb;
    dst_nbit_stride = recon_buffer_8bit->stride_bit_inc_cb;

    svt_aom_un_pack2d(dst_16bit,
                      dst_stride_16bit,
                      dst,
                      dst_stride,
                      dst_nbit,
                      dst_nbit_stride,
                      ctx->blk_geom->bwidth_uv,
                      ctx->blk_geom->bheight_uv);

    // CR
    dst_16bit = (uint16_t*)(recon_buffer_16bit->buffer_cr) +
        (pred_buf_x_offest_16bit_uv + recon_buffer_16bit->org_x / 2 +
         (pred_buf_y_offest_16bit_uv + recon_buffer_16bit->org_y / 2) * recon_buffer_16bit->stride_cr);
    dst_stride_16bit = recon_buffer_16bit->stride_cr;

    dst = recon_buffer_8bit->buffer_cr + pred_buf_x_offest_8bit_uv + recon_buffer_8bit->org_x / 2 +
        (pred_buf_y_offest_8bit_uv + recon_buffer_8bit->org_y / 2) * recon_buffer_8bit->stride_cr;
    dst_stride = recon_buffer_8bit->stride_cr;

    dst_nbit        = recon_buffer_8bit->buffer_bit_inc_cr
               ? recon_buffer_8bit->buffer_bit_inc_cr + pred_buf_x_offest_8bit_uv + recon_buffer_8bit->org_x / 2 +
            (pred_buf_y_offest_8bit_uv + recon_buffer_8bit->org_y / 2) * recon_buffer_8bit->stride_cr
               : recon_buffer_8bit->buffer_bit_inc_cr;
    dst_nbit_stride = recon_buffer_8bit->stride_bit_inc_cr;

    svt_aom_un_pack2d(dst_16bit,
                      dst_stride_16bit,
                      dst,
                      dst_stride,
                      dst_nbit,
                      dst_nbit_stride,
                      ctx->blk_geom->bwidth_uv,
                      ctx->blk_geom->bheight_uv);
}

// Check if reference frame pair of the current block matches with the given
// block.
static INLINE int match_ref_frame_pair(const BlockModeInfo* mbmi, const MvReferenceFrame* ref_frames) {
    return ((ref_frames[0] == mbmi->ref_frame[0]) && (ref_frames[1] == mbmi->ref_frame[1]));
}

static void lpd1_tx_shortcut_detector(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer** cand_bf_ptr_array) {
    const BlockGeom*             blk_geom           = ctx->blk_geom;
    const ModeDecisionCandidate* cand               = cand_bf_ptr_array[ctx->mds0_best_idx]->cand;
    const uint64_t               best_md_stage_dist = cand_bf_ptr_array[ctx->mds0_best_idx]->luma_fast_dist;
    const uint32_t               th_normalizer      = blk_geom->bheight * blk_geom->bwidth * ctx->qp_index;
    ctx->use_tx_shortcuts_mds3                      = (100 * best_md_stage_dist) <
        (ctx->lpd1_tx_ctrls.use_mds3_shortcuts_th * th_normalizer);
    ctx->lpd1_allow_skipping_tx = (100 * best_md_stage_dist) < (ctx->lpd1_tx_ctrls.skip_tx_th * th_normalizer);

    if (ctx->lpd1_tx_ctrls.use_neighbour_info && ctx->depth_removal_ctrls.enabled &&
        ctx->depth_removal_ctrls.disallow_below_64x64) {
        ctx->use_tx_shortcuts_mds3 = 1;
    }

    if ((!ctx->use_tx_shortcuts_mds3 || !ctx->lpd1_allow_skipping_tx) && ctx->lpd1_tx_ctrls.use_neighbour_info &&
        ctx->is_subres_safe) {
        MacroBlockD* xd = ctx->blk_ptr->av1xd;
        if (xd->left_available && xd->up_available) {
            const BlockModeInfo* const left_mi  = &xd->left_mbmi->block_mi;
            const BlockModeInfo* const above_mi = &xd->above_mbmi->block_mi;
            if (left_mi->skip && above_mi->skip) {
                MvReferenceFrame rf[2]                    = {cand->block_mi.ref_frame[0], cand->block_mi.ref_frame[1]};
                int              num_ref_frame_pair_match = match_ref_frame_pair(left_mi, rf);
                num_ref_frame_pair_match += match_ref_frame_pair(above_mi, rf);

                uint16_t use_tx_shortcuts_mds3_mult  = 2 * ctx->lpd1_tx_ctrls.use_neighbour_info,
                         lpd1_allow_skipping_tx_mult = use_tx_shortcuts_mds3_mult; // is halved below

                if (num_ref_frame_pair_match == 2) {
                    if (left_mi->mode == cand->block_mi.mode && above_mi->mode == cand->block_mi.mode) {
                        use_tx_shortcuts_mds3_mult  = 4;
                        lpd1_allow_skipping_tx_mult = 3;
                        if (is_inter_mode(cand->block_mi.mode)) {
                            if (rf[1] > INTRA_FRAME) { // BI_PRED
                                if (left_mi->mv[0].as_int == cand->block_mi.mv[0].as_int &&
                                    left_mi->mv[1].as_int == cand->block_mi.mv[1].as_int &&
                                    above_mi->mv[0].as_int == cand->block_mi.mv[0].as_int &&
                                    above_mi->mv[1].as_int == cand->block_mi.mv[1].as_int) {
                                    use_tx_shortcuts_mds3_mult  = 6;
                                    lpd1_allow_skipping_tx_mult = 4;
                                }
                            } else {
                                // unipred MVs stored in idx0
                                if (left_mi->mv[0].as_int == cand->block_mi.mv[0].as_int &&
                                    above_mi->mv[0].as_int == cand->block_mi.mv[0].as_int) {
                                    use_tx_shortcuts_mds3_mult  = 6;
                                    lpd1_allow_skipping_tx_mult = 4;
                                }
                            }
                        }
                    }
                }
                ctx->use_tx_shortcuts_mds3  = (100 * best_md_stage_dist) <
                        (((use_tx_shortcuts_mds3_mult * ctx->lpd1_tx_ctrls.use_mds3_shortcuts_th) >> 1) * th_normalizer)
                     ? 1
                     : ctx->use_tx_shortcuts_mds3;
                ctx->lpd1_allow_skipping_tx = (100 * best_md_stage_dist) <
                        (((lpd1_allow_skipping_tx_mult * ctx->lpd1_tx_ctrls.skip_tx_th) >> 1) * th_normalizer)
                    ? 1
                    : ctx->lpd1_allow_skipping_tx;
            }
        }
    }
}

static void md_encode_block_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                      EbPictureBufferDesc* input_pic) {
    ModeDecisionCandidateBuffer** cand_bf_ptr_array_base = ctx->cand_bf_ptr_array;
    ModeDecisionCandidateBuffer** cand_bf_ptr_array;
    const BlockGeom*              blk_geom = ctx->blk_geom;
    ModeDecisionCandidateBuffer*  cand_bf;
    ModeDecisionCandidate*        fast_cand_array = ctx->fast_cand_array;
    uint32_t                      fast_candidate_total_count;

    BlockLocation loc;
    loc.input_origin_index = ctx->blk_org_x + input_pic->org_x +
        (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y;
    loc.input_cb_origin_in_index = ((ctx->blk_org_x + input_pic->org_x) >> 1) +
        ((ctx->blk_org_y + input_pic->org_y) >> 1) * input_pic->stride_cb;

    BlkStruct* blk_ptr     = ctx->blk_ptr;
    cand_bf_ptr_array      = &(cand_bf_ptr_array_base[0]);
    ctx->blk_lambda_tuning = pcs->ppcs->blk_lambda_tuning;
    if (pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
        SuperBlock*    sb_ptr  = ctx->sb_ptr;
        const Position blk_org = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
        svt_aom_apply_segmentation_based_quantization(pcs, sb_ptr, blk_ptr, blk_geom->bsize, blk_org.x, blk_org.y);
    }
    //Get the new lambda for current block
    if (pcs->ppcs->blk_lambda_tuning) {
        svt_aom_set_tuned_blk_lambda(ctx, pcs);
    } else if (pcs->ppcs->scs->static_config.tune == TUNE_SSIM || pcs->ppcs->scs->static_config.tune == TUNE_IQ ||
               pcs->ppcs->scs->static_config.tune == TUNE_MS_SSIM) {
        int mi_row = ctx->blk_org_y / 4;
        int mi_col = ctx->blk_org_x / 4;
        aom_av1_set_ssim_rdmult(ctx, pcs, mi_row, mi_col);
    }

    // need to init xd before product_coding_loop_init_fast_loop()
    svt_aom_init_xd(pcs, ctx);
    ctx->me_sb_addr      = ctx->sb_ptr->index;
    ctx->me_block_offset = svt_aom_get_me_block_offset(
        ctx->blk_org_x, ctx->blk_org_y, ctx->blk_geom->bsize, pcs->ppcs->enable_me_8x8, pcs->ppcs->enable_me_16x16);

    // derive me offsets
    ctx->geom_offset_x  = 0;
    ctx->geom_offset_y  = 0;
    ctx->me_cand_offset = ctx->me_block_offset * pcs->ppcs->pa_me_data->max_cand;

    ctx->tot_ref_frame_types = pcs->ppcs->tot_ref_frame_types;
    memcpy(ctx->ref_frame_type_arr, pcs->ppcs->ref_frame_type_arr, sizeof(MvReferenceFrame) * MODE_CTX_REF_FRAMES);

    // ref_frame_type_arr is pruned in PD for S-Frame RA mode,
    // determine_best_references() may add back some ref frame types
    // skip it to avoid pruned types
    if (pcs->ppcs->scs->mrp_ctrls.use_best_references == 3 && pcs->temporal_layer_index > 0 &&
        !pcs->ppcs->sframe_ref_pruned) {
        determine_best_references(pcs, ctx, ctx->ref_frame_type_arr, &ctx->tot_ref_frame_types);
    }

    if (!ctx->shut_fast_rate && pcs->slice_type != I_SLICE) {
        svt_aom_generate_av1_mvp_table(ctx,
                                       ctx->blk_ptr,
                                       ctx->blk_geom,
                                       ctx->blk_org_x,
                                       ctx->blk_org_y,
                                       ctx->ref_frame_type_arr,
                                       ctx->tot_ref_frame_types,
                                       pcs);
    }
    product_coding_loop_init_fast_loop(pcs, ctx);
    ctx->is_intra_bordered = ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0;
    //mvp array is not constructed in LPD1. reset to zero.
    memset(ctx->mvp_count, 0, sizeof(ctx->mvp_count));
    // Read and (if needed) perform 1/8 Pel ME MVs refinement
    if (pcs->slice_type != I_SLICE) {
        read_refine_me_mvs_light_pd1(pcs, input_pic, ctx);
    }
    generate_md_stage_0_cand_light_pd1(ctx, &fast_candidate_total_count, pcs);
    if (pcs->slice_type != I_SLICE && ctx->approx_inter_rate < 2) {
        if (!ctx->shut_fast_rate) {
            estimate_ref_frames_num_bits(ctx, pcs);
        }
    }

    ctx->md_stage       = MD_STAGE_0;
    ctx->mds0_best_idx  = 0;
    ctx->mds0_best_cost = (uint64_t)~0;

    uint8_t perform_md_recon = svt_aom_do_md_recon(pcs->ppcs, ctx);

    // If there is only a single candidate, skip compensation if transform will be skipped (unless compensation is needed for recon)
    if (fast_candidate_total_count > 1 || perform_md_recon || ctx->lpd1_skip_inter_tx_level < 2 ||
        is_intra_mode(fast_cand_array[0].block_mi.mode)) {
        md_stage_0_light_pd1(
            pcs, ctx, cand_bf_ptr_array_base, fast_cand_array, fast_candidate_total_count, input_pic, &loc);

        ctx->perform_mds1 = 0;
        lpd1_tx_shortcut_detector(ctx, cand_bf_ptr_array);
        // Condition needed in order to avoid mismatch between recon flag ON/OFF when lpd1_skip_inter_tx_level == 2
        if (fast_candidate_total_count == 1 && ctx->lpd1_skip_inter_tx_level == 2 &&
            !is_intra_mode(fast_cand_array[0].block_mi.mode)) {
            ctx->use_tx_shortcuts_mds3  = 1;
            ctx->lpd1_allow_skipping_tx = 1;
        }
    } else {
        cand_bf                   = cand_bf_ptr_array_base[0];
        cand_bf->cand             = &fast_cand_array[0];
        *(cand_bf->fast_cost)     = 0;
        cand_bf->fast_luma_rate   = 0;
        cand_bf->fast_chroma_rate = 0;

        /* If the interpolation filter type is assigned at the picture level, use that value, OW use regular.
         * NB intra_bc always uses BILINEAR, but IBC is not allowed in LPD1. */
        cand_bf->cand->block_mi.interp_filters = (pcs->ppcs->frm_hdr.interpolation_filter == SWITCHABLE)
            ? 0
            : av1_broadcast_interp_filter(pcs->ppcs->frm_hdr.interpolation_filter);
        cand_bf->cand->block_mi.tx_depth       = 0;
        ctx->use_tx_shortcuts_mds3             = 1;
        ctx->lpd1_allow_skipping_tx            = 1;
    }
    // For 10bit content, when recon is not needed, hbd_md can stay =0,
    // and the 8bit prediction is used to produce the residual (with 8bit source).
    // When recon is needed, the prediction must be re-done in 10bit,
    // and the residual will be generated with the 10bit pred and 10bit source

    // Using the 8bit residual for the TX will cause different streams compared to using the 10bit residual.
    // To generate the same streams, compute the 10bit prediction before computing the recon

    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && perform_md_recon) {
        ctx->hbd_md = 2;

        // Update input pic and offsets
        input_pic              = pcs->input_frame16bit;
        loc.input_origin_index = ctx->blk_org_x + input_pic->org_x +
            (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y;
        loc.input_cb_origin_in_index = ((ctx->blk_org_x + input_pic->org_x) >> 1) +
            ((ctx->blk_org_y + input_pic->org_y) >> 1) * input_pic->stride_cb;
    }
    ctx->md_stage = MD_STAGE_3;
    md_stage_3_light_pd1(pcs, ctx, input_pic, &loc);
    cand_bf = cand_bf_ptr_array[ctx->mds0_best_idx];

    // Full Mode Decision (choose the best mode)
    svt_aom_product_full_mode_decision_light_pd1(pcs, ctx, cand_bf);
    // Perform inverse transform recon when needed
    if (perform_md_recon) {
        av1_perform_inverse_transform_recon(pcs, ctx, cand_bf, ctx->blk_geom);
    }

    // Convert 10bit recon (used as final EncDec recon) to 8bit recon (used for MD intra pred)
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && ctx->hbd_md) {
        if (!ctx->skip_intra) {
            convert_md_recon_16bit_to_8bit(pcs, ctx);
        }
        ctx->hbd_md = 0;
    }
    if (!ctx->skip_intra) {
        copy_recon_light_pd1(pcs, ctx, cand_bf);
    }
}

static void tx_shortcut_detector(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer** cand_bf_ptr_array) {
    const BlockGeom* blk_geom           = ctx->blk_geom;
    const uint64_t   best_md_stage_dist = cand_bf_ptr_array[ctx->mds0_best_idx]->luma_fast_dist;
    const uint32_t   th_normalizer      = blk_geom->bheight * blk_geom->bwidth * ctx->qp_index;
    ctx->use_tx_shortcuts_mds3          = (100 * best_md_stage_dist) <
        (ctx->tx_shortcut_ctrls.use_mds3_shortcuts_th * th_normalizer);
}

static void non_normative_txs(PictureControlSet* pcs, ModeDecisionContext* ctx, BlkStruct* blk_ptr,
                              ModeDecisionCandidateBuffer* cand_bf) {
    // That's a non-conformant tx-partitioning
    ctx->min_nz_h = (uint16_t)~0;
    ctx->min_nz_v = (uint16_t)~0;

    if (cand_bf->block_has_coeff) {
        const bool is_inter = (is_inter_mode(cand_bf->cand->block_mi.mode) || cand_bf->cand->block_mi.use_intrabc)
            ? true
            : false;

        {
            // 2 * Tx-2NxN
            ctx->txb_1d_offset  = 0;
            TxSize    tx_size   = tx_depth_to_tx_size[0][ctx->blk_geom->bsize];
            const int txbwidth  = tx_size_wide[tx_size];
            const int txbheight = tx_size_high[tx_size] >> 1;
            if (tx_size == TX_64X64) {
                tx_size = TX_64X32;
            } else if (tx_size == TX_32X32) {
                tx_size = TX_32X16;
            } else if (tx_size == TX_16X16) {
                tx_size = TX_16X8;
            } else if (tx_size == TX_8X8) {
                tx_size = TX_8X4;
            } else {
                assert(0);
            }

            // Transform Loop
            for (int h_part = 0; h_part < 2; h_part++) {
                uint16_t txb_origin_x = tx_org[ctx->blk_geom->bsize][is_inter][0][0].x;
                uint16_t txb_origin_y = tx_org[ctx->blk_geom->bsize][is_inter][0][0].y + txbheight * h_part;

                uint32_t txb_origin_index = txb_origin_x + (txb_origin_y * cand_bf->residual->stride_y);

                EbPictureBufferDesc* recon_coeff_ptr = cand_bf->rec_coeff;
                EbPictureBufferDesc* quant_coeff_ptr = cand_bf->quant;

                ctx->three_quad_energy = 0;
                svt_aom_estimate_transform(pcs,
                                           ctx,
                                           &(((int16_t*)cand_bf->residual->buffer_y)[txb_origin_index]),
                                           cand_bf->residual->stride_y,
                                           &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                                           NOT_USED_VALUE,
                                           tx_size,
                                           &ctx->three_quad_energy,
                                           ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                           DCT_DCT,
                                           PLANE_TYPE_Y,
                                           DEFAULT_SHAPE);

                uint16_t eob_txt = 0;
                svt_aom_quantize_inv_quantize_light(pcs,
                                                    &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                                                    &(((int32_t*)quant_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                                                    &(((int32_t*)recon_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                                                    blk_ptr->qindex,
                                                    tx_size,
                                                    &eob_txt,
                                                    ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                                    DCT_DCT);

                ctx->txb_1d_offset += txbwidth * txbheight;
                ctx->min_nz_h = MIN((uint16_t)eob_txt, ctx->min_nz_h);
            }
        }

        {
            // 2 * Tx-2NxN
            ctx->txb_1d_offset  = 0;
            TxSize    tx_size   = tx_depth_to_tx_size[0][ctx->blk_geom->bsize];
            const int txbwidth  = tx_size_wide[tx_size] >> 1;
            const int txbheight = tx_size_high[tx_size];
            if (tx_size == TX_64X64) {
                tx_size = TX_32X64;
            } else if (tx_size == TX_32X32) {
                tx_size = TX_16X32;
            } else if (tx_size == TX_16X16) {
                tx_size = TX_8X16;
            } else if (tx_size == TX_8X8) {
                tx_size = TX_4X8;
            } else {
                assert(0);
            }

            // Transform Loop
            for (int v_part = 0; v_part < 2; v_part++) {
                uint16_t txb_origin_x = tx_org[ctx->blk_geom->bsize][is_inter][0][0].x + txbwidth * v_part;
                uint16_t txb_origin_y = tx_org[ctx->blk_geom->bsize][is_inter][0][0].y;

                uint32_t txb_origin_index = txb_origin_x + (txb_origin_y * cand_bf->residual->stride_y);

                EbPictureBufferDesc* recon_coeff_ptr = cand_bf->rec_coeff;
                EbPictureBufferDesc* quant_coeff_ptr = cand_bf->quant;

                ctx->three_quad_energy = 0;
                svt_aom_estimate_transform(pcs,
                                           ctx,
                                           &(((int16_t*)cand_bf->residual->buffer_y)[txb_origin_index]),
                                           cand_bf->residual->stride_y,
                                           &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                                           NOT_USED_VALUE,
                                           tx_size,
                                           &ctx->three_quad_energy,
                                           ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                           DCT_DCT,
                                           PLANE_TYPE_Y,
                                           DEFAULT_SHAPE);

                uint16_t eob_txt = 0;
                svt_aom_quantize_inv_quantize_light(pcs,
                                                    &(((int32_t*)ctx->tx_coeffs->buffer_y)[ctx->txb_1d_offset]),
                                                    &(((int32_t*)quant_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                                                    &(((int32_t*)recon_coeff_ptr->buffer_y)[ctx->txb_1d_offset]),
                                                    blk_ptr->qindex,
                                                    tx_size,
                                                    &eob_txt,
                                                    ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                                                    DCT_DCT);

                ctx->txb_1d_offset += txbwidth * txbheight;
                ctx->min_nz_v = MIN((uint16_t)eob_txt, ctx->min_nz_v);
            }
        }
    }
}

//determine condition to activate the use best me references speed feature
static bool get_enable_use_best_me(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    bool enable_best_me = 0;
    if (pcs->ppcs->scs->mrp_ctrls.use_best_references && pcs->temporal_layer_index > 0) {
        if (pcs->ppcs->scs->mrp_ctrls.use_best_references == 1) {
            uint32_t b64_x            = ctx->blk_org_x / 64;
            uint32_t b64_y            = ctx->blk_org_y / 64;
            uint32_t pic_width_in_b64 = (pcs->ppcs->aligned_width + pcs->ppcs->scs->b64_size - 1) /
                pcs->ppcs->scs->b64_size;
            uint32_t b64_idx = b64_y * pic_width_in_b64 + b64_x;
            svt_aom_assert_err(b64_idx < pcs->b64_total_count, "out of range index");
            uint32_t me_8x8_dist = pcs->ppcs->me_8x8_distortion[b64_idx];
            if (me_8x8_dist > 45000) {
                enable_best_me = 1;
            }
        } else if (pcs->ppcs->scs->mrp_ctrls.use_best_references == 2) {
            if (pcs->ppcs->tpl_ctrls.enable) {
                uint8_t sb_max_list0_ref_idx, sb_max_list1_ref_idx, sb_inter_selection;
                if (get_sb_tpl_inter_stats(
                        pcs, ctx, &sb_inter_selection, &sb_max_list0_ref_idx, &sb_max_list1_ref_idx)) {
                    if (sb_max_list0_ref_idx == 0 && sb_max_list1_ref_idx == 0) {
                        enable_best_me = 1;
                    }
                }
            }
        } else {
            svt_aom_assert_err(pcs->ppcs->scs->mrp_ctrls.use_best_references == 3, "use best me err");
            enable_best_me = 1;
        }
    }

    return enable_best_me;
}

static void md_encode_block(PictureControlSet* pcs, ModeDecisionContext* ctx, const PC_TREE* const pc_tree,
                            const MdScan* const mds, EbPictureBufferDesc* input_pic) {
    ModeDecisionCandidateBuffer** cand_bf_ptr_array_base = ctx->cand_bf_ptr_array;
    ModeDecisionCandidateBuffer** cand_bf_ptr_array;
    const BlockGeom*              blk_geom = ctx->blk_geom;
    BlockLocation                 loc;
    loc.input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
        (ctx->blk_org_x + input_pic->org_x);
    loc.input_cb_origin_in_index = ((ctx->round_origin_y >> 1) + (input_pic->org_y >> 1)) * input_pic->stride_cb +
        ((ctx->round_origin_x >> 1) + (input_pic->org_x >> 1));
    BlkStruct* blk_ptr                   = ctx->blk_ptr;
    cand_bf_ptr_array                    = &(cand_bf_ptr_array_base[0]);
    ctx->blk_lambda_tuning               = pcs->ppcs->blk_lambda_tuning;
    ctx->tune_ssim_level                 = SSIM_LVL_0;
    ctx->obmc_weighted_pred_ready        = false;
    ctx->obmc_neighbor_luma_pred_ready   = false;
    ctx->obmc_neighbor_chroma_pred_ready = false;
    ctx->obmc_is_luma_neigh_10bit        = false;
    if (pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
        SuperBlock*    sb_ptr  = ctx->sb_ptr;
        const Position blk_org = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
        svt_aom_apply_segmentation_based_quantization(pcs, sb_ptr, blk_ptr, blk_geom->bsize, blk_org.x, blk_org.y);
    }
    //Get the new lambda for current block
    if (pcs->ppcs->blk_lambda_tuning) {
        svt_aom_set_tuned_blk_lambda(ctx, pcs);
    } else if (pcs->ppcs->scs->static_config.tune == TUNE_SSIM || pcs->ppcs->scs->static_config.tune == TUNE_IQ ||
               pcs->ppcs->scs->static_config.tune == TUNE_MS_SSIM) {
        int mi_row = ctx->blk_org_y / 4;
        int mi_col = ctx->blk_org_x / 4;
        aom_av1_set_ssim_rdmult(ctx, pcs, mi_row, mi_col);
    }
    ctx->tot_ref_frame_types = pcs->ppcs->tot_ref_frame_types;
    memcpy(ctx->ref_frame_type_arr, pcs->ppcs->ref_frame_type_arr, sizeof(MvReferenceFrame) * MODE_CTX_REF_FRAMES);

    derive_me_offsets(pcs->ppcs->scs, pcs, ctx);

    // ref_frame_type_arr is pruned in PD for S-Frame RA mode,
    // determine_best_references() may add back some ref frame types
    // skip it to avoid pruned types
    if (get_enable_use_best_me(pcs, ctx) && !pcs->ppcs->sframe_ref_pruned) {
        determine_best_references(pcs, ctx, ctx->ref_frame_type_arr, &ctx->tot_ref_frame_types);
    }
    svt_aom_init_xd(pcs, ctx);
    if (!ctx->shut_fast_rate) {
        FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
        // Generate MVP(s)
        if (frm_hdr->allow_intrabc) { // pcs->slice_type == I_SLICE
            MvReferenceFrame ref_frame = INTRA_FRAME;
            svt_aom_generate_av1_mvp_table(
                ctx, ctx->blk_ptr, ctx->blk_geom, ctx->blk_org_x, ctx->blk_org_y, &ref_frame, 1, pcs);
        } else if (pcs->slice_type != I_SLICE) {
            svt_aom_generate_av1_mvp_table(ctx,
                                           ctx->blk_ptr,
                                           ctx->blk_geom,
                                           ctx->blk_org_x,
                                           ctx->blk_org_y,
                                           ctx->ref_frame_type_arr,
                                           ctx->tot_ref_frame_types,
                                           pcs);
        }
    }
    product_coding_loop_init_fast_loop(pcs, ctx);

    ctx->ind_uv_avail = 0;
    // Search for the best independent intra chroma mode if search is enabled to be done before MDS0
    if (ctx->uv_ctrls.uv_mode == CHROMA_MODE_0 && !ctx->uv_ctrls.ind_uv_last_mds && ctx->blk_geom->sq_size < 128 &&
        ctx->has_uv) {
        // Set MD stage to 0 to avoid using TX shortcuts in chroma transform path that are
        // meant to be based on luma TX data, which is not available
        ctx->md_stage = MD_STAGE_0;
        search_best_independent_uv_mode(
            pcs, input_pic, loc.input_cb_origin_in_index, loc.input_cb_origin_in_index, 0, ctx);
    }
#if TUNE_STILL_IMAGE
    if (pcs->slice_type != I_SLICE) {
        ctx->is_intra_bordered  = ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx)
                                                                                                : 0;
        ctx->updated_enable_pme = ctx->md_pme_ctrls.enabled;
        ctx->updated_enable_pme = ctx->is_intra_bordered &&
                ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled
            ? 0
            : ctx->updated_enable_pme;

        // Read MVPs (rounded-up to the closest integer) for use in md_sq_motion_search() and/or predictive_me_search() and/or perform_md_reference_pruning()
        if (((ctx->md_subpel_me_ctrls.enabled && ctx->md_subpel_me_ctrls.mvp_th > 0) || ctx->md_sq_me_ctrls.enabled ||
             ctx->updated_enable_pme || ctx->ref_pruning_ctrls.enabled)) {
            build_single_ref_mvp_array(pcs, ctx);
        }
        // Read and (if needed) perform 1/8 Pel ME MVs refinement
        read_refine_me_mvs(pcs, ctx, pc_tree);

        ctx->md_pme_dist = (uint32_t)~0;
        for (uint8_t list_idx = 0; list_idx < MAX_NUM_OF_REF_PIC_LIST; list_idx++) {
            for (uint8_t ref_idx = 0; ref_idx < REF_LIST_MAX_DEPTH; ref_idx++) {
                ctx->pme_res[list_idx][ref_idx].dist = (uint32_t)~0;
            }
        }
        // Perform md reference pruning
        if (ctx->ref_pruning_ctrls.enabled) {
            perform_md_reference_pruning(pcs, ctx);
        }
        // Perform ME search around the best MVP
        if (ctx->updated_enable_pme) {
            pme_search(pcs, ctx, input_pic);
        }
        if (ctx->inter_intra_comp_ctrls.enabled && svt_aom_is_interintra_allowed_bsize(ctx->blk_geom->bsize)) {
            svt_aom_precompute_intra_pred_for_inter_intra(pcs, ctx);
        }
    }
#else
    ctx->is_intra_bordered = ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0;
    ctx->updated_enable_pme = ctx->md_pme_ctrls.enabled;
    ctx->updated_enable_pme = ctx->is_intra_bordered && ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled
        ? 0
        : ctx->updated_enable_pme;

    // Read MVPs (rounded-up to the closest integer) for use in md_sq_motion_search() and/or predictive_me_search() and/or perform_md_reference_pruning()
    if (pcs->slice_type != I_SLICE &&
        ((ctx->md_subpel_me_ctrls.enabled && ctx->md_subpel_me_ctrls.mvp_th > 0) || ctx->md_sq_me_ctrls.enabled ||
         ctx->updated_enable_pme || ctx->ref_pruning_ctrls.enabled)) {
        build_single_ref_mvp_array(pcs, ctx);
    }
    if (pcs->slice_type != I_SLICE) {
        // Read and (if needed) perform 1/8 Pel ME MVs refinement
        read_refine_me_mvs(pcs, ctx, pc_tree);
    }
    ctx->md_pme_dist = (uint32_t)~0;
    for (uint8_t list_idx = 0; list_idx < MAX_NUM_OF_REF_PIC_LIST; list_idx++) {
        for (uint8_t ref_idx = 0; ref_idx < REF_LIST_MAX_DEPTH; ref_idx++) {
            ctx->pme_res[list_idx][ref_idx].dist = (uint32_t)~0;
        }
    }
    // Perform md reference pruning
    if (ctx->ref_pruning_ctrls.enabled) {
        perform_md_reference_pruning(pcs, ctx);
    }
    // Perform ME search around the best MVP
    if (ctx->updated_enable_pme) {
        pme_search(pcs, ctx, input_pic);
    }
    if (ctx->inter_intra_comp_ctrls.enabled && svt_aom_is_interintra_allowed_bsize(ctx->blk_geom->bsize)) {
        svt_aom_precompute_intra_pred_for_inter_intra(pcs, ctx);
    }
#endif
    uint32_t fast_candidate_total_count;
    ctx->md_stage = MD_STAGE_0;
    generate_md_stage_0_cand(pcs, ctx, &fast_candidate_total_count);
    if (pcs->slice_type != I_SLICE && ctx->approx_inter_rate < 2) {
        if (!ctx->shut_fast_rate) {
            estimate_ref_frames_num_bits(ctx, pcs);
        }
    }
    CandClass cand_class_it;
    uint32_t  buffer_start_idx = 0;
    uint32_t  buffer_count_for_curr_class;
    uint32_t  buffer_total_count = 0;
    ctx->md_stage_1_total_count  = 0;
    ctx->md_stage_2_total_count  = 0;
    ctx->md_stage_3_total_count  = 0;
    // Derive NIC(s)
    set_md_stage_counts(pcs, ctx);
    uint64_t best_md_stage_cost = (uint64_t)~0;
    ctx->mds0_best_idx          = 0;
    ctx->mds0_best_class_it     = 0;
#if OPT_PER_BLK_INTRA // use_hadamard
    // Enable Hadamard cost at block level only when enabled for the SB
    // and when multiple fast candidates exist (no benefit for single candidate).
    ctx->mds0_use_hadamard_blk = ctx->mds0_use_hadamard_sb && fast_candidate_total_count > 1;
#endif
    ctx->mds1_best_idx          = 0;
    ctx->mds1_best_class_it     = 0;
    ctx->perform_mds1           = 1;
    ctx->use_tx_shortcuts_mds3  = 0;
    for (cand_class_it = CAND_CLASS_0; cand_class_it < CAND_CLASS_TOTAL; cand_class_it++) {
        ctx->mds0_best_cost_per_class[cand_class_it] = (uint64_t)~0;
    }
    ctx->mds0_best_cost        = (uint64_t)~0;
    ctx->mds0_best_class       = 0;
    ctx->mds0_best_class0_cost = (uint64_t)~0;
    for (cand_class_it = CAND_CLASS_0; cand_class_it < CAND_CLASS_TOTAL; cand_class_it++) {
        //number of next level candidates could not exceed number of curr level candidates
        ctx->md_stage_1_count[cand_class_it] = MIN(ctx->md_stage_0_count[cand_class_it],
                                                   ctx->md_stage_1_count[cand_class_it]);

        if (ctx->md_stage_0_count[cand_class_it] > 0 && ctx->md_stage_1_count[cand_class_it] > 0) {
            buffer_count_for_curr_class = ctx->md_stage_1_count[cand_class_it] + 1;
            buffer_total_count += buffer_count_for_curr_class;
            svt_aom_assert_err(buffer_total_count <= ctx->max_nics, "not enough cand buffers");
            //Input: md_stage_0_count[cand_class_it]  Output:  md_stage_1_count[cand_class_it]
            ctx->target_class = cand_class_it;
            md_stage_0(pcs,
                       ctx,
                       cand_bf_ptr_array_base,
                       ctx->fast_cand_array,
                       fast_candidate_total_count,
                       input_pic,
                       &loc,
                       buffer_start_idx,
                       buffer_count_for_curr_class);
            //Sort:  md_stage_1_count[cand_class_it]
            uint32_t* cand_buff_indices = ctx->cand_buff_indices[cand_class_it];
            if (ctx->md_stage_1_count[cand_class_it] == 1) {
                cand_buff_indices[0] = *(cand_bf_ptr_array[buffer_start_idx]->fast_cost) <
                        *(cand_bf_ptr_array[buffer_start_idx + 1]->fast_cost)
                    ? buffer_start_idx
                    : buffer_start_idx + 1;
            } else {
                sort_fast_cost_based_candidates(
                    ctx,
                    buffer_start_idx,
                    ctx->md_stage_1_count[cand_class_it] +
                        1, // # cands to sort. buffer_count_for_curr_class may be wrong when multiple iterations used at MDS0
                    ctx->cand_buff_indices[cand_class_it]);
            }
            if (*(ctx->cand_bf_ptr_array[cand_buff_indices[0]]->fast_cost) < best_md_stage_cost) {
                best_md_stage_cost      = *(ctx->cand_bf_ptr_array[cand_buff_indices[0]]->fast_cost);
                ctx->mds0_best_idx      = cand_buff_indices[0];
                ctx->mds0_best_class_it = cand_class_it;
            }

            buffer_start_idx += buffer_count_for_curr_class; //for next iteration.
        }
    }
    post_mds0_nic_pruning(pcs, ctx, best_md_stage_cost);
    // Use detector for applying TX shortcuts at MDS3; if MDS1 is performed, use that info to apply
    // shortcuts instead of MDS0 info

#if OPT_PER_BLK_INTRA // use_hadamard
    if (ctx->perform_mds1 == 0 && ctx->tx_shortcut_ctrls.use_mds3_shortcuts_th > 0 && !ctx->mds0_use_hadamard_blk) {
#else
    if (ctx->perform_mds1 == 0 && ctx->tx_shortcut_ctrls.use_mds3_shortcuts_th > 0 && !ctx->mds0_use_hadamard) {
#endif
        tx_shortcut_detector(ctx, cand_bf_ptr_array);
    }
    // 1st Full-Loop
    assert(IMPLIES(!ctx->perform_mds1, ctx->md_stage_1_total_count == 1));
    // If MDS1 is bypassed, don't update the best MD stage cost because the cost will be used in
    // the post-mds1 pruning, which relies on the cost.  Pruning is done on the full_cost, but if
    // MDS1 is bypassed the full_cost will be set to the fast_cost in MDS0.
    if (ctx->bypass_md_stage_1 == false) {
        best_md_stage_cost = (uint64_t)~0;
    }
    ctx->md_stage = MD_STAGE_1;
    for (cand_class_it = CAND_CLASS_0; cand_class_it < CAND_CLASS_TOTAL; cand_class_it++) {
        //number of next level candidates could not exceed number of curr level candidates
        ctx->md_stage_2_count[cand_class_it] = MIN(ctx->md_stage_1_count[cand_class_it],
                                                   ctx->md_stage_2_count[cand_class_it]);
        if (ctx->perform_mds1) {
            if (ctx->bypass_md_stage_1 == false && ctx->md_stage_1_count[cand_class_it] > 0 &&
                ctx->md_stage_2_count[cand_class_it] > 0) {
                ctx->target_class = cand_class_it;
                md_stage_1(pcs, ctx, input_pic, &loc);

                // Sort the candidates of the target class based on the 1st full loop cost

                //sort the new set of candidates
                if (ctx->md_stage_1_count[cand_class_it]) {
                    sort_full_cost_based_candidates(
                        ctx, ctx->md_stage_1_count[cand_class_it], ctx->cand_buff_indices[cand_class_it]);
                }
                uint32_t* cand_buff_indices = ctx->cand_buff_indices[cand_class_it];
                if (*(ctx->cand_bf_ptr_array[cand_buff_indices[0]]->full_cost) < best_md_stage_cost) {
                    best_md_stage_cost      = *(ctx->cand_bf_ptr_array[cand_buff_indices[0]]->full_cost);
                    ctx->mds1_best_idx      = cand_buff_indices[0];
                    ctx->mds1_best_class_it = cand_class_it;
                }
            }
        } else {
            ctx->mds1_best_idx      = ctx->mds0_best_idx;
            ctx->mds1_best_class_it = ctx->mds0_best_class_it;
        }
    }
    if (ctx->perform_mds1) {
        post_mds1_nic_pruning(pcs, ctx, best_md_stage_cost);
    }
    // 2nd Full-Loop
    // If MDS2 is bypassed, don't update the best MD stage cost because the cost will be used in
    // the post-mds2 pruning, which relies on the cost.  Pruning is done on the full_cost, which
    // is available from MDS1 even in MDS2 is bypassed.
    if (ctx->bypass_md_stage_2 == false) {
        best_md_stage_cost = (uint64_t)~0;
    }
    ctx->md_stage = MD_STAGE_2;
    for (cand_class_it = CAND_CLASS_0; cand_class_it < CAND_CLASS_TOTAL; cand_class_it++) {
        //number of next level candidates could not exceed number of curr level candidates
        ctx->md_stage_3_count[cand_class_it] = MIN(ctx->md_stage_2_count[cand_class_it],
                                                   ctx->md_stage_3_count[cand_class_it]);
        // Perform mds1 may be true when there is one candidate after MDS0, in which case, we should skip directly
        // to the final MD stage. The variable was named when MDS2 was not available.
        if (ctx->perform_mds1) {
            if (ctx->bypass_md_stage_2 == false && ctx->md_stage_2_count[cand_class_it] > 0 &&
                ctx->md_stage_3_count[cand_class_it] > 0) {
                ctx->target_class = cand_class_it;

                md_stage_2(pcs, ctx, input_pic, &loc);
                // Sort the candidates of the target class based on the 1st full loop cost

                //sort the new set of candidates
                if (ctx->md_stage_2_count[cand_class_it]) {
                    sort_full_cost_based_candidates(
                        ctx, ctx->md_stage_2_count[cand_class_it], ctx->cand_buff_indices[cand_class_it]);
                }

                uint32_t* cand_buff_indices = ctx->cand_buff_indices[cand_class_it];
                best_md_stage_cost          = MIN((*(ctx->cand_bf_ptr_array[cand_buff_indices[0]]->full_cost)),
                                         best_md_stage_cost);
            }
        }
    }

    if (ctx->perform_mds1) {
        post_mds2_nic_pruning(pcs, ctx, best_md_stage_cost);
        construct_best_sorted_arrays_md_stage_3(ctx, ctx->best_candidate_index_array);
    } else {
        ctx->md_stage_3_total_count        = 1;
        ctx->best_candidate_index_array[0] = ctx->cand_buff_indices[ctx->mds1_best_class_it][0];
    }
    assert(ctx->md_stage_3_total_count > 0);
    // Search the best independent intra chroma mode if search is to be performed before the last MD stage
    // and if the search is allowed (if there are intra candidates remaining or based on speed features).
    if (ctx->uv_ctrls.uv_mode == CHROMA_MODE_0 && ctx->uv_ctrls.ind_uv_last_mds && ctx->blk_geom->sq_size < 128 &&
        ctx->has_uv && perform_ind_uv_search_last_mds(ctx, cand_bf_ptr_array, ctx->best_candidate_index_array)) {
        if (ctx->uv_ctrls.ind_uv_last_mds == 2) {
            search_best_mds3_uv_mode(pcs,
                                     input_pic,
                                     loc.input_cb_origin_in_index,
                                     loc.input_cb_origin_in_index,
                                     0,
                                     ctx,
                                     ctx->md_stage_3_total_count);
        } else {
            search_best_independent_uv_mode(
                pcs, input_pic, loc.input_cb_origin_in_index, loc.input_cb_origin_in_index, 0, ctx);
        }
    }

    const uint8_t org_hbd          = ctx->hbd_md;
    const uint8_t perform_md_recon = svt_aom_do_md_recon(pcs->ppcs, ctx);

    // For 10bit content, when recon is not needed, hbd_md can stay =0,
    // and the 8bit prediction is used to produce the residual (with 8bit source).
    // When recon is needed, the prediction must be re-done in 10bit,
    // and the residual will be generated with the 10bit pred and 10bit source

    // Using the 8bit residual for the TX will cause different streams compared to using the 10bit residual.
    // To generate the same streams, compute the 10bit prediction before computing the recon

    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md && ctx->pd_pass == PD_PASS_1 &&
        perform_md_recon) {
        ctx->hbd_md             = 2;
        ctx->need_hbd_comp_mds3 = 1;
        ctx->scale_palette      = 1;
        // Set the new input picture and offsets
        input_pic                    = pcs->input_frame16bit;
        loc.input_cb_origin_in_index = ((ctx->round_origin_y >> 1) + (input_pic->org_y >> 1)) * input_pic->stride_cb +
            ((ctx->round_origin_x >> 1) + (input_pic->org_x >> 1));
        loc.input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
            (ctx->blk_org_x + input_pic->org_x);
    }
    // 3rd Full-Loop
    ctx->md_stage        = MD_STAGE_3;
    ctx->tune_ssim_level = ((pcs->scs->static_config.tune == TUNE_SSIM || pcs->scs->static_config.tune == TUNE_IQ) &&
                            pcs->slice_type != I_SLICE && ctx->pd_pass == PD_PASS_1)
        ? SSIM_LVL_3
        : SSIM_LVL_0;
    md_stage_3(pcs, ctx, input_pic, &loc, ctx->md_stage_3_total_count);

    // Full Mode Decision (choose the best mode)
    uint32_t candidate_index = svt_aom_product_full_mode_decision(
        pcs, ctx, cand_bf_ptr_array, ctx->md_stage_3_total_count, ctx->best_candidate_index_array);
    ModeDecisionCandidateBuffer* cand_bf = cand_bf_ptr_array[candidate_index];
    //perform inverse transform recon when needed
    if (perform_md_recon) {
        av1_perform_inverse_transform_recon(pcs, ctx, cand_bf, ctx->blk_geom);
    }
    if (ctx->shape == PART_N &&
        ((!ctx->md_disallow_nsq_search && ctx->nsq_search_ctrls.max_part0_to_part1_dev &&
          ctx->blk_geom->bsize >= BLOCK_8X8 && ctx->blk_geom->sq_size > ctx->nsq_geom_ctrls.min_nsq_block_size) ||
         (ctx->skip_sub_depth_ctrls.enabled && ctx->blk_geom->sq_size <= ctx->skip_sub_depth_ctrls.max_size &&
          mds->split_flag &&
          (ctx->blk_geom->bsize >= BLOCK_16X16 || (!ctx->disallow_4x4 && ctx->blk_geom->bsize == BLOCK_8X8))))) {
        calc_scr_to_recon_dist_per_quadrant(
            ctx, input_pic, loc.input_origin_index, loc.input_cb_origin_in_index, cand_bf, 0, 0);
    }
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !org_hbd && ctx->pd_pass == PD_PASS_1 &&
        ctx->hbd_md) {
        if (!ctx->skip_intra || ctx->inter_intra_comp_ctrls.enabled) {
            convert_md_recon_16bit_to_8bit(pcs, ctx);
        }
        ctx->hbd_md             = 0;
        ctx->need_hbd_comp_mds3 = 0;
    }

    if (!ctx->skip_intra || ctx->inter_intra_comp_ctrls.enabled) {
        copy_recon_md(pcs, ctx, cand_bf);
    }
    if (!ctx->md_disallow_nsq_search && ctx->nsq_psq_txs_ctrls.enabled && ctx->shape == PART_N &&
        ctx->blk_geom->bsize >= BLOCK_8X8 && ctx->blk_geom->sq_size > ctx->nsq_geom_ctrls.min_nsq_block_size) {
        non_normative_txs(pcs, ctx, blk_ptr, cand_bf);
    }
}

static bool update_skip_nsq_based_on_split_rate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                const PC_TREE* const pc_tree, const MdScan* const mds) {
    bool       skip_nsq = false;
    const Part shape    = ctx->shape;
    const int  sq_size  = ctx->blk_geom->sq_size;

    // return immediately if SQ, or NSQ but Parent not available
    if (shape == PART_N || !pc_tree->tested_blk[PART_N][0]) {
        return skip_nsq;
    }

    const BlkStruct* sq_blk_ptr = pc_tree->block_data[PART_N][0];
    // if hbd_md is 0, we may still use 10bit lambda to generate final costs if we are bypassing encdec for 10bit content.
    const bool     used_10bit_at_mds3 = (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec &&
                                     ctx->pd_pass == PD_PASS_1 && svt_aom_do_md_recon(pcs->ppcs, ctx));
    const uint32_t full_lambda        = ctx->hbd_md || used_10bit_at_mds3 ? ctx->full_sb_lambda_md[EB_10_BIT_MD]
                                                                          : ctx->full_sb_lambda_md[EB_8_BIT_MD];
    uint32_t       nsq_split_cost_th  = ctx->nsq_search_ctrls.nsq_split_cost_th;
    // Get the rate cost of splitting into the current NSQ shape.
    // If the cost of the split rate is significant, then the shape is unlikely to be selected.
    if (nsq_split_cost_th) {
        if (sq_size <= 16) {
            nsq_split_cost_th = MAX(1, nsq_split_cost_th - ctx->nsq_search_ctrls.rate_th_offset_lte16);
        }
        const uint64_t split_rate = svt_aom_partition_rate_cost(pcs->ppcs,
                                                                pc_tree->bsize,
                                                                pc_tree->mi_row,
                                                                pc_tree->mi_col,
                                                                ctx->md_rate_est_ctx,
                                                                from_shape_to_part[shape],
                                                                pc_tree->left_part_ctx,
                                                                pc_tree->above_part_ctx);
        const uint64_t part_cost  = RDCOST(full_lambda, split_rate, 0);

        if (part_cost * 1000 > sq_blk_ptr->cost * nsq_split_cost_th) {
            return true;
        }
    }
    uint32_t H_vs_V_split_rate_th = ctx->nsq_search_ctrls.H_vs_V_split_rate_th;
    // Skip H/V if the rate cost of signaling H/V is significantly bigger than the rate cost of signaling V/H
    if (H_vs_V_split_rate_th && (shape == PART_H || shape == PART_V)) {
        if (sq_size <= 16) {
            H_vs_V_split_rate_th += ctx->nsq_search_ctrls.rate_th_offset_lte16;
        }
        const uint64_t H_rate      = svt_aom_partition_rate_cost(pcs->ppcs,
                                                            pc_tree->bsize,
                                                            pc_tree->mi_row,
                                                            pc_tree->mi_col,
                                                            ctx->md_rate_est_ctx,
                                                            PARTITION_HORZ,
                                                            pc_tree->left_part_ctx,
                                                            pc_tree->above_part_ctx);
        const uint64_t H_rate_cost = RDCOST(full_lambda, H_rate, 0);

        const uint64_t V_rate      = svt_aom_partition_rate_cost(pcs->ppcs,
                                                            pc_tree->bsize,
                                                            pc_tree->mi_row,
                                                            pc_tree->mi_col,
                                                            ctx->md_rate_est_ctx,
                                                            PARTITION_VERT,
                                                            pc_tree->left_part_ctx,
                                                            pc_tree->above_part_ctx);
        const uint64_t V_rate_cost = RDCOST(full_lambda, V_rate, 0);

        if (shape == PART_H && H_rate_cost * H_vs_V_split_rate_th > V_rate_cost * 100) {
            return true;
        }

        if (shape == PART_V && V_rate_cost * H_vs_V_split_rate_th > H_rate_cost * 100) {
            return true;
        }
    }
    uint32_t non_HV_split_rate_th = ctx->nsq_search_ctrls.non_HV_split_rate_th;
    // Skip non-H/V if the rate cost of signaling the shape is significantly bigger than the rate cost of signaling the current best shape
    if (non_HV_split_rate_th && !(shape == PART_H || shape == PART_V)) {
        if (sq_size <= 16) {
            non_HV_split_rate_th += ctx->nsq_search_ctrls.rate_th_offset_lte16;
        }
        const uint64_t part_rate = svt_aom_partition_rate_cost(pcs->ppcs,
                                                               pc_tree->bsize,
                                                               pc_tree->mi_row,
                                                               pc_tree->mi_col,
                                                               ctx->md_rate_est_ctx,
                                                               from_shape_to_part[shape],
                                                               pc_tree->left_part_ctx,
                                                               pc_tree->above_part_ctx);
        const uint64_t part_cost = RDCOST(full_lambda, part_rate, 0);

        const uint64_t best_part_rate = svt_aom_partition_rate_cost(pcs->ppcs,
                                                                    pc_tree->bsize,
                                                                    pc_tree->mi_row,
                                                                    pc_tree->mi_col,
                                                                    ctx->md_rate_est_ctx,
                                                                    pc_tree->partition,
                                                                    pc_tree->left_part_ctx,
                                                                    pc_tree->above_part_ctx);
        const uint64_t best_part_cost = RDCOST(full_lambda, best_part_rate, 0);

        if (part_cost * non_HV_split_rate_th > best_part_cost * 100) {
            return true;
        }
    }
    uint32_t lower_depth_split_cost_th = ctx->nsq_search_ctrls.lower_depth_split_cost_th;
    // Skip testing NSQ shapes at this depth if the rate cost of splitting is very low (assuming a lower depth is available for splitting)
    if (lower_depth_split_cost_th && mds->split_flag) {
        if (sq_size <= 16) {
            lower_depth_split_cost_th += ctx->nsq_search_ctrls.rate_th_offset_lte16;
        }
        const uint64_t split_rate = svt_aom_partition_rate_cost(pcs->ppcs,
                                                                pc_tree->bsize,
                                                                pc_tree->mi_row,
                                                                pc_tree->mi_col,
                                                                ctx->md_rate_est_ctx,
                                                                PARTITION_SPLIT,
                                                                pc_tree->left_part_ctx,
                                                                pc_tree->above_part_ctx);
        const uint64_t split_cost = RDCOST(full_lambda, split_rate, 0);
        if (split_cost * 10000 < sq_blk_ptr->cost * lower_depth_split_cost_th) {
            return true;
        }
    }
    const uint32_t component_multiple_th = ctx->nsq_search_ctrls.component_multiple_th;
    // Skip testing NSQ shapes at this depth if the rate cost of splitting is very low (assuming a lower depth is available for splitting)
    if (component_multiple_th) {
        const uint64_t parent_rate_cost = RDCOST(full_lambda, sq_blk_ptr->total_rate, 0);
        const uint64_t parent_dist_cost = RDCOST(full_lambda, 0, sq_blk_ptr->full_dist);

        const uint64_t max_comp = MAX(parent_rate_cost, parent_dist_cost);
        const uint64_t min_comp = MIN(parent_rate_cost, parent_dist_cost);

        if (max_comp > component_multiple_th * min_comp) {
            return true;
        }
    }
    return skip_nsq;
}

static bool update_skip_nsq_based_on_sq_recon_dist(ModeDecisionContext* ctx, const PC_TREE* const pc_tree) {
    uint32_t   max_part0_to_part1_dev = ctx->nsq_search_ctrls.max_part0_to_part1_dev;
    const Part shape                  = ctx->shape;

    // return immediately if SQ, or NSQ but Parent not available, or max_part0_to_part1_dev is off
    if (shape == PART_N || !pc_tree->tested_blk[PART_N][0] || max_part0_to_part1_dev == 0) {
        return false;
    }

    BlkStruct* sq_blk_ptr = pc_tree->block_data[PART_N][0];

    // Derive the distortion/cost ratio
    const uint32_t full_lambda     = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    const uint64_t dist            = RDCOST(full_lambda, 0, sq_blk_ptr->full_dist);
    const uint64_t dist_cost_ratio = (dist * 100) / sq_blk_ptr->cost;
    const uint64_t min_ratio       = 50;
    const uint64_t max_ratio       = 100;
    const uint64_t modulated_th    = (100 * (dist_cost_ratio - min_ratio)) / (max_ratio - min_ratio);

    // Modulate TH based on parent SQ pred_mode
    switch (sq_blk_ptr->block_mi.mode) {
    case NEWMV:
    case NEW_NEWMV:
        max_part0_to_part1_dev = ((max_part0_to_part1_dev * 75) / 100);
        break;
    case DC_PRED:
    case H_PRED:
    case V_PRED:
    case NEAREST_NEARESTMV:
    case NEAR_NEARMV:
        max_part0_to_part1_dev *= 2;
        break;
    case D45_PRED:
    case D135_PRED:
    case D113_PRED:
    case D157_PRED:
    case D203_PRED:
    case D67_PRED:
    case SMOOTH_PRED:
    case SMOOTH_H_PRED:
    case SMOOTH_V_PRED:
    case PAETH_PRED:
    case GLOBALMV:
    case GLOBAL_GLOBALMV:
        max_part0_to_part1_dev <<= 2;
        break;
    default:
        break;
    }

    if (shape == PART_H || shape == PART_HA || shape == PART_HB || shape == PART_H4) {
        // multiply the TH by 4 when Parent is D45 or D135 (diagonal) or when Parent is D67 / V / D113 (H_path)
        if (sq_blk_ptr->block_mi.mode == V_PRED || sq_blk_ptr->block_mi.mode == D67_PRED ||
            sq_blk_ptr->block_mi.mode == D113_PRED || sq_blk_ptr->block_mi.mode == D45_PRED ||
            sq_blk_ptr->block_mi.mode == D135_PRED) {
            max_part0_to_part1_dev <<= 2;
        } else if (sq_blk_ptr->block_mi.mode == H_PRED) {
            max_part0_to_part1_dev = 0;
        }

        const uint64_t dist_q0 = MAX(1, ctx->rec_dist_per_quadrant[0]);
        const uint64_t dist_q1 = MAX(1, ctx->rec_dist_per_quadrant[1]);
        const uint64_t dist_q2 = MAX(1, ctx->rec_dist_per_quadrant[2]);
        const uint64_t dist_q3 = MAX(1, ctx->rec_dist_per_quadrant[3]);

        const uint64_t dist_h0 = dist_q0 + dist_q1;
        const uint64_t dist_h1 = dist_q2 + dist_q3;

        const uint32_t dev = (uint32_t)((ABS((int64_t)dist_h0 - (int64_t)dist_h1) * 100) / MIN(dist_h0, dist_h1));
        // TH = TH + TH * Min(dev_0,dev_1); dev_0 is q0 - to - q1 deviation, and dev_1 is q2 - to - q3 deviation
        const uint32_t quad_dev_t = (uint32_t)((ABS((int64_t)dist_q0 - (int64_t)dist_q1) * 100) /
                                               MIN(dist_q0, dist_q1));
        const uint32_t quad_dev_b = (uint32_t)((ABS((int64_t)dist_q2 - (int64_t)dist_q3) * 100) /
                                               MIN(dist_q2, dist_q3));
        max_part0_to_part1_dev    = max_part0_to_part1_dev +
            (((uint64_t)max_part0_to_part1_dev * MIN(quad_dev_t, quad_dev_b)) / 100);

        max_part0_to_part1_dev = (uint32_t)((dist_cost_ratio <= min_ratio) ? 0
                                                : (dist_cost_ratio <= max_ratio)
                                                ? (max_part0_to_part1_dev * modulated_th) / 100
                                                : dist_cost_ratio);
        if (dev < max_part0_to_part1_dev) {
            return true;
        }
    }

    if (shape == PART_V || shape == PART_VA || shape == PART_VB || shape == PART_V4) {
        // multiply the TH by 4 when Parent is D45 or D135 (diagonal) or when Parent is D157 / H / D203 (V_path)
        if (sq_blk_ptr->block_mi.mode == H_PRED || sq_blk_ptr->block_mi.mode == D157_PRED ||
            sq_blk_ptr->block_mi.mode == D203_PRED || sq_blk_ptr->block_mi.mode == D45_PRED ||
            sq_blk_ptr->block_mi.mode == D135_PRED) {
            max_part0_to_part1_dev <<= 2;
        } else if (sq_blk_ptr->block_mi.mode == V_PRED) {
            max_part0_to_part1_dev = 0;
        }

        const uint64_t dist_q0 = MAX(1, ctx->rec_dist_per_quadrant[0]);
        const uint64_t dist_q1 = MAX(1, ctx->rec_dist_per_quadrant[1]);
        const uint64_t dist_q2 = MAX(1, ctx->rec_dist_per_quadrant[2]);
        const uint64_t dist_q3 = MAX(1, ctx->rec_dist_per_quadrant[3]);

        const uint64_t dist_v0 = dist_q0 + dist_q2;
        const uint64_t dist_v1 = dist_q1 + dist_q3;

        const uint32_t dev = (uint32_t)((ABS((int64_t)dist_v0 - (int64_t)dist_v1) * 100) / MIN(dist_v0, dist_v1));

        // TH = TH + TH * Min(dev_0,dev_1); dev_0 is q0-to-q2 deviation, and dev_1 is q1-to-q3 deviation
        const uint32_t quad_dev_l = (uint32_t)((ABS((int64_t)dist_q0 - (int64_t)dist_q2) * 100) /
                                               MIN(dist_q0, dist_q2));
        const uint32_t quad_dev_r = (uint32_t)((ABS((int64_t)dist_q1 - (int64_t)dist_q3) * 100) /
                                               MIN(dist_q1, dist_q3));
        max_part0_to_part1_dev    = max_part0_to_part1_dev +
            (((uint64_t)max_part0_to_part1_dev * MIN(quad_dev_l, quad_dev_r)) / 100);

        max_part0_to_part1_dev = (uint32_t)((dist_cost_ratio <= min_ratio) ? 0
                                                : (dist_cost_ratio <= max_ratio)
                                                ? (max_part0_to_part1_dev * modulated_th) / 100
                                                : dist_cost_ratio);
        if (dev < max_part0_to_part1_dev) {
            return true;
        }
    }
    return false;
}

/*
 * Determine if the evaluation of nsq blocks (HA, HB, VA, VB, H4, V4) can be skipped
 * based on the relative cost of the SQ, H, and V blocks.  The scaling factor sq_weight
 * determines how likely it is to skip blocks, and is a function of the qp, block shape,
 * prediction mode, block coeffs, and encode mode.  If skip_hv4_on_best_part is enabled
 * H4/V4 blocks will be skipped if the best partition so far is not H/V.
 *
 * skip HA, HB and H4 if (valid SQ and H) and (H_COST > (SQ_WEIGHT * SQ_COST) / 100)
 * skip VA, VB and V4 if (valid SQ and V) and (V_COST > (SQ_WEIGHT * SQ_COST) / 100)
 *
 * Returns true if the blocks should be skipped; false otherwise.
 */
static uint8_t update_skip_nsq_shapes(ModeDecisionContext* ctx, const PC_TREE* const pc_tree) {
    const Part shape     = ctx->shape;
    uint8_t    skip_nsq  = 0;
    uint32_t   sq_weight = ctx->nsq_search_ctrls.sq_weight;
    // return immediately if the skip nsq threshold is infinite
    if (sq_weight == (uint32_t)~0) {
        return skip_nsq;
    }

    // use a conservative threshold for H4, V4 blocks
    if (shape == PART_H4 || shape == PART_V4) {
        sq_weight += CONSERVATIVE_OFFSET_0;
    }

    if (shape == PART_HA || shape == PART_HB || shape == PART_H4) {
        if (pc_tree->tested_blk[PART_N][0] && pc_tree->tested_blk[PART_H][0] && pc_tree->tested_blk[PART_H][1]) {
            // Use aggressive thresholds for blocks without coeffs
            if (shape == PART_HA) {
                if (!pc_tree->block_data[PART_H][0]->block_has_coeff) {
                    sq_weight = (int32_t)sq_weight + AGGRESSIVE_OFFSET_1;
                }
            }
            if (shape == PART_HB) {
                if (!pc_tree->block_data[PART_H][1]->block_has_coeff) {
                    sq_weight = (int32_t)sq_weight + AGGRESSIVE_OFFSET_1;
                }
            }

            // compute the cost of the SQ block and H block
            const uint64_t sq_cost = pc_tree->block_data[PART_N][0]->cost;
            const uint64_t h_cost  = pc_tree->block_data[PART_H][0]->cost + pc_tree->block_data[PART_H][1]->cost;

            // Determine if nsq shapes can be skipped based on the relative cost of SQ and H blocks
            skip_nsq = (h_cost > ((sq_cost * sq_weight) / 100));
            // If not skipping, perform a check on the relative H/V costs
            if (!skip_nsq && pc_tree->tested_blk[PART_V][0] && pc_tree->tested_blk[PART_V][1]) {
                //compute the cost of V partition
                const uint64_t v_cost   = pc_tree->block_data[PART_V][0]->cost + pc_tree->block_data[PART_V][1]->cost;
                const uint32_t v_weight = ctx->nsq_search_ctrls.hv_weight;
                //if the cost of H partition is bigger than the V partition by a certain percentage
                skip_nsq = (h_cost > ((v_cost * v_weight) / 100));
            }
        }
    }

    if (shape == PART_VA || shape == PART_VB || shape == PART_V4) {
        if (pc_tree->tested_blk[PART_N][0] && pc_tree->tested_blk[PART_V][0] && pc_tree->tested_blk[PART_V][1]) {
            // Use aggressive thresholds for blocks without coeffs
            if (shape == PART_VA) {
                if (!pc_tree->block_data[PART_V][0]->block_has_coeff) {
                    sq_weight = (int32_t)sq_weight + AGGRESSIVE_OFFSET_1;
                }
            }
            if (shape == PART_VB) {
                if (!pc_tree->block_data[PART_V][1]->block_has_coeff) {
                    sq_weight = (int32_t)sq_weight + AGGRESSIVE_OFFSET_1;
                }
            }

            // compute the cost of the SQ block and V block
            const uint64_t sq_cost = pc_tree->block_data[PART_N][0]->cost;
            const uint64_t v_cost  = pc_tree->block_data[PART_V][0]->cost + pc_tree->block_data[PART_V][1]->cost;

            // Determine if nsq shapes can be skipped based on the relative cost of SQ and V blocks
            skip_nsq = (v_cost > ((sq_cost * sq_weight) / 100));

            // If not skipping, perform a check on the relative H/V costs
            if (!skip_nsq && pc_tree->tested_blk[PART_H][0] && pc_tree->tested_blk[PART_H][1]) {
                const uint64_t h_cost   = pc_tree->block_data[PART_H][0]->cost + pc_tree->block_data[PART_H][1]->cost;
                const uint32_t h_weight = ctx->nsq_search_ctrls.hv_weight;
                //if the cost of V partition is bigger than the H partition by a certain percentage
                skip_nsq = (v_cost > ((h_cost * h_weight) / 100));
            }
        }
    }

    return skip_nsq;
}

static bool update_skip_nsq_based_on_sq_txs(ModeDecisionContext* ctx, const PC_TREE* const pc_tree) {
    const Part shape = ctx->shape;

    // return immediately if SQ, or NSQ but Parent not available, or sq_txs is off
    if (shape == PART_N || !pc_tree->tested_blk[PART_N][0] || !ctx->nsq_psq_txs_ctrls.enabled) {
        return false;
    }

    BlkStruct* sq_md_blk_arr_nsq = pc_tree->block_data[PART_N][0];
    if ((ctx->min_nz_h != (uint16_t)~0) && (ctx->min_nz_v != (uint16_t)~0)) {
        uint32_t hv_to_sq_th = ctx->nsq_psq_txs_ctrls.hv_to_sq_th;
        uint32_t h_to_v_th   = ctx->nsq_psq_txs_ctrls.h_to_v_th;
        uint16_t cnt_h_min   = ctx->min_nz_h;
        uint16_t cnt_v_min   = ctx->min_nz_v;
        uint16_t cnt_h_best  = cnt_h_min << 1;
        uint16_t cnt_v_best  = cnt_v_min << 1;

        if ((cnt_h_best >= ((sq_md_blk_arr_nsq->cnt_nz_coeff * hv_to_sq_th) / 100)) &&
            (cnt_v_best >= ((sq_md_blk_arr_nsq->cnt_nz_coeff * hv_to_sq_th) / 100))) {
            return true;
        }

        if (shape == PART_H || shape == PART_HA || shape == PART_HB || shape == PART_H4) {
            if ((cnt_v_best <= cnt_h_best) && (cnt_h_best >= ((sq_md_blk_arr_nsq->cnt_nz_coeff * h_to_v_th) / 100))) {
                return true;
            }
        }

        if (shape == PART_V || shape == PART_VA || shape == PART_VB || shape == PART_V4) {
            if ((cnt_h_best <= cnt_v_best) && (cnt_v_best >= ((sq_md_blk_arr_nsq->cnt_nz_coeff * h_to_v_th) / 100))) {
                return true;
            }
        }
    }

    return false;
}

/*
 * Pad high bit depth pictures.
 *
 * Returns pointer to padded data.
 */
static EbPictureBufferDesc* pad_hbd_pictures(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                             EbPictureBufferDesc* in_pic) {
    uint32_t sb_org_x = ctx->sb_origin_x;
    uint32_t sb_org_y = ctx->sb_origin_y;
    //perform the packing of 10bit if not done in previous PD passes
    if (!ctx->hbd_pack_done) {
        const uint32_t input_luma_offset = ((sb_org_y + in_pic->org_y) * in_pic->stride_y) + (sb_org_x + in_pic->org_x);
        const uint32_t input_cb_offset   = (((sb_org_y + in_pic->org_y) >> 1) * in_pic->stride_cb) +
            ((sb_org_x + in_pic->org_x) >> 1);
        const uint32_t input_cr_offset = (((sb_org_y + in_pic->org_y) >> 1) * in_pic->stride_cr) +
            ((sb_org_x + in_pic->org_x) >> 1);

        uint32_t sb_width  = MIN(scs->sb_size, pcs->ppcs->aligned_width - sb_org_x);
        uint32_t sb_height = MIN(scs->sb_size, pcs->ppcs->aligned_height - sb_org_y);

        //sb_width is n*8 so the 2bit-decompression kernel works properly
        uint32_t comp_stride_y           = in_pic->stride_y / 4;
        uint32_t comp_luma_buffer_offset = comp_stride_y * in_pic->org_y + in_pic->org_x / 4;
        comp_luma_buffer_offset += sb_org_x / 4 + sb_org_y * comp_stride_y;

        svt_aom_compressed_pack_sb(in_pic->buffer_y + input_luma_offset,
                                   in_pic->stride_y,
                                   in_pic->buffer_bit_inc_y + comp_luma_buffer_offset,
                                   comp_stride_y,
                                   (uint16_t*)ctx->input_sample16bit_buffer->buffer_y,
                                   ctx->input_sample16bit_buffer->stride_y,
                                   sb_width,
                                   sb_height);

        uint32_t comp_stride_uv            = in_pic->stride_cb / 4;
        uint32_t comp_chroma_buffer_offset = comp_stride_uv * (in_pic->org_y / 2) + in_pic->org_x / 2 / 4;
        comp_chroma_buffer_offset += sb_org_x / 4 / 2 + sb_org_y / 2 * comp_stride_uv;

        svt_aom_compressed_pack_sb(in_pic->buffer_cb + input_cb_offset,
                                   in_pic->stride_cb,
                                   in_pic->buffer_bit_inc_cb + comp_chroma_buffer_offset,
                                   comp_stride_uv,
                                   (uint16_t*)ctx->input_sample16bit_buffer->buffer_cb,
                                   ctx->input_sample16bit_buffer->stride_cb,
                                   sb_width / 2,
                                   sb_height / 2);

        svt_aom_compressed_pack_sb(in_pic->buffer_cr + input_cr_offset,
                                   in_pic->stride_cr,
                                   in_pic->buffer_bit_inc_cr + comp_chroma_buffer_offset,
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

        uint32_t chroma_pad_width  = (scs->sb_size - sb_width) >> 1;
        uint32_t chroma_pad_height = (scs->sb_size - sb_height) >> 1;

        svt_aom_pad_input_picture_16bit((uint16_t*)ctx->input_sample16bit_buffer->buffer_cb,
                                        ctx->input_sample16bit_buffer->stride_cb,
                                        sb_width >> 1,
                                        sb_height >> 1,
                                        chroma_pad_width,
                                        chroma_pad_height);

        svt_aom_pad_input_picture_16bit((uint16_t*)ctx->input_sample16bit_buffer->buffer_cr,
                                        ctx->input_sample16bit_buffer->stride_cr,
                                        sb_width >> 1,
                                        sb_height >> 1,
                                        chroma_pad_width,
                                        chroma_pad_height);
        svt_aom_store16bit_input_src(
            ctx->input_sample16bit_buffer, pcs, sb_org_x, sb_org_y, scs->sb_size, scs->sb_size);

        ctx->hbd_pack_done = 1;
    }
    return pcs->input_frame16bit;
}

/*
 * Update the neighbour arrays before starting block processing.
 */
static INLINE void update_neighbour_arrays_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    const uint16_t tile_idx = ctx->tile_index;
    ctx->recon_neigh_y      = pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
}

/*
 * Update the neighbour arrays before starting block processing.
 */
static void update_neighbour_arrays(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    const uint16_t tile_idx = ctx->tile_index;
    ctx->leaf_partition_na  = pcs->mdleaf_partition_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && !ctx->hbd_md && ctx->pd_pass == PD_PASS_1) {
        ctx->recon_neigh_y  = pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->recon_neigh_cb = pcs->md_cb_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->recon_neigh_cr = pcs->md_cr_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];

        ctx->luma_recon_na_16bit = pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->cb_recon_na_16bit   = pcs->md_cb_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->cr_recon_na_16bit   = pcs->md_cr_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    } else if (!ctx->hbd_md) {
        ctx->recon_neigh_y  = pcs->md_luma_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->recon_neigh_cb = pcs->md_cb_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->recon_neigh_cr = pcs->md_cr_recon_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    } else {
        ctx->luma_recon_na_16bit = pcs->md_luma_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->cb_recon_na_16bit   = pcs->md_cb_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
        ctx->cr_recon_na_16bit   = pcs->md_cr_recon_na_16bit[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    }
    ctx->luma_dc_sign_level_coeff_na = pcs->md_y_dcs_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    ctx->cb_dc_sign_level_coeff_na   = pcs->md_cb_dc_sign_level_coeff_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    ctx->cr_dc_sign_level_coeff_na   = pcs->md_cr_dc_sign_level_coeff_na[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
    ctx->txfm_context_array          = pcs->md_txfm_context_array[MD_NEIGHBOR_ARRAY_INDEX][tile_idx];
}

static EbErrorType md_rtime_alloc_palette_info(BlkStruct* md_blk_arr_nsq) {
    EB_MALLOC_ARRAY(md_blk_arr_nsq->palette_info, 1);
    EB_MALLOC_ARRAY(md_blk_arr_nsq->palette_info->color_idx_map, MAX_PALETTE_SQUARE);

    return EB_ErrorNone;
}

/*
 * Initialize data needed for processing each block.  Update neighbour array if the block
 * is the first d1 block.  Called before process each block.
 */
static void init_block_data(PictureControlSet* pcs, ModeDecisionContext* ctx, const int mi_row, const int mi_col,
                            const Part shape, const uint32_t blk_idx_mds) {
    const BlockGeom* blk_geom = ctx->blk_geom;
    BlkStruct*       blk_ptr  = ctx->blk_ptr;
    ctx->scale_palette        = 0;
    ctx->blk_org_x            = mi_col << MI_SIZE_LOG2;
    ctx->blk_org_y            = mi_row << MI_SIZE_LOG2;
    ctx->round_origin_x       = ROUND_UV(ctx->blk_org_x);
    ctx->round_origin_y       = ROUND_UV(ctx->blk_org_y);
    ctx->has_uv               = is_chroma_reference(mi_row, mi_col, blk_geom->bsize, 1, 1);
    ctx->shape                = shape;

    blk_ptr->mds_idx = blk_idx_mds;
    blk_ptr->qindex  = ctx->qp_index;
    //  MD palette info buffer
    if (svt_av1_allow_palette(pcs->ppcs->palette_level, blk_geom->bsize)) {
        if (blk_ptr->palette_mem == 0) {
            md_rtime_alloc_palette_info(blk_ptr);
            blk_ptr->palette_mem = 1;
        }
    }

    blk_ptr->palette_size[0] = 0;
    blk_ptr->palette_size[1] = 0;
    ctx->sb64_sq_no4xn_geom  = 0;
    if (pcs->ppcs->scs->super_block_size == 64 && blk_geom->bwidth == blk_geom->bheight &&
        blk_geom->bsize > BLOCK_8X4) {
        ctx->sb64_sq_no4xn_geom = 1;
    }
}

#if !FTR_VLPD0
static const uint8_t var_log2_lut[4] = {6, 5, 4, 3};
static const uint8_t var_grid_lut[4] = {1, 2, 4, 8};
static const uint8_t var_base_lut[4] = {0, 1, 5, 21};

static void get_blk_var_map(int block_size, int org_x, int org_y, int* blk_idx, int sub_idx[4]) {
    // Map block size to level: 64->0, 32->1, 16->2, 8->3
    const int lvl = 6 - svt_log2f(block_size);

    // Parent block
    const int shift = var_log2_lut[lvl];
    const int grid  = var_grid_lut[lvl];
    const int base  = var_base_lut[lvl];

    const int gx = org_x >> shift;
    const int gy = org_y >> shift;

    *blk_idx = base + gy * grid + gx;

    // Sub-blocks
    const int sub_lvl   = lvl + 1;
    const int sub_shift = var_log2_lut[sub_lvl];
    const int sub_base  = var_base_lut[sub_lvl];
    const int sub_grid  = var_grid_lut[sub_lvl];

    const int sx = org_x >> sub_shift;
    const int sy = org_y >> sub_shift;

    sub_idx[0] = sub_base + (sy + 0) * sub_grid + (sx + 0);
    sub_idx[1] = sub_base + (sy + 0) * sub_grid + (sx + 1);
    sub_idx[2] = sub_base + (sy + 1) * sub_grid + (sx + 0);
    sub_idx[3] = sub_base + (sy + 1) * sub_grid + (sx + 1);
}
#endif
/*
 * Check if a block is redundant, and if so, copy the data from the original block
 * return 1 if block is redundant and updated, 0 otherwise
 */
static bool update_redundant(PictureControlSet* pcs, ModeDecisionContext* ctx, PC_TREE* pc_tree, const int nsi) {
    const BlockGeom* blk_geom = ctx->blk_geom;
    BlkStruct*       blk_ptr  = ctx->blk_ptr;

    // For SQ blocks, certain calculations are performed on the predicted/recon samples that
    // cannot be copied from the higher depth's redundant blocks (e.g. rec_dist_per_quadrant).
    // To avoid basing decisions on this uncomputed data (or copying erroneous data) do not
    // skip processing SQ blocks.
    if (ctx->shape == PART_N || !ctx->redundant_blk) {
        return 0;
    }

    // if PART_HB and nsi==0 --> if tested[PART_H][0] -> match
    // if PART_VB and nsi==0 --> if tested[PART_V][0] -> match
    // if PART_VA and nsi==0 --> if tested[PART_HA][0] -> match
    BlkStruct* redund_blk_ptr = NULL;
    if (ctx->shape == PART_HB && nsi == 0 && pc_tree->tested_blk[PART_H][0]) {
        redund_blk_ptr = pc_tree->block_data[PART_H][0];
    } else if (ctx->shape == PART_VB && nsi == 0 && pc_tree->tested_blk[PART_V][0]) {
        redund_blk_ptr = pc_tree->block_data[PART_V][0];
    } else if (ctx->shape == PART_VA && nsi == 0 && pc_tree->tested_blk[PART_HA][0]) {
        redund_blk_ptr = pc_tree->block_data[PART_HA][0];
    }

    // if no redundant block identified, exit
    if (!redund_blk_ptr) {
        return 0;
    }

    // Copy results
    move_blk_data_redund(pcs, ctx, redund_blk_ptr, blk_ptr);

    if (ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1) {
        // If a redundant block is being tested, there must be a search over NSQ shapes and/or depth.
        // Therefore, we save the recon and coeffs under the blk_ptr, instead of writing directly to
        // a final buffer.  Once the final partition is selected, the recon/coeff will be copied to
        // the final buffer in svt_aom_encdec_update.

        // Copy recon
        const uint8_t bwidth     = blk_geom->bwidth;
        const uint8_t bheight    = blk_geom->bheight;
        const uint8_t bwidth_uv  = blk_geom->bwidth_uv;
        const uint8_t bheight_uv = blk_geom->bheight_uv;
        uint16_t      sz         = ctx->encoder_bit_depth > EB_EIGHT_BIT ? sizeof(uint16_t) : sizeof(uint8_t);

        uint32_t dst_stride = blk_ptr->recon_tmp->stride_y;
        uint32_t src_stride = redund_blk_ptr->recon_tmp->stride_y;
        for (uint32_t i = 0; i < bheight; i++) {
            svt_memcpy(blk_ptr->recon_tmp->buffer_y + (i * dst_stride) * sz,
                       redund_blk_ptr->recon_tmp->buffer_y + (i * src_stride) * sz,
                       bwidth * sz);
        }

        dst_stride = blk_ptr->recon_tmp->stride_cb;
        src_stride = redund_blk_ptr->recon_tmp->stride_cb;
        for (uint32_t i = 0; i < bheight_uv; i++) {
            svt_memcpy(blk_ptr->recon_tmp->buffer_cb + (i * dst_stride) * sz,
                       redund_blk_ptr->recon_tmp->buffer_cb + (i * src_stride) * sz,
                       bwidth_uv * sz);
        }

        dst_stride = blk_ptr->recon_tmp->stride_cr;
        src_stride = redund_blk_ptr->recon_tmp->stride_cr;
        for (uint32_t i = 0; i < bheight_uv; i++) {
            svt_memcpy(blk_ptr->recon_tmp->buffer_cr + (i * dst_stride) * sz,
                       redund_blk_ptr->recon_tmp->buffer_cr + (i * src_stride) * sz,
                       bwidth_uv * sz);
        }

        // Copy coeffs
        int32_t* dst_ptr = &(((int32_t*)blk_ptr->coeff_tmp->buffer_y)[0]);
        int32_t* src_ptr = &(((int32_t*)redund_blk_ptr->coeff_tmp->buffer_y)[0]);
        svt_memcpy(dst_ptr, src_ptr, bheight * bwidth * sizeof(int32_t));

        dst_ptr = &(((int32_t*)blk_ptr->coeff_tmp->buffer_cb)[0]);
        src_ptr = &(((int32_t*)redund_blk_ptr->coeff_tmp->buffer_cb)[0]);
        svt_memcpy(dst_ptr, src_ptr, bheight_uv * bwidth_uv * sizeof(int32_t));

        dst_ptr = &(((int32_t*)blk_ptr->coeff_tmp->buffer_cr)[0]);
        src_ptr = &(((int32_t*)redund_blk_ptr->coeff_tmp->buffer_cr)[0]);
        svt_memcpy(dst_ptr, src_ptr, bheight_uv * bwidth_uv * sizeof(int32_t));
    }
    return 1;
}

static bool get_skip_processing_nsq_block(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                          const PC_TREE* const pc_tree, const MdScan* const mds) {
    int skip_processing_block = false;
    if (update_skip_nsq_based_on_split_rate(pcs, ctx, pc_tree, mds)) {
        return true;
    }
    if (update_skip_nsq_based_on_sq_txs(ctx, pc_tree)) {
        return true;
    }
    if (update_skip_nsq_based_on_sq_recon_dist(ctx, pc_tree)) {
        return true;
    }
    if (update_skip_nsq_shapes(ctx, pc_tree)) {
        return true;
    }
    return skip_processing_block;
}

static bool eval_sub_depth_skip_cond1(ModeDecisionContext* ctx, const PC_TREE* const pc_tree) {
    uint8_t n = 4;
    float   average, variance, std_deviation, sum = 0, sum1 = 0;

    // Compute the sum of all dist
    for (uint8_t q_idx = 0; q_idx < n; q_idx++) {
        sum = sum + ctx->rec_dist_per_quadrant[q_idx];
    }
    average = sum / (float)n;

    // Compute variance and standard deviation
    for (uint8_t q_idx = 0; q_idx < n; q_idx++) {
        sum1 = sum1 + (float)pow((ctx->rec_dist_per_quadrant[q_idx] - average), 2);
    }
    variance      = sum1 / n;
    std_deviation = sqrtf(variance);

    uint32_t count_non_zero_coeffs = pc_tree->block_data[PART_N][0]->cnt_nz_coeff;

    uint32_t total_samples = (ctx->blk_geom->sq_size * ctx->blk_geom->sq_size);
    uint32_t coeff_perc    = (count_non_zero_coeffs * 100) / total_samples;

    if (std_deviation < ctx->skip_sub_depth_ctrls.quad_deviation_th &&
        coeff_perc < ctx->skip_sub_depth_ctrls.coeff_perc) {
        return true;
    }

    return false;
}

// For certain configurations, update the feature settings for NSQ blocks in non-ISLICE frames
static void faster_md_settings_nsq(PictureControlSet* pcs, ModeDecisionContext* ctx, const PC_TREE* const pc_tree,
                                   const bool is_child) {
    // Update GM settings based on SQ block mode
    if (pcs->ppcs->gm_ctrls.enabled && pcs->ppcs->gm_ctrls.inj_psq_glb) {
        if (pc_tree->tested_blk[PART_N][0]) {
            PredictionMode sq_mode = pc_tree->block_data[PART_N][0]->block_mi.mode;
            if (sq_mode != GLOBAL_GLOBALMV && sq_mode != GLOBALMV) {
                ctx->params_status       = 1;
                ctx->global_mv_injection = 0;
            }
        }
    }

    // Update settings for sub_depth_block_lvl feature
    if (ctx->nsq_search_ctrls.sub_depth_block_lvl && ctx->pd_pass == PD_PASS_1 && is_child) {
        ctx->nsq_search_ctrls.sq_weight            = MIN(85, ctx->nsq_search_ctrls.sq_weight);
        ctx->nsq_search_ctrls.nsq_split_cost_th    = MIN(60, ctx->nsq_search_ctrls.nsq_split_cost_th);
        ctx->nsq_search_ctrls.H_vs_V_split_rate_th = MAX(60, ctx->nsq_search_ctrls.H_vs_V_split_rate_th);
        ctx->nsq_search_ctrls.non_HV_split_rate_th = MAX(60, ctx->nsq_search_ctrls.non_HV_split_rate_th);
        ctx->params_status                         = 1;
    }
}
#if !CLN_REMOVE_VAR_SUB_DEPTH
// Use variance to determine if sub depths should be skipped. Returns true when sub depths should be skipped,
// false when the sub depths should be tested.
static bool var_skip_sub_depth(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    int blk_idx;
    int sub_idx[4];
#if FTR_VLPD0
    svt_aom_get_blk_var_map(ctx->blk_geom->sq_size, ctx->blk_geom->org_x, ctx->blk_geom->org_y, &blk_idx, sub_idx);
#else
    get_blk_var_map(ctx->blk_geom->sq_size, ctx->blk_geom->org_x, ctx->blk_geom->org_y, &blk_idx, sub_idx);
#endif
    uint16_t* sb_var = pcs->ppcs->variance[ctx->sb_index];

    uint32_t sub_var[4] = {sb_var[sub_idx[0]], sb_var[sub_idx[1]], sb_var[sub_idx[2]], sb_var[sub_idx[3]]};

    uint32_t min_var = UINT32_MAX;
    uint32_t max_var = 0;
    uint32_t sum_var = 0;

    for (int i = 0; i < 4; i++) {
        uint32_t v = sub_var[i];
        sum_var += v;
        if (v < min_var) {
            min_var = v;
        }
        if (v > max_var) {
            max_var = v;
        }
    }

    uint32_t spread_var = max_var - min_var;

    uint32_t count_non_zero_coeffs = ctx->md_blk_arr_nsq[ctx->blk_geom->sqi_mds].cnt_nz_coeff;
    uint32_t total_samples         = (ctx->blk_geom->sq_size * ctx->blk_geom->sq_size);
    uint32_t coeff_perc            = (count_non_zero_coeffs * 100) / total_samples;

    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.var_skip_sub_depth_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->scs->static_config.qp);

    uint8_t  th_idx  = 6 - svt_log2f(ctx->blk_geom->sq_size);
    uint32_t abs_th  = ctx->var_skip_sub_depth_ctrls.edge_th[th_idx][0];
    abs_th           = DIVIDE_AND_ROUND(abs_th * q_weight, q_weight_denom);
    uint32_t rel_th  = ctx->var_skip_sub_depth_ctrls.edge_th[th_idx][1];
    rel_th           = DIVIDE_AND_ROUND(rel_th * q_weight, q_weight_denom);
    uint32_t peak_th = ctx->var_skip_sub_depth_ctrls.edge_th[th_idx][2];
    peak_th          = DIVIDE_AND_ROUND(peak_th * q_weight, q_weight_denom);
#if FTR_VLPD0
    const bool is_edge = (ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0 ||
                          coeff_perc > ctx->var_skip_sub_depth_ctrls.coeff_th) &&
#else
    const bool is_edge = (coeff_perc > ctx->var_skip_sub_depth_ctrls.coeff_th) &&
#endif
        (100 * spread_var > abs_th * sum_var) && (100 * max_var > rel_th * sum_var) && (spread_var > peak_th);

    return !is_edge;
}
#endif

static bool test_split_partition_lpd0(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                      MdScan* mds, PC_TREE* pc_tree, int mi_row, int mi_col) {
    const int mi_rows = pcs->ppcs->av1_cm->mi_rows;
    const int mi_cols = pcs->ppcs->av1_cm->mi_cols;
    // For properly set MdScan, this should never be true. This check is added as a safeguard, in case
    // we try testing a block that is completely outside the picture boundaries.
    if (mi_row >= mi_rows || mi_col >= mi_cols) {
        return false;
    }

    const uint32_t full_lambda = ctx->full_sb_lambda_md[EB_8_BIT_MD];
#if FTR_VLPD0 //
    int64_t above_split_rate = (scs->allintra && ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0)
        ? 0
        : svt_aom_partition_rate_cost(pcs->ppcs,
                                      pc_tree->bsize,
                                      mi_row,
                                      mi_col,
                                      ctx->md_rate_est_ctx,
                                      PARTITION_SPLIT,
                                      0, //left_neighbor_partition,
                                      0); // above_neighbor_partition);
#else
    int64_t above_split_rate = svt_aom_partition_rate_cost(pcs->ppcs,
                                                           pc_tree->bsize,
                                                           mi_row,
                                                           mi_col,
                                                           ctx->md_rate_est_ctx,
                                                           PARTITION_SPLIT,
                                                           0, //left_neighbor_partition,
                                                           0); // above_neighbor_partition);
#endif
    // If not using accurate partition rate, bias against splitting by increasing the rate of SPLIT partition
    if (!pcs->ppcs->use_accurate_part_ctx) {
        above_split_rate *= 2;
    }
    int64_t split_cost = RDCOST(full_lambda, above_split_rate, 0);

    bool      last_quad_valid = true;
    const int mi_step         = mi_size_wide[pc_tree->bsize] / 2;
    for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
        const int x_idx = (i & 1) * mi_step;
        const int y_idx = (i >> 1) * mi_step;
        // if block fully outside pic, don't process
        if (mi_row + y_idx >= mi_rows || mi_col + x_idx >= mi_cols) {
            // If any quadrant is out of bounds, the last quadrant will be
            last_quad_valid = false;
            continue;
        }

        // Check current depth cost; if larger than parent, exit early
#if FTR_VLPD0
        if (!pcs->scs->allintra || ctx->lpd0_ctrls.pd0_level != VERY_LIGHT_PD0) {
#endif
            if (pc_tree->rdc.valid) {
                const uint32_t th = (i == 0)
                    ? (ctx->depth_early_exit_ctrls.split_cost_th == 0 ? 1000
                                                                      : ctx->depth_early_exit_ctrls.split_cost_th)
                    : (ctx->depth_early_exit_ctrls.early_exit_th == 0 ? 1000
                                                                      : ctx->depth_early_exit_ctrls.early_exit_th);
                if ((pc_tree->rdc.rd_cost * th * ctx->parent_cost_bias) <= (split_cost * 1000 * 1000)) {
                    return false;
                }
            }
#if FTR_VLPD0
        }
#endif
        const bool valid_split_partition = svt_aom_pick_partition_lpd0(
            scs, pcs, ctx, mds->split[i], pc_tree->split[i], mi_row + y_idx, mi_col + x_idx);

        if (!valid_split_partition) {
            // If split is invalid, then exit (all
            // split quadrants must be valid for split to be selected).
            return false;
        }

        split_cost += pc_tree->split[i]->rdc.rd_cost;
    }

    // Only get here if all partitions are valid (and/or out of bounds).
    PC_TREE* array_update_part = pc_tree;
    if (pc_tree->rdc.valid && (ctx->parent_cost_bias * pc_tree->rdc.rd_cost <= split_cost * 1000)) {
        pc_tree->rdc.valid = 1;
    } else {
        pc_tree->rdc.rd_cost = split_cost;
        pc_tree->rdc.valid   = 1;
        pc_tree->partition   = PARTITION_SPLIT;
        array_update_part    = last_quad_valid ? pc_tree->split[3] : NULL;
    }

    // When current depth is selected, this array update is for the 3rd quadrant (which is not updated in
    // svt_aom_pick_partition to avoid redundant copies). Check that the block is available, since it may be
    // an out of bounds block (the previous, in bound, quadrants would have updated the relevant neighbour
    // arrays in svt_aom_pick_partition) and check that it is not further subdivided, in which case the neighbour
    // arrays would already be updated.
    if (array_update_part && array_update_part->partition != PARTITION_SPLIT) {
        mode_decision_update_neighbor_arrays_light_pd0(ctx, array_update_part);
    }

    return true;
}

/*
 * Loop over all passed blocks in an SB and perform mode decision for each block,
 * then output the optimal mode distribution/partitioning for the given SB.
 *
 * For each block, selects the best mode through multiple MD stages (accuracy increases
 * while the number of mode candidates decreases as you move from one stage to another).
 * Based on the block costs, selects the best partition for a parent block (if NSQ
 * shapes are present). Finally, performs inter-depth decision towards a final partitiioning.
 */
bool svt_aom_pick_partition_lpd0(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds,
                                 PC_TREE* pc_tree, int mi_row, int mi_col) {
    // get the input picture; if high bit-depth, pad the input pic
    EbPictureBufferDesc* input_pic = pcs->ppcs->enhanced_pic;

    pc_tree->mi_row    = mi_row;
    pc_tree->mi_col    = mi_col;
    pc_tree->rdc.valid = 0;
    // Neighbour partition array is not updated in PD0, so set neighbour info to invalid.
    pc_tree->left_part_ctx  = 0;
    pc_tree->above_part_ctx = 0;

    // Check that shape is valid, and adjust tested blocks so only valid blocks are tested
    // LPD0 assumes one block per shape
    if (mds->tot_shapes) {
        const Part shape    = mds->shapes[0];
        const int  mi_rows  = pcs->ppcs->av1_cm->mi_rows;
        const int  mi_cols  = pcs->ppcs->av1_cm->mi_cols;
        const int  hbs      = mi_size_wide[mds->bsize] >> 1;
        const bool has_rows = mi_row + hbs < mi_rows;
        const bool has_cols = mi_col + hbs < mi_cols;
        if ((!has_rows && !has_cols) || (!has_cols && shape != PART_V) || (!has_rows && shape != PART_H)) {
            mds->tot_shapes = 0;
        }
    }

    if (mds->tot_shapes) {
        const Part     shape       = mds->shapes[0];
        const uint32_t blk_idx_mds = mds->mds_idx + ns_blk_offset_md[shape];

        ctx->blk_geom                 = get_blk_geom_mds(scs->blk_geom_mds, blk_idx_mds);
        pc_tree->block_data[shape][0] = ctx->blk_ptr = &ctx->md_blk_arr_nsq[blk_idx_mds];

        // LPD0 always uses one block (first block in the shape) so no NSQ offset to mi_row/col needed
        init_block_data(pcs, ctx, mi_row, mi_col, shape, blk_idx_mds);
        md_encode_block_light_pd0(pcs, ctx, input_pic);
        pc_tree->tested_blk[shape][0] = true;

        pc_tree->rdc.rd_cost = pc_tree->block_data[shape][0]->cost;
        pc_tree->rdc.valid   = 1;
        pc_tree->partition   = from_shape_to_part[shape];
#if !CLN_REMOVE_VAR_SUB_DEPTH
        if (ctx->var_skip_sub_depth_ctrls.enabled && mds->split_flag && pc_tree->tested_blk[PART_N][0]) {
            if (ctx->blk_geom->sq_size <= ctx->var_skip_sub_depth_ctrls.max_size &&
                ctx->blk_geom->sq_size >= ctx->var_skip_sub_depth_ctrls.min_size && var_skip_sub_depth(pcs, ctx)) {
                mds->split_flag = false;
            }
        }
#endif
    }

    if (mds->split_flag) {
        const bool valid_part = test_split_partition_lpd0(scs, pcs, ctx, mds, pc_tree, mi_row, mi_col);
        if (!valid_part && pc_tree->rdc.valid) {
            mode_decision_update_neighbor_arrays_light_pd0(ctx, pc_tree);
        }
    } else if (pc_tree->rdc.valid && mds->index < 3) {
        mode_decision_update_neighbor_arrays_light_pd0(ctx, pc_tree);
    }

    return pc_tree->rdc.valid;
}

static void test_split_partition_lpd1(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx,
                                      MdScan* mds, PC_TREE* pc_tree, int mi_row, int mi_col) {
    const int mi_rows = pcs->ppcs->av1_cm->mi_rows;
    const int mi_cols = pcs->ppcs->av1_cm->mi_cols;
    const int mi_step = mi_size_wide[pc_tree->bsize] / 2;
    for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
        const int x_idx = (i & 1) * mi_step;
        const int y_idx = (i >> 1) * mi_step;

        // if block fully outside pic, don't process
        if (mi_row + y_idx >= mi_rows || mi_col + x_idx >= mi_cols) {
            continue;
        }
        svt_aom_pick_partition_lpd1(scs, pcs, ctx, mds->split[i], pc_tree->split[i], mi_row + y_idx, mi_col + x_idx);
    }
    pc_tree->partition = PARTITION_SPLIT;
}

/*
 * Select the best partitioning and modes for the passed block. Recursively search lower subpartitions of the passed block.
 * Output the optimal mode distribution/partitioning for the given SB.
 *
 * For each block, selects the best mode through multiple MD stages (accuracy increases
 * while the number of mode candidates decreases as you move from one stage to another).
 * Based on the block costs, selects the best partition for a parent block (if NSQ
 * shapes are present). Finally, performs inter-depth decision towards a final partitiioning.
 */
void svt_aom_pick_partition_lpd1(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds,
                                 PC_TREE* pc_tree, int mi_row, int mi_col) {
    // LPD1 assumes 8bit MD; if it's 16bit and bypassing encdec, the high bit depth pic will
    // be used in MDS3 only to generate a conformant recon.
    EbPictureBufferDesc* input_pic = pcs->ppcs->enhanced_pic;

    pc_tree->mi_row    = mi_row;
    pc_tree->mi_col    = mi_col;
    pc_tree->rdc.valid = 0;

    // Check that shape is valid, and adjust tested blocks so only valid blocks are tested.
    // LPD1 assumes one block per shape
    if (mds->tot_shapes) {
        const Part shape    = mds->shapes[0];
        const int  mi_rows  = pcs->ppcs->av1_cm->mi_rows;
        const int  mi_cols  = pcs->ppcs->av1_cm->mi_cols;
        const int  hbs      = mi_size_wide[mds->bsize] >> 1;
        const bool has_rows = mi_row + hbs < mi_rows;
        const bool has_cols = mi_col + hbs < mi_cols;
        if ((!has_rows && !has_cols) || (!has_cols && shape != PART_V) || (!has_rows && shape != PART_H)) {
            mds->tot_shapes = 0;
        }
    }

    // Test current depth if flagged to be tested
    if (mds->tot_shapes) {
        // LPD1 does not support NSQ shapes, except for H/V at the picture boundaries. In all cases,
        // only a single block would be tested (either SQ, or the first block in the H/V shape
        // which is allowed in the picture).
        const Part     shape       = mds->shapes[0];
        const uint32_t blk_idx_mds = mds->mds_idx + ns_blk_offset_md[shape];

        // Get the blk_geom and blk_ptr for the current block within the shape being tested
        ctx->blk_geom                 = get_blk_geom_mds(scs->blk_geom_mds, blk_idx_mds);
        pc_tree->block_data[shape][0] = ctx->blk_ptr = &ctx->md_blk_arr_nsq[blk_idx_mds];

        // LPD1 assumes a fixed partition structure, so partition neighbour arrays (blk_ptr->left_neighbor_partition and
        // blk_ptr->above_neighbor_partition) are not updated, and the neighbour arrays will not be accessed, since the
        // partition rate is not needed (i.e. no calls to svt_aom_partition_rate_cost).
        // LPD1 always uses one block (first block in the shape) so no NSQ offset to mi_row/col needed.
        init_block_data(pcs, ctx, mi_row, mi_col, shape, blk_idx_mds);

        // Encode the block
        md_encode_block_light_pd1(pcs, ctx, input_pic);
        pc_tree->tested_blk[shape][0] = true;

        // LPD1 uses a fixed partition structure, so no need to update cost
        pc_tree->partition = from_shape_to_part[shape];

        // The current block is the last at a given d1 level (b/c fixed partition); update d2 info
        // Always update the partition context array because may be needed for other SBs which
        // have NSQ or multiple depths enabled
        struct PartitionContext partition;
        partition.above = partition_context_lookup[ctx->blk_geom->bsize].above;
        partition.left  = partition_context_lookup[ctx->blk_geom->bsize].left;

        svt_aom_neighbor_array_unit_mode_write(ctx->leaf_partition_na,
                                               (uint8_t*)(&partition),
                                               ctx->blk_org_x,
                                               ctx->blk_org_y,
                                               ctx->blk_geom->bwidth,
                                               ctx->blk_geom->bheight,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        // If TXS enabled at picture level, there are necessary context updates
        if (pcs->ppcs->frm_hdr.tx_mode == TX_MODE_SELECT) {
            uint8_t tx_size = tx_depth_to_tx_size[ctx->blk_ptr->block_mi.tx_depth][ctx->blk_geom->bsize];
            uint8_t bw      = tx_size_wide[tx_size];
            uint8_t bh      = tx_size_high[tx_size];

            svt_aom_neighbor_array_unit_mode_write(ctx->txfm_context_array,
                                                   &bw,
                                                   ctx->blk_org_x,
                                                   ctx->blk_org_y,
                                                   ctx->blk_geom->bwidth,
                                                   ctx->blk_geom->bheight,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_MASK);

            svt_aom_neighbor_array_unit_mode_write(ctx->txfm_context_array,
                                                   &bh,
                                                   ctx->blk_org_x,
                                                   ctx->blk_org_y,
                                                   ctx->blk_geom->bwidth,
                                                   ctx->blk_geom->bheight,
                                                   NEIGHBOR_ARRAY_UNIT_LEFT_MASK);
        }
        // Define temp mi_row/col to prevent overwriting the variables, which are forwarded
        // when splitting the partition. This should not matter since the partition is fixed.
        // Call to partition_mi_offset required to get block size for NSQ shapes at pic boundary.
        int       temp_mi_row = mi_row;
        int       temp_mi_col = mi_col;
        BlockSize sub_bsize   = partition_mi_offset(pc_tree->bsize, shape, 0 /*nsi*/, &temp_mi_row, &temp_mi_col);
        svt_aom_update_mi_map(pcs, ctx, pc_tree->partition, sub_bsize, temp_mi_row, temp_mi_col);
    }

    // ready for next depth
    if (mds->split_flag) {
        test_split_partition_lpd1(scs, pcs, ctx, mds, pc_tree, mi_row, mi_col);
    }
}

/*
Update the above and left neighbour partition for the square block. This is used in deriving the partition rate
in svt_aom_partition_rate_cost.  The partition is always signaled with respect to the top left corner of the square
block, so only derive the neighbours for the square blocks (even if they will not be tested).  Only square blocks
are passed to svt_aom_partition_rate_cost.
*/
static void update_part_neighs(ModeDecisionContext* ctx, PC_TREE* pc_tree, const int mi_row, const int mi_col) {
    if (ctx->pd_pass == PD_PASS_0) {
        pc_tree->left_part_ctx  = 0;
        pc_tree->above_part_ctx = 0;
        return;
    }

    const uint32_t blk_org_x = mi_col << MI_SIZE_LOG2;
    const uint32_t blk_org_y = mi_row << MI_SIZE_LOG2;

    NeighborArrayUnit* leaf_partition_na             = ctx->leaf_partition_na;
    const uint32_t     partition_left_neighbor_index = get_neighbor_array_unit_left_index(leaf_partition_na, blk_org_y);
    const uint32_t     partition_above_neighbor_index = get_neighbor_array_unit_top_index(leaf_partition_na, blk_org_x);

    // Generate Partition context
    pc_tree->above_part_ctx =
        (((PartitionContext*)leaf_partition_na->top_array)[partition_above_neighbor_index].above ==
         (char)INVALID_NEIGHBOR_DATA)
        ? 0
        : ((PartitionContext*)leaf_partition_na->top_array)[partition_above_neighbor_index].above;

    pc_tree->left_part_ctx = (((PartitionContext*)leaf_partition_na->left_array)[partition_left_neighbor_index].left ==
                              (char)INVALID_NEIGHBOR_DATA)
        ? 0
        : ((PartitionContext*)leaf_partition_na->left_array)[partition_left_neighbor_index].left;
}

void svt_aom_init_sb_data(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx) {
    // Update neighbour arrays for the SB
    if (ctx->pd_pass == PD_PASS_0 && ctx->lpd0_ctrls.pd0_level > REGULAR_PD0) {
        if (!ctx->skip_intra) {
            update_neighbour_arrays_light_pd0(pcs, ctx);
        }
    } else {
        update_neighbour_arrays(pcs, ctx);
    }

    // If high bit-depth, pad the input pic. Done once for SB.
    // If using 8bit MD but bypassing EncDec, will need the 16bit pic for MDS3.
    if (ctx->hbd_md || (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1)) {
        pad_hbd_pictures(scs, pcs, ctx, pcs->ppcs->enhanced_pic);
    }

    ctx->coded_area_sb       = 0;
    ctx->coded_area_sb_uv    = 0;
    ctx->params_status       = 0;
    ctx->copied_neigh_arrays = 0;
    if (ctx->pd_pass == PD_PASS_1 && ctx->lpd1_ctrls.pd1_level > REGULAR_PD1) {
        ctx->tx_depth              = 0;
        ctx->txb_itr               = 0;
        ctx->txb_1d_offset         = 0;
        ctx->luma_txb_skip_context = 0;
        ctx->luma_dc_sign_context  = 0;
        ctx->cb_txb_skip_context   = 0;
        ctx->cb_dc_sign_context    = 0;
        ctx->cr_txb_skip_context   = 0;
        ctx->cr_dc_sign_context    = 0;
        ctx->ind_uv_avail          = 0;
    } else if (ctx->pd_pass == PD_PASS_0 && ctx->lpd0_ctrls.pd0_level > REGULAR_PD0) {
        // Set SB-level variables here
        ctx->tx_depth                 = 0;
        ctx->txb_1d_offset            = 0;
        ctx->txb_itr                  = 0;
        ctx->luma_txb_skip_context    = 0;
        ctx->luma_dc_sign_context     = 0;
        ctx->mds_do_rdoq              = true;
        ctx->mds_do_spatial_sse       = false;
        ctx->mds_fast_coeff_est_level = ctx->rate_est_ctrls.pd0_fast_coeff_est_level; //ON  !!!
        ctx->ind_uv_avail             = 0;
    }
}

static bool test_split_partition(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds,
                                 PC_TREE* pc_tree, int mi_row, int mi_col) {
    const int mi_rows = pcs->ppcs->av1_cm->mi_rows;
    const int mi_cols = pcs->ppcs->av1_cm->mi_cols;
    // For properly set MdScan, this should never be true. This check is added as a safeguard, in case
    // we try testing a block that is completely outside the picture boundaries.
    if (mi_row >= mi_rows || mi_col >= mi_cols) {
        return false;
    }
    // if hbd_md is 0, we may still use 10bit lambda to generate final costs if we are bypassing encdec for 10bit content.
    const bool     used_10bit_at_mds3 = (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec &&
                                     ctx->pd_pass == PD_PASS_1 && svt_aom_do_md_recon(pcs->ppcs, ctx));
    const uint32_t full_lambda        = ctx->hbd_md || used_10bit_at_mds3 ? ctx->full_sb_lambda_md[EB_10_BIT_MD]
                                                                          : ctx->full_sb_lambda_md[EB_8_BIT_MD];
    int64_t        above_split_rate   = svt_aom_partition_rate_cost(pcs->ppcs,
                                                           pc_tree->bsize,
                                                           mi_row,
                                                           mi_col,
                                                           ctx->md_rate_est_ctx,
                                                           PARTITION_SPLIT,
                                                           pc_tree->left_part_ctx,
                                                           pc_tree->above_part_ctx);

    // If not using accurate partition rate, bias against splitting by increasing the rate of SPLIT partition
    if (!pcs->ppcs->use_accurate_part_ctx) {
        above_split_rate *= 2;
    }
    int64_t split_cost = RDCOST(full_lambda, above_split_rate, 0);

    bool      last_quad_valid = true;
    const int mi_step         = mi_size_wide[pc_tree->bsize] / 2;
    for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
        const int x_idx = (i & 1) * mi_step;
        const int y_idx = (i >> 1) * mi_step;

        // if block fully outside pic, don't process
        if (mi_row + y_idx >= mi_rows || mi_col + x_idx >= mi_cols) {
            // If any quadrant is out of bounds, the last quadrant will be
            last_quad_valid = false;
            continue;
        }

        // Check current depth cost; if larger than parent, exit early
        if (pc_tree->rdc.valid) {
            assert(!(ctx->pd_pass == PD_PASS_1 && ctx->pred_depth_only) &&
                   "If PD1 and pred depth only, parent depth cost should be unavailable");
            const uint32_t th = (i == 0)
                ? (ctx->depth_early_exit_ctrls.split_cost_th == 0 ? 1000 : ctx->depth_early_exit_ctrls.split_cost_th)
                : (ctx->depth_early_exit_ctrls.early_exit_th == 0 ? 1000 : ctx->depth_early_exit_ctrls.early_exit_th);
            if ((pc_tree->rdc.rd_cost * th * ctx->parent_cost_bias) <= (split_cost * 1000 * 1000)) {
                return false;
            }
        }

        const bool valid_split_partition = svt_aom_pick_partition(
            scs, pcs, ctx, mds->split[i], pc_tree->split[i], mi_row + y_idx, mi_col + x_idx);

        if (!valid_split_partition) {
            // If split is invalid, then exit (all
            // split quadrants must be valid for split to be selected).
            return false;
        }
        split_cost += pc_tree->split[i]->rdc.rd_cost;
    }

    // Only get here if all partitions are valid (and/or out of bounds).
    PC_TREE* array_update_part = pc_tree;
    if (pc_tree->rdc.valid && (ctx->parent_cost_bias * pc_tree->rdc.rd_cost <= split_cost * 1000)) {
        pc_tree->rdc.valid = 1;
    } else {
        pc_tree->rdc.rd_cost = split_cost;
        pc_tree->rdc.valid   = 1;
        pc_tree->partition   = PARTITION_SPLIT;
        array_update_part    = last_quad_valid ? pc_tree->split[3] : NULL;
    }

    // When current depth is selected, this array update is for the 3rd quadrant (which is not updated in
    // svt_aom_pick_partition to avoid redundant copies). Check that the block is available, since it may be
    // an out of bounds block (the previous, in bound, quadrants would have updated the relevant neighbour
    // arrays in svt_aom_pick_partition) and check that it is not further subdivided, in which case the neighbour
    // arrays would already be updated.
    if (array_update_part && array_update_part->partition != PARTITION_SPLIT) {
        md_update_all_neighbour_arrays_multiple(pcs, ctx, array_update_part);
    }

    return true;
}

static bool test_depth(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds,
                       PC_TREE* pc_tree, const int mi_row, const int mi_col) {
#if TUNE_STILL_IMAGE
    const bool allintra = scs->allintra;
    const bool rtc_tune = scs->static_config.rtc;
#endif
    EbPictureBufferDesc* input_pic        = ctx->hbd_md ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
    const uint32_t       base_blk_idx_mds = mds->mds_idx;

    // Reset settings, in case they were over-written by previous block
    // Only reset settings when features that change settings are used.
    if (ctx->params_status == 1) {
#if TUNE_STILL_IMAGE
        allintra       ? svt_aom_sig_deriv_enc_dec_allintra(pcs, ctx)
            : rtc_tune ? svt_aom_sig_deriv_enc_dec_rtc(pcs, ctx)
                       : svt_aom_sig_deriv_enc_dec_default(pcs, ctx);

#else
        svt_aom_sig_deriv_enc_dec(scs, pcs, ctx);
#endif
        ctx->params_status = 0;
    }

    // Copy neighbour arrays to temp buffer for later reuse if testing more than 1 NSQ shape
    // or will be splitting (SQ doesn't need to update neighbour arrays)
    const bool copy_neigh_arrays = (mds->tot_shapes > 2) || mds->split_flag ||
        (mds->tot_shapes > 1 && mds->shapes[0] != PART_N);

    const int mi_rows      = pcs->ppcs->av1_cm->mi_rows;
    const int mi_cols      = pcs->ppcs->av1_cm->mi_cols;
    const int hbs          = mi_size_wide[mds->bsize] >> 1;
    const int quarter_step = mi_size_wide[mds->bsize] >> 2;

    // if hbd_md is 0, we may still use 10bit lambda to generate final costs if we are bypassing encdec for 10bit content.
    const bool     used_10bit_at_mds3 = (ctx->encoder_bit_depth > EB_EIGHT_BIT && ctx->bypass_encdec &&
                                     ctx->pd_pass == PD_PASS_1 && svt_aom_do_md_recon(pcs->ppcs, ctx));
    const uint32_t full_lambda        = ctx->hbd_md || used_10bit_at_mds3 ? ctx->full_sb_lambda_md[EB_10_BIT_MD]
                                                                          : ctx->full_sb_lambda_md[EB_8_BIT_MD];
    // Loop over all shapes set to be tested at the current depth
    for (uint32_t shape_idx = 0; shape_idx < mds->tot_shapes; shape_idx++) {
        const Part shape           = mds->shapes[shape_idx];
        uint8_t    shape_block_cnt = num_ns_per_shape[shape];
        uint32_t   blk_idx_mds     = base_blk_idx_mds +
            (pc_tree->bsize == BLOCK_128X128 ? ns_blk_offset_128_md[shape] : ns_blk_offset_md[shape]);

        // Check that shape is valid, and adjust tested blocks so only valid blocks are tested
        const bool has_rows = mi_row + hbs < mi_rows;
        const bool has_cols = mi_col + hbs < mi_cols;
        if ((!has_rows && !has_cols) || (!has_cols && shape != PART_V) || (!has_rows && shape != PART_H)) {
            continue;
        } else if (!has_rows || !has_cols || (shape == PART_H4 && mi_row + 3 * quarter_step >= mi_rows) ||
                   (shape == PART_V4 && mi_col + 3 * quarter_step >= mi_cols)) {
            shape_block_cnt--;
        }

        const int64_t part_rate  = svt_aom_partition_rate_cost(pcs->ppcs,
                                                              pc_tree->bsize,
                                                              mi_row,
                                                              mi_col,
                                                              ctx->md_rate_est_ctx,
                                                              from_shape_to_part[shape],
                                                              pc_tree->left_part_ctx,
                                                              pc_tree->above_part_ctx);
        int64_t       part_cost  = RDCOST(full_lambda, part_rate, 0);
        bool          valid_part = true;

        for (uint32_t nsi = 0; nsi < shape_block_cnt; nsi++, blk_idx_mds++) {
            // Get the blk_geom and blk_ptr for the current block within the shape being tested
            ctx->blk_geom                   = get_blk_geom_mds(scs->blk_geom_mds, blk_idx_mds);
            pc_tree->block_data[shape][nsi] = ctx->blk_ptr = &ctx->md_blk_arr_nsq[blk_idx_mds];

            // Get the origin of the current block (within the NSQ shape). Don't want to overwrite
            // mi_row/col, which should remain the SQ origin since that will be reused for processing
            // future shapes.
            int temp_mi_row = mi_row;
            int temp_mi_col = mi_col;
            partition_mi_offset(pc_tree->bsize, shape, nsi, &temp_mi_row, &temp_mi_col);
            init_block_data(pcs, ctx, temp_mi_row, temp_mi_col, shape, blk_idx_mds);

            // If performing NSQ search, take shortcuts to reduce NSQ overhead
            if (shape != PART_N && nsi == 0) {
                // Update settings for the NSQ(s) for certain speed features
                if (pcs->slice_type != I_SLICE) {
                    faster_md_settings_nsq(pcs, ctx, pc_tree, mds->is_child);
                }

                // call nsq-reduction func if NSQ is on
                if (get_skip_processing_nsq_block(pcs, ctx, pc_tree, mds)) {
                    valid_part = false;
                    break;
                }
            }

            if (ctx->copied_neigh_arrays && nsi == 0) {
                // Copy info of SQ block
                svt_aom_copy_neighbour_arrays( //restore [1] in [0] after done last ns block
                    pcs,
                    ctx,
                    NSQ_NEIGHBOR_ARRAY_INDEX,
                    MD_NEIGHBOR_ARRAY_INDEX,
                    pc_tree->bsize,
                    mi_row,
                    mi_col);
            }

            // encode the current block (unless it's redundant, then copy the data from redundant blk)
            if (!ctx->redundant_blk || !update_redundant(pcs, ctx, pc_tree, nsi)) {
                md_encode_block(pcs, ctx, pc_tree, mds, input_pic);
            }
            pc_tree->tested_blk[shape][nsi] = true;

            part_cost += pc_tree->block_data[shape][nsi]->cost;

            if (pc_tree->rdc.valid && part_cost >= pc_tree->rdc.rd_cost) {
                valid_part = false;
                break;
            }

            // Copy neighbour arrays to temp buffer for later reuse if testing more than 1 NSQ shape
            // or will be splitting (SQ doesn't need to update neighbour arrays)
            if (nsi + 1 < shape_block_cnt) {
                if (!ctx->copied_neigh_arrays && copy_neigh_arrays) {
                    // Save info for whole SQ block
                    svt_aom_copy_neighbour_arrays( //save a clean neigh in [1], encode uses [0], reload the clean in [0] after done last ns block in a partition
                        pcs,
                        ctx,
                        MD_NEIGHBOR_ARRAY_INDEX,
                        NSQ_NEIGHBOR_ARRAY_INDEX,
                        pc_tree->bsize,
                        mi_row,
                        mi_col);
                    ctx->copied_neigh_arrays = 1;
                }
                assert(pc_tree->tested_blk[shape][nsi]);
                md_update_all_neighbour_arrays(pcs, ctx, pc_tree, shape, nsi);
            }
        }

        if (valid_part) {
            if (!pc_tree->rdc.valid || part_cost < pc_tree->rdc.rd_cost) {
                pc_tree->partition   = from_shape_to_part[shape];
                pc_tree->rdc.rd_cost = part_cost;
                pc_tree->rdc.valid   = 1;
            }
        }
    }

    return pc_tree->rdc.valid;
}

/*
 * Select the best partitioning and modes for the passed block. Recursively search lower subpartitions of the passed block.
 * Output the optimal mode distribution/partitioning for the given SB.
 *
 * For each block, selects the best mode through multiple MD stages (accuracy increases
 * while the number of mode candidates decreases as you move from one stage to another).
 * Based on the block costs, selects the best partition for a parent block (if NSQ
 * shapes are present). Finally, performs inter-depth decision towards a final partitiioning.
 */
bool svt_aom_pick_partition(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx, MdScan* mds,
                            PC_TREE* pc_tree, int mi_row, int mi_col) {
    pc_tree->mi_row    = mi_row;
    pc_tree->mi_col    = mi_col;
    pc_tree->rdc.valid = 0;

    // Update the left and above partition neighbours for the square block, which are used to derive
    // the partition rate
    update_part_neighs(ctx, pc_tree, mi_row, mi_col);

    // Test current depth if shapes are set to be tested
    if (mds->tot_shapes) {
        test_depth(scs, pcs, ctx, mds, pc_tree, mi_row, mi_col);

        if (ctx->skip_sub_depth_ctrls.enabled && ctx->blk_geom->sq_size <= ctx->skip_sub_depth_ctrls.max_size &&
            mds->split_flag && // could be further splitted
            pc_tree->tested_blk[PART_N][0] && // valid block
            eval_sub_depth_skip_cond1(ctx, pc_tree)) {
            mds->split_flag = false;
        }

        // Now have checked all d1 blocks, so update d2 info
        if (ctx->copied_neigh_arrays && mds->split_flag) {
            // Copy data for SQ block
            svt_aom_copy_neighbour_arrays( //restore [1] in [0] after done last ns block
                pcs,
                ctx,
                NSQ_NEIGHBOR_ARRAY_INDEX,
                MD_NEIGHBOR_ARRAY_INDEX,
                pc_tree->bsize,
                mi_row,
                mi_col);
        }

        ctx->copied_neigh_arrays = 0;
    }

    // Test lower depths if flagged to be tested
    if (mds->split_flag) {
        const bool valid_part = test_split_partition(scs, pcs, ctx, mds, pc_tree, mi_row, mi_col);
        if (!valid_part && pc_tree->rdc.valid) {
            md_update_all_neighbour_arrays_multiple(pcs, ctx, pc_tree);
        }
    } else if (pc_tree->rdc.valid && mds->index < 3) {
        md_update_all_neighbour_arrays_multiple(pcs, ctx, pc_tree);
    }

    return pc_tree->rdc.valid;
}
