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

/***************************************
* Includes
***************************************/
#include <stdio.h>
#include <stdlib.h>
#include <limits.h>

#include "common_utils.h"
#include "definitions.h"
#include "sequence_control_set.h"
#include "mode_decision.h"
#include "md_process.h"
#include "motion_estimation.h"

#include "av1me.h"
#include "hash.h"
#include "enc_inter_prediction.h"
#include "rd_cost.h"
#include "aom_dsp_rtcd.h"
#include "svt_log.h"
#include "resize.h"
#include "mcomp.h"
#include "ac_bias.h"
#include "src_ops_process.h"
#include "utility.h"
#include "adaptive_mv_pred.h"

static const uint32_t intra_luma_to_chroma[INTRA_MODES] = {
    UV_DC_PRED, // Average of above and left pixels
    UV_V_PRED, // Vertical
    UV_H_PRED, // Horizontal
    UV_D45_PRED, // Directional 45  degree
    UV_D135_PRED, // Directional 135 degree
    UV_D113_PRED, // Directional 113 degree
    UV_D157_PRED, // Directional 157 degree
    UV_D203_PRED, // Directional 203 degree
    UV_D67_PRED, // Directional 67  degree
    UV_SMOOTH_PRED, // Combination of horizontal and vertical interpolation
    UV_SMOOTH_V_PRED, // Vertical interpolation
    UV_SMOOTH_H_PRED, // Horizontal interpolation
    UV_PAETH_PRED, // Predict from the direction of smallest gradient
};

void calc_target_weighted_pred(PictureControlSet* pcs, ModeDecisionContext* ctx, const Av1Common* cm,
                               const MacroBlockD* xd, int mi_row, int mi_col, const uint8_t* above, int above_stride,
                               const uint8_t* left, int left_stride);
#define INC_MD_CAND_CNT(cnt, max_can_count)                  \
    MULTI_LINE_MACRO_BEGIN                                   \
    if (cnt + 1 < max_can_count)                             \
        cnt++;                                               \
    else                                                     \
        SVT_ERROR("Mode decision candidate count exceeded"); \
    MULTI_LINE_MACRO_END

#define SUPERRES_INVALID_STATE 0x7fffffff

bool svt_av1_is_lossless_segment(PictureControlSet* pcs, int8_t segment_id) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    if (frm_hdr->segmentation_params.segmentation_enabled) {
        return pcs->lossless[segment_id];
    } else {
        return pcs->lossless[0];
    }
}

static bool check_mv_validity(int16_t x_mv, int16_t y_mv, uint8_t need_shift) {
    Mv mv;
    //go to 1/8th if input is 1/4pel
    mv.y = y_mv << need_shift;
    mv.x = x_mv << need_shift;
    /* AV1 limits
      -16384 < MV_x_in_1/8 or MV_y_in_1/8 < 16384
      which means in full pel:
      -2048 < MV_x_in_full_pel or MV_y_in_full_pel < 2048
    */
    if (!is_mv_valid(&mv)) {
        return false;
    }
    return true;
}

int svt_is_interintra_allowed(uint8_t enable_inter_intra, BlockSize bsize, PredictionMode mode,
                              const MvReferenceFrame ref_frame[2]) {
    return enable_inter_intra && svt_aom_is_interintra_allowed_bsize((const BlockSize)bsize) &&
        svt_aom_is_interintra_allowed_mode(mode) && svt_aom_is_interintra_allowed_ref(ref_frame);
}

int svt_aom_filter_intra_allowed_bsize(BlockSize bs) {
    return block_size_wide[bs] <= 32 && block_size_high[bs] <= 32;
}

int svt_aom_filter_intra_allowed(uint8_t enable_filter_intra, BlockSize bsize, uint8_t palette_size, uint32_t mode) {
    return enable_filter_intra && mode == DC_PRED && palette_size == 0 && svt_aom_filter_intra_allowed_bsize(bsize);
}

// returns the max inter-inter compound type based on settings and block size
static MD_COMP_TYPE get_tot_comp_types_bsize(MD_COMP_TYPE tot_comp_types, BlockSize bsize) {
    return (svt_aom_get_wedge_params_bits(bsize) == 0) ? MIN(tot_comp_types, MD_COMP_WEDGE) : tot_comp_types;
}

/*
Get the ME offset for a given block (the offset used to locate the PA MVs from the parent PCS).
*/
uint32_t svt_aom_get_me_block_offset(const uint32_t org_x, const uint32_t org_y, const BlockSize bsize,
                                     const uint8_t enable_me_8x8, const uint8_t enable_me_16x16) {
    const int      bwidth     = block_size_wide[bsize];
    const int      bheight    = block_size_high[bsize];
    const uint32_t max_length = MAX(bwidth, bheight);

    uint32_t me_idx = 0;
    switch (max_length) {
    case 4:
    case 8:
        me_idx++;
        if (org_x & 8) { // (org_x % 16) / 8
            me_idx += 1;
        }
        if (org_y & 8) { // (org_y % 16) / 8
            me_idx += 2;
        }
        AOM_FALLTHROUGH_INTENDED;
    case 16:
        me_idx++;
        if (org_x & 16) { // (org_x % 32) / 16
            me_idx += 5;
        }
        if (org_y & 16) { // (org_y % 32) / 16
            me_idx += 10;
        }
        AOM_FALLTHROUGH_INTENDED;
    case 32:
        me_idx++;
        if (org_x & 32) { // (org_x % 64) / 32
            me_idx += 21;
        }
        if (org_y & 32) { // (org_y % 64) / 32
            me_idx += 42;
        }
        break;
    default:
        // me_idx = 0;
        break;
    }

    uint32_t me_block_offset = me_idx_85[me_idx]; // convert idx to me_idx

    if (!enable_me_8x8) {
        if (me_block_offset >= MAX_SB64_PU_COUNT_NO_8X8) {
            me_block_offset = me_idx_85_8x8_to_16x16_conversion[me_block_offset - MAX_SB64_PU_COUNT_NO_8X8];
        }
        assert(me_block_offset < 21);
        if (!enable_me_16x16) {
            if (me_block_offset >= MAX_SB64_PU_COUNT_WO_16X16) {
                assert(me_block_offset < 21);
                me_block_offset = me_idx_16x16_to_parent_32x32_conversion[me_block_offset - MAX_SB64_PU_COUNT_WO_16X16];
            }
        }
    }

    return me_block_offset;
}

//Given one reference frame identified by the pair (list_index,ref_index)
//indicate if ME data is valid
uint8_t svt_aom_is_me_data_present(uint32_t me_block_offset, uint32_t me_cand_offset, const MeSbResults* me_results,
                                   uint8_t list_idx, uint8_t ref_idx) {
    uint8_t            total_me_cnt     = me_results->total_me_candidate_index[me_block_offset];
    const MeCandidate* me_block_results = &me_results->me_candidate_array[me_cand_offset];
    for (uint32_t me_cand_i = 0; me_cand_i < total_me_cnt; ++me_cand_i) {
        const MeCandidate* me_cand = &me_block_results[me_cand_i];
        assert(me_cand->direction <= 2);
        if (me_cand->direction == 0 || me_cand->direction == 2) {
            if (list_idx == me_cand->ref0_list && ref_idx == me_cand->ref_idx_l0) {
                return 1;
            }
        }
        if (me_cand->direction == 1 || me_cand->direction == 2) {
            if (list_idx == me_cand->ref1_list && ref_idx == me_cand->ref_idx_l1) {
                return 1;
            }
        }
    }
    return 0;
}

/********************************************
* Constants
********************************************/
// 1 - Regular uni-pred ,
// 2 - Regular uni-pred + Wedge compound Inter Intra
// 3 - Regular uni-pred + Wedge compound Inter Intra + Smooth compound Inter Intra

#if CONFIG_ENABLE_OBMC
static bool warped_motion_mode_allowed(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    return frm_hdr->allow_warped_motion && has_overlappable_candidates(ctx->blk_ptr) && ctx->blk_geom->bwidth >= 8 &&
        ctx->blk_geom->bheight >= 8 && ctx->wm_ctrls.enabled;
}
#endif
MotionMode svt_aom_obmc_motion_mode_allowed(
    const PictureControlSet* pcs, ModeDecisionContext* ctx, const BlockSize bsize,
    uint8_t          situation, // 0: candidate(s) preparation, 1: data preparation, 2: simple translation face-off
    MvReferenceFrame rf0, MvReferenceFrame rf1, PredictionMode mode) {
    if (ctx->obmc_ctrls.trans_face_off && !situation) {
        return SIMPLE_TRANSLATION;
    }
    // check if should cap the max block size for obmc

    if (block_size_wide[bsize] > ctx->obmc_ctrls.max_blk_size ||
        block_size_high[bsize] > ctx->obmc_ctrls.max_blk_size) {
        return SIMPLE_TRANSLATION;
    }
    if (!ctx->obmc_ctrls.enabled) {
        return SIMPLE_TRANSLATION;
    }
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;

    if (!frm_hdr->is_motion_mode_switchable) {
        return SIMPLE_TRANSLATION;
    }

    if (frm_hdr->force_integer_mv == 0) {
        const TransformationType gm_type = pcs->ppcs->global_motion[rf0].wmtype;
        if (is_global_mv_block(mode, bsize, gm_type)) {
            return SIMPLE_TRANSLATION;
        }
    }
    if (is_motion_variation_allowed_bsize(bsize) && is_inter_singleref_mode(mode) && rf1 != INTRA_FRAME &&
        !(rf1 > INTRA_FRAME)) // is_motion_variation_allowed_compound
    {
        if (!has_overlappable_candidates(ctx->blk_ptr)) { // check_num_overlappable_neighbors
            return SIMPLE_TRANSLATION;
        }

        return OBMC_CAUSAL;
    } else {
        return SIMPLE_TRANSLATION;
    }
}

//static uint32_t  AntiContouringIntraMode[11] = { EB_INTRA_PLANAR, EB_INTRA_DC, EB_INTRA_HORIZONTAL, EB_INTRA_VERTICAL,
//EB_INTRA_MODE_2, EB_INTRA_MODE_6, EB_INTRA_MODE_14, EB_INTRA_MODE_18, EB_INTRA_MODE_22, EB_INTRA_MODE_30, EB_INTRA_MODE_34 };
int32_t svt_aom_have_newmv_in_inter_mode(PredictionMode mode) {
    return (mode == NEWMV || mode == NEW_NEWMV || mode == NEAREST_NEWMV || mode == NEW_NEARESTMV ||
            mode == NEAR_NEWMV || mode == NEW_NEARMV);
}

static MvReferenceFrame to_ref_frame[2][4] = {{LAST_FRAME, LAST2_FRAME, LAST3_FRAME, GOLDEN_FRAME},
                                              {BWDREF_FRAME, ALTREF2_FRAME, ALTREF_FRAME, INVALID_REF}};

MvReferenceFrame svt_get_ref_frame_type(uint8_t list, uint8_t ref_idx) {
    return to_ref_frame[list][ref_idx];
};

uint8_t svt_aom_get_max_drl_index(uint8_t refmvCnt, PredictionMode mode) {
    uint8_t max_drl = 0;

    if (mode == NEWMV || mode == NEW_NEWMV) {
        if (refmvCnt < 2) {
            max_drl = 1;
        } else if (refmvCnt == 2) {
            max_drl = 2;
        } else {
            max_drl = 3;
        }
    }

    if (mode == NEARMV || mode == NEAR_NEARMV || mode == NEAR_NEWMV || mode == NEW_NEARMV) {
        if (refmvCnt < 3) {
            max_drl = 1;
        } else if (refmvCnt == 3) {
            max_drl = 2;
        } else {
            max_drl = 3;
        }
    }

    return max_drl;
}

#define MV_COST_WEIGHT 108

static int64_t pick_interintra_wedge(PictureControlSet* pcs, ModeDecisionContext* ctx, const BlockSize bsize,
                                     const uint8_t* const p0, const uint8_t* const p1, uint8_t* src_buf,
                                     uint32_t src_stride, int8_t* wedge_index_out) {
    assert(svt_aom_is_interintra_wedge_used(bsize));
    // assert(cpi->common.seq_params.enable_interintra_compound);

    const int bw = block_size_wide[bsize];
    const int bh = block_size_high[bsize];
    DECLARE_ALIGNED(32, int16_t, residual1[MAX_INTERINTRA_SB_SQUARE]); // src - pred1
    DECLARE_ALIGNED(32, int16_t, diff10[MAX_INTERINTRA_SB_SQUARE]); // pred1 - pred0
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    if (ctx->hbd_md) {
        svt_aom_highbd_subtract_block(bh, bw, residual1, bw, src_buf, src_stride, p1, bw, EB_TEN_BIT);
        svt_aom_highbd_subtract_block(bh, bw, diff10, bw, p1, bw, p0, bw, EB_TEN_BIT);

    } else
#endif
    {
        svt_aom_subtract_block(bh, bw, residual1, bw, src_buf, src_stride, p1, bw);
        svt_aom_subtract_block(bh, bw, diff10, bw, p1, bw, p0, bw);
    }

    int8_t  wedge_index = -1;
    int64_t rd          = pick_wedge_fixed_sign(pcs, ctx, bsize, residual1, diff10, 0, &wedge_index);
    *wedge_index_out    = wedge_index;

    return rd;
}

static void inter_intra_search(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidate* cand) {
    SequenceControlSet* scs = pcs->scs;
    DECLARE_ALIGNED(16, uint8_t, tmp_buf[2 * MAX_INTERINTRA_SB_SQUARE]);
    DECLARE_ALIGNED(16, uint8_t, ii_pred_buf[2 * MAX_INTERINTRA_SB_SQUARE]);
    // get inter pred for ref0
    EbPictureBufferDesc* src_pic     = ctx->hbd_md ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
    uint16_t*            src_buf_hbd = (uint16_t*)src_pic->buffer_y + (ctx->blk_org_x + src_pic->org_x) +
        (ctx->blk_org_y + src_pic->org_y) * src_pic->stride_y;
    uint8_t* src_buf = src_pic->buffer_y + (ctx->blk_org_x + src_pic->org_x) +
        (ctx->blk_org_y + src_pic->org_y) * src_pic->stride_y;

    uint8_t  bit_depth   = ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT;
    uint32_t full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];

    uint32_t            bwidth  = ctx->blk_geom->bwidth;
    uint32_t            bheight = ctx->blk_geom->bheight;
    EbPictureBufferDesc pred_desc;
    pred_desc.org_x = pred_desc.org_y = 0;
    pred_desc.stride_y                = bwidth;

    EbPictureBufferDesc* ref_pic_list0 = svt_aom_get_ref_pic_buffer(pcs, cand->block_mi.ref_frame[0]);
    EbPictureBufferDesc* ref_pic_list1 = NULL;

    // Use scaled references if resolution of the reference is different from that of the input
    // Only have one ref
    if (ref_pic_list0 != NULL) {
        uint8_t list_idx0  = get_list_idx(cand->block_mi.ref_frame[0]);
        int8_t  ref_idx_l0 = get_ref_frame_idx(cand->block_mi.ref_frame[0]);
        svt_aom_use_scaled_rec_refs_if_needed(
            pcs,
            pcs->ppcs->enhanced_pic,
            (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx0][ref_idx_l0]->object_ptr,
            &ref_pic_list0,
            ctx->hbd_md);
    }
    pred_desc.buffer_y = tmp_buf;

    //we call the regular inter prediction path here (no compound)
    cand->block_mi.interp_filters     = 0;
    cand->block_mi.is_interintra_used = 0;
    svt_aom_inter_prediction(scs,
                             pcs,
                             &cand->block_mi,
                             &cand->wm_params_l0,
                             &cand->wm_params_l1,
                             ctx->blk_ptr,
                             ctx->blk_geom->bsize,
                             ctx->shape,
                             false, // use_precomputed_obmc
                             false, // use_precomputed_ii - ii not performed here
                             ctx,
                             NULL,
                             NULL,
                             NULL,
                             ref_pic_list0,
                             ref_pic_list1,
                             ctx->blk_org_x,
                             ctx->blk_org_y,
                             &pred_desc, //output
                             0, //output org_x,
                             0, //output org_y,
                             PICTURE_BUFFER_DESC_LUMA_MASK,
                             ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
                             0); // is_16bit_pipeline

    assert(svt_aom_is_interintra_wedge_used(ctx->blk_geom->bsize)); //if not I need to add nowedge path!!

    int64_t        best_interintra_rd   = INT64_MAX;
    InterIntraMode best_interintra_mode = INTERINTRA_MODES;
    for (int j = 0; j < INTERINTRA_MODES; ++j) {
        // if ((!cpi->oxcf.enable_smooth_intra || cpi->sf.disable_smooth_intra) &&
        //     (InterIntraMode)j == II_SMOOTH_PRED)
        //   continue;
        InterIntraMode interintra_mode = (InterIntraMode)j;
        // rmode = interintra_mode_cost[mbmi->interintra_mode];
        const int bsize_group = eb_size_group_lookup[ctx->blk_geom->bsize];
        const int rmode       = ctx->md_rate_est_ctx->inter_intra_mode_fac_bits[bsize_group][interintra_mode];
        // av1_combine_interintra(xd, bsize, 0, tmp_buf, bw, intrapred, bw);
        if (ctx->hbd_md) {
            svt_aom_combine_interintra_highbd(interintra_mode, // mode,
                                              0, // use_wedge_interintra,
                                              0, // cand->interintra_wedge_index,
                                              0, // int wedge_sign,
                                              ctx->blk_geom->bsize,
                                              ctx->blk_geom->bsize, // plane_bsize,
                                              ii_pred_buf,
                                              bwidth, /*uint8_t *comppred, int compstride,*/
                                              tmp_buf,
                                              bwidth, /*const uint8_t *interpred, int interstride,*/
                                              ctx->intrapred_buf[j],
                                              bwidth /*const uint8_t *intrapred,   int intrastride*/,
                                              bit_depth);
        } else {
            svt_aom_combine_interintra(interintra_mode, //mode,
                                       0, //use_wedge_interintra,
                                       0, //cand->interintra_wedge_index,
                                       0, //int wedge_sign,
                                       ctx->blk_geom->bsize,
                                       ctx->blk_geom->bsize, // plane_bsize,
                                       ii_pred_buf,
                                       bwidth, /*uint8_t *comppred, int compstride,*/
                                       tmp_buf,
                                       bwidth, /*const uint8_t *interpred, int interstride,*/
                                       ctx->intrapred_buf[j],
                                       bwidth /*const uint8_t *intrapred,   int intrastride*/);
        }
        int64_t rd;
        if (ctx->inter_intra_comp_ctrls.use_rd_model) {
            int     rate_sum;
            int64_t dist_sum;
            model_rd_for_sb_with_curvfit(pcs,
                                         ctx,
                                         ctx->blk_geom->bsize,
                                         bwidth,
                                         bheight,
                                         ctx->hbd_md ? (uint8_t*)src_buf_hbd : src_buf,
                                         src_pic->stride_y,
                                         ii_pred_buf,
                                         bwidth,
                                         0,
                                         0,
                                         0,
                                         0,
                                         &rate_sum,
                                         &dist_sum,
                                         NULL,
                                         NULL,
                                         NULL);

            rd = RDCOST(full_lambda, rate_sum + rmode, dist_sum);
        } else {
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
            if (ctx->hbd_md) {
                rd = svt_aom_highbd_sse((uint8_t*)src_buf_hbd, src_pic->stride_y, ii_pred_buf, bwidth, bwidth, bheight);
            } else
#endif
            {
                rd = svt_aom_sse(src_buf, src_pic->stride_y, ii_pred_buf, bwidth, bwidth, bheight);
            }
        }
        if (rd < best_interintra_rd) {
            best_interintra_rd             = rd;
            cand->block_mi.interintra_mode = best_interintra_mode = interintra_mode;
        }
    }
    // To test: Enable wedge search if source variance and edge strength are above the thresholds.
    //CHKN need to re-do intra pred using the winner, or have a separate intra serch for wedge
    int64_t       best_interintra_rd_wedge = INT64_MAX;
    const uint8_t ii_wedge_mode            = ctx->shape == PART_N ? ctx->inter_intra_comp_ctrls.wedge_mode_sq
                                                                  : ctx->inter_intra_comp_ctrls.wedge_mode_nsq;
    if (ii_wedge_mode) {
        best_interintra_rd_wedge = pick_interintra_wedge(pcs,
                                                         ctx,
                                                         ctx->blk_geom->bsize,
                                                         ctx->intrapred_buf[best_interintra_mode],
                                                         tmp_buf,
                                                         ctx->hbd_md ? (uint8_t*)src_buf_hbd : src_buf,
                                                         src_pic->stride_y,
                                                         &cand->block_mi.interintra_wedge_index);
    }

    // for ii_wedge_mode 1, always inject wedge as a separate candidate; for wedge mode 2 only inject
    // if wedge is better than non-wedge
    if (ii_wedge_mode == 1 || best_interintra_rd_wedge < best_interintra_rd) {
        cand->block_mi.use_wedge_interintra = 1;
    } else {
        cand->block_mi.use_wedge_interintra = 0;
    }
}

static COMPOUND_TYPE to_av1_compound_lut[] = {COMPOUND_AVERAGE, COMPOUND_DISTWTD, COMPOUND_DIFFWTD, COMPOUND_WEDGE};

static void determine_compound_mode(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidate* cand,
                                    MD_COMP_TYPE cur_type) {
    BlockModeInfo* block_mi        = &cand->block_mi;
    block_mi->interinter_comp.type = to_av1_compound_lut[cur_type];
    switch (cur_type) {
    case MD_COMP_AVG:
        block_mi->comp_group_idx = 0;
        block_mi->compound_idx   = 1;
        break;
    case MD_COMP_DIST:
        block_mi->comp_group_idx = 0;
        block_mi->compound_idx   = 0;
        break;
    case MD_COMP_DIFF0:
        block_mi->comp_group_idx            = 1;
        block_mi->compound_idx              = 1;
        block_mi->interinter_comp.mask_type = 55;
        svt_aom_search_compound_diff_wedge(pcs, ctx, cand);
        break;
    case MD_COMP_WEDGE:
        block_mi->comp_group_idx = 1;
        block_mi->compound_idx   = 1;
        svt_aom_search_compound_diff_wedge(pcs, ctx, cand);
        break;
    default:
        SVT_ERROR("not used comp type\n");
        assert(0);
        break;
    }
}

void svt_aom_choose_best_av1_mv_pred(ModeDecisionContext* ctx, MvReferenceFrame ref_frame,
                                     PredictionMode mode, // NEW or NEW_NEW
                                     Mv mv0, Mv mv1,
                                     uint8_t* bestDrlIndex, // output
                                     Mv       best_pred_mv[2] // output
) {
    if (ctx->shut_fast_rate) {
        return;
    }
    if (ctx->approx_inter_rate > 1) {
        *bestDrlIndex   = 0;
        best_pred_mv[0] = ctx->ref_mv_stack[ref_frame][0].this_mv;
        best_pred_mv[1] = ctx->ref_mv_stack[ref_frame][0].comp_mv;
        return;
    }
    int16_t mv0x = mv0.x;
    int16_t mv0y = mv0.y;
    int16_t mv1x = mv1.x;
    int16_t mv1y = mv1.y;

    uint8_t is_compound = is_inter_compound_mode(mode);

    struct MdRateEstimationContext* md_rate_est_ctx = ctx->md_rate_est_ctx;
    BlkStruct*                      blk_ptr         = ctx->blk_ptr;
    uint8_t                         max_drl_index;
    Mv                              nearestmv[2] = {{{0}}, {{0}}};
    Mv                              nearmv[2];
    Mv                              ref_mv[2];
    Mv                              mv;

    max_drl_index = svt_aom_get_max_drl_index(blk_ptr->av1xd->ref_mv_count[ref_frame], mode);
    // max_drl_index = 1;

    if (max_drl_index == 1) {
        *bestDrlIndex = 0;

        best_pred_mv[0] = ctx->ref_mv_stack[ref_frame][0].this_mv;
        best_pred_mv[1] = ctx->ref_mv_stack[ref_frame][0].comp_mv;
    } else {
        uint8_t  drli;
        uint32_t best_mv_cost = 0xFFFFFFFF;
        for (drli = 0; drli < max_drl_index; drli++) {
            svt_aom_get_av1_mv_pred_drl(ctx, blk_ptr, ref_frame, is_compound, mode, drli, nearestmv, nearmv, ref_mv);

            //compute the rate for this drli Cand
            mv.y             = mv0y;
            mv.x             = mv0x;
            uint32_t mv_rate = 0;
            if (ctx->approx_inter_rate) {
                mv_rate = (uint32_t)svt_av1_mv_bit_cost_light(&mv, &(ref_mv[0]));
            } else {
                mv_rate = (uint32_t)svt_av1_mv_bit_cost(
                    &mv, &(ref_mv[0]), md_rate_est_ctx->nmv_vec_cost, md_rate_est_ctx->nmvcoststack, MV_COST_WEIGHT);
            }

            if (is_compound) {
                mv.y = mv1y;
                mv.x = mv1x;
                if (ctx->approx_inter_rate) {
                    mv_rate += (uint32_t)svt_av1_mv_bit_cost_light(&mv, &(ref_mv[1]));
                } else {
                    mv_rate += (uint32_t)svt_av1_mv_bit_cost(&mv,
                                                             &(ref_mv[1]),
                                                             md_rate_est_ctx->nmv_vec_cost,
                                                             md_rate_est_ctx->nmvcoststack,
                                                             MV_COST_WEIGHT);
                }
            }

            const int32_t new_mv = (mode == NEWMV || mode == NEW_NEWMV);
            if (new_mv) {
                int32_t idx;
                for (idx = 0; idx < 2; ++idx) {
                    if (blk_ptr->av1xd->ref_mv_count[ref_frame] > idx + 1) {
                        uint8_t drl_1_ctx = av1_drl_ctx(&(ctx->ref_mv_stack[ref_frame][0]), idx);
                        mv_rate += ctx->md_rate_est_ctx->drl_mode_fac_bits[drl_1_ctx][drli != idx];
                        if (drli == idx) {
                            break;
                        }
                    }
                }
            }

            if (mv_rate < best_mv_cost) {
                best_mv_cost    = mv_rate;
                *bestDrlIndex   = drli;
                best_pred_mv[0] = ref_mv[0];
                best_pred_mv[1] = ref_mv[1];
            }
        }
    }
}

static void mode_decision_cand_bf_dctor(EbPtr p) {
    ModeDecisionCandidateBuffer* obj = (ModeDecisionCandidateBuffer*)p;
    EB_DELETE(obj->pred);
    EB_DELETE(obj->rec_coeff);
    EB_DELETE(obj->quant);
}

static void mode_decision_scratch_cand_bf_dctor(EbPtr p) {
    ModeDecisionCandidateBuffer* obj = (ModeDecisionCandidateBuffer*)p;
    EB_DELETE(obj->pred);
    EB_DELETE(obj->residual);
    EB_DELETE(obj->rec_coeff);
    EB_DELETE(obj->recon);
    EB_DELETE(obj->quant);
}

/***************************************
* Mode Decision Candidate Ctor
***************************************/
EbErrorType svt_aom_mode_decision_cand_bf_ctor(ModeDecisionCandidateBuffer* buffer_ptr, EbBitDepth max_bitdepth,
                                               uint8_t sb_size, uint32_t buffer_desc_mask,
                                               EbPictureBufferDesc* temp_residual, EbPictureBufferDesc* temp_recon_ptr,
                                               uint64_t* fast_cost, uint64_t* full_cost, uint64_t* full_cost_ssim) {
    EbPictureBufferDescInitData picture_buffer_desc_init_data;

    EbPictureBufferDescInitData thirty_two_width_picture_buffer_desc_init_data;

    buffer_ptr->dctor = mode_decision_cand_bf_dctor;

    // Init Picture Data
    picture_buffer_desc_init_data.max_width          = sb_size;
    picture_buffer_desc_init_data.max_height         = sb_size;
    picture_buffer_desc_init_data.bit_depth          = max_bitdepth;
    picture_buffer_desc_init_data.color_format       = EB_YUV420;
    picture_buffer_desc_init_data.buffer_enable_mask = buffer_desc_mask;
    picture_buffer_desc_init_data.left_padding       = 0;
    picture_buffer_desc_init_data.right_padding      = 0;
    picture_buffer_desc_init_data.top_padding        = 0;
    picture_buffer_desc_init_data.bot_padding        = 0;
    picture_buffer_desc_init_data.split_mode         = false;
    picture_buffer_desc_init_data.is_16bit_pipeline  = max_bitdepth > EB_EIGHT_BIT;

    thirty_two_width_picture_buffer_desc_init_data.max_width          = sb_size;
    thirty_two_width_picture_buffer_desc_init_data.max_height         = sb_size;
    thirty_two_width_picture_buffer_desc_init_data.bit_depth          = EB_THIRTYTWO_BIT;
    thirty_two_width_picture_buffer_desc_init_data.color_format       = EB_YUV420;
    thirty_two_width_picture_buffer_desc_init_data.buffer_enable_mask = buffer_desc_mask;
    thirty_two_width_picture_buffer_desc_init_data.left_padding       = 0;
    thirty_two_width_picture_buffer_desc_init_data.right_padding      = 0;
    thirty_two_width_picture_buffer_desc_init_data.top_padding        = 0;
    thirty_two_width_picture_buffer_desc_init_data.bot_padding        = 0;
    thirty_two_width_picture_buffer_desc_init_data.split_mode         = false;
    thirty_two_width_picture_buffer_desc_init_data.is_16bit_pipeline  = true;

    // Candidate Ptr
    buffer_ptr->cand = NULL;

    // Video Buffers
    EB_NEW(buffer_ptr->pred, svt_picture_buffer_desc_ctor, (EbPtr)&picture_buffer_desc_init_data);
    // Reuse the residual_ptr memory in MD context
    buffer_ptr->residual = temp_residual;
    EB_NEW(buffer_ptr->rec_coeff, svt_picture_buffer_desc_ctor, (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);
    EB_NEW(buffer_ptr->quant, svt_picture_buffer_desc_ctor, (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);
    // Reuse the recon_ptr memory in MD context
    buffer_ptr->recon = temp_recon_ptr;

    // Costs
    buffer_ptr->fast_cost      = fast_cost;
    buffer_ptr->full_cost      = full_cost;
    buffer_ptr->full_cost_ssim = full_cost_ssim;
    return EB_ErrorNone;
}

EbErrorType svt_aom_mode_decision_scratch_cand_bf_ctor(ModeDecisionCandidateBuffer* buffer_ptr, uint8_t sb_size,
                                                       EbBitDepth max_bitdepth) {
    EbPictureBufferDescInitData picture_buffer_desc_init_data;
    EbPictureBufferDescInitData double_width_picture_buffer_desc_init_data;
    EbPictureBufferDescInitData thirty_two_width_picture_buffer_desc_init_data;

    buffer_ptr->dctor = mode_decision_scratch_cand_bf_dctor;

    // Init Picture Data
    picture_buffer_desc_init_data.max_width                           = sb_size;
    picture_buffer_desc_init_data.max_height                          = sb_size;
    picture_buffer_desc_init_data.bit_depth                           = max_bitdepth;
    picture_buffer_desc_init_data.color_format                        = EB_YUV420;
    picture_buffer_desc_init_data.buffer_enable_mask                  = PICTURE_BUFFER_DESC_FULL_MASK;
    picture_buffer_desc_init_data.left_padding                        = 0;
    picture_buffer_desc_init_data.right_padding                       = 0;
    picture_buffer_desc_init_data.top_padding                         = 0;
    picture_buffer_desc_init_data.bot_padding                         = 0;
    picture_buffer_desc_init_data.split_mode                          = false;
    picture_buffer_desc_init_data.is_16bit_pipeline                   = max_bitdepth > EB_EIGHT_BIT;
    double_width_picture_buffer_desc_init_data.max_width              = sb_size;
    double_width_picture_buffer_desc_init_data.max_height             = sb_size;
    double_width_picture_buffer_desc_init_data.bit_depth              = EB_SIXTEEN_BIT;
    double_width_picture_buffer_desc_init_data.color_format           = EB_YUV420;
    double_width_picture_buffer_desc_init_data.buffer_enable_mask     = PICTURE_BUFFER_DESC_FULL_MASK;
    double_width_picture_buffer_desc_init_data.left_padding           = 0;
    double_width_picture_buffer_desc_init_data.right_padding          = 0;
    double_width_picture_buffer_desc_init_data.top_padding            = 0;
    double_width_picture_buffer_desc_init_data.bot_padding            = 0;
    double_width_picture_buffer_desc_init_data.split_mode             = false;
    double_width_picture_buffer_desc_init_data.is_16bit_pipeline      = true;
    thirty_two_width_picture_buffer_desc_init_data.max_width          = sb_size;
    thirty_two_width_picture_buffer_desc_init_data.max_height         = sb_size;
    thirty_two_width_picture_buffer_desc_init_data.bit_depth          = EB_THIRTYTWO_BIT;
    thirty_two_width_picture_buffer_desc_init_data.color_format       = EB_YUV420;
    thirty_two_width_picture_buffer_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    thirty_two_width_picture_buffer_desc_init_data.left_padding       = 0;
    thirty_two_width_picture_buffer_desc_init_data.right_padding      = 0;
    thirty_two_width_picture_buffer_desc_init_data.top_padding        = 0;
    thirty_two_width_picture_buffer_desc_init_data.bot_padding        = 0;
    thirty_two_width_picture_buffer_desc_init_data.split_mode         = false;
    thirty_two_width_picture_buffer_desc_init_data.is_16bit_pipeline  = true;

    // Candidate Ptr
    buffer_ptr->cand = NULL;

    // Video Buffers
    EB_NEW(buffer_ptr->pred, svt_picture_buffer_desc_ctor, (EbPtr)&picture_buffer_desc_init_data);
    EB_NEW(buffer_ptr->residual, svt_picture_buffer_desc_ctor, (EbPtr)&double_width_picture_buffer_desc_init_data);
    EB_NEW(buffer_ptr->rec_coeff, svt_picture_buffer_desc_ctor, (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);
    EB_NEW(buffer_ptr->quant, svt_picture_buffer_desc_ctor, (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);

    EB_NEW(buffer_ptr->recon, svt_picture_buffer_desc_ctor, (EbPtr)&picture_buffer_desc_init_data);
    return EB_ErrorNone;
}

/***************************************
* return true if the MV candidate is already injected
***************************************/
static bool mv_is_already_injected(ModeDecisionContext* ctx, Mv mv0, Mv mv1, uint8_t ref_type) {
    MvReferenceFrame rf[2];
    av1_set_ref_frame(rf, ref_type);

    // Unipred Candidate
    if (rf[1] <= INTRA_FRAME) {
        // First check the validity of the candidate MV, and exit if invalid MV
        if (ctx->corrupted_mv_check && !check_mv_validity(mv0.x, mv0.y, 0)) {
            return true;
        }

        for (int cand_idx = 0; cand_idx < ctx->injected_mv_count; cand_idx++) {
            if (ctx->injected_ref_types[cand_idx] == ref_type && ctx->injected_mvs[cand_idx][0].as_int == mv0.as_int) {
                return true;
            }
        }
    } else { // Bipred Candidate
        // First check the validity of the candidate MV, and exit if invalid MV
        if (ctx->corrupted_mv_check && (!check_mv_validity(mv0.x, mv0.y, 0) || !check_mv_validity(mv1.x, mv1.y, 0))) {
            return true;
        }

        RedundantCandCtrls* redund_ctrls = &ctx->cand_reduction_ctrls.redundant_cand_ctrls;
        if (redund_ctrls->score_th) {
            uint8_t is_high_mag = (ABS(mv0.x) > redund_ctrls->mag_th) && (ABS(mv0.y) > redund_ctrls->mag_th) &&
                (ABS(mv1.x) > redund_ctrls->mag_th) && (ABS(mv1.y) > redund_ctrls->mag_th);
            for (int cand_idx = 0; cand_idx < ctx->injected_mv_count; cand_idx++) {
                if (ctx->injected_ref_types[cand_idx] == ref_type) {
                    int score = ABS(ctx->injected_mvs[cand_idx][0].x - mv0.x) +
                        ABS(ctx->injected_mvs[cand_idx][0].y - mv0.y) + ABS(ctx->injected_mvs[cand_idx][1].x - mv1.x) +
                        ABS(ctx->injected_mvs[cand_idx][1].y - mv1.y);

                    if (score == 0 || (score < redund_ctrls->score_th && is_high_mag)) {
                        return true;
                    }
                }
            }
        } else {
            for (int cand_idx = 0; cand_idx < ctx->injected_mv_count; cand_idx++) {
                if (ctx->injected_ref_types[cand_idx] == ref_type &&
                    ctx->injected_mvs[cand_idx][0].as_int == mv0.as_int &&
                    ctx->injected_mvs[cand_idx][1].as_int == mv1.as_int) {
                    return true;
                }
            }
        }
    }
    return false;
}

bool svt_aom_is_valid_unipred_ref(ModeDecisionContext* ctx, uint8_t inter_cand_group, uint8_t list_idx,
                                  uint8_t ref_idx) {
    if (!ctx->ref_pruning_ctrls.enabled) {
        return true;
    }
    if (!ctx->ref_filtering_res[inter_cand_group][list_idx][ref_idx].do_ref &&
        (ref_idx || !ctx->ref_pruning_ctrls.closest_refs[inter_cand_group])) {
        return false;
    } else {
        return true;
    }
}

// Determine if the MV-to-MVP difference satisfies the mv_diff restriction
static bool is_valid_mv_diff(Mv best_pred_mv[2], Mv mv0, Mv mv1, uint8_t is_compound) {
    const uint8_t mv_diff_max_bit = MV_IN_USE_BITS;

    if (abs(mv0.x - best_pred_mv[0].x) > (1 << mv_diff_max_bit) ||
        abs(mv0.y - best_pred_mv[0].y) > (1 << mv_diff_max_bit)) {
        return false;
    }

    if (is_compound) {
        if (abs(mv1.x - best_pred_mv[1].x) > (1 << mv_diff_max_bit) ||
            abs(mv1.y - best_pred_mv[1].y) > (1 << mv_diff_max_bit)) {
            return false;
        }
    }
    return true;
}

static bool is_valid_bipred_ref(ModeDecisionContext* ctx, uint8_t inter_cand_group, uint8_t list_idx_0,
                                uint8_t ref_idx_0, uint8_t list_idx_1, uint8_t ref_idx_1) {
    if (!ctx->ref_pruning_ctrls.enabled) {
        return true;
    }
    // Both ref should be 1 for bipred refs to be valid: if 1 is not best_refs then there is a chance to exit the injection
    if (!ctx->ref_filtering_res[inter_cand_group][list_idx_0][ref_idx_0].do_ref ||
        !ctx->ref_filtering_res[inter_cand_group][list_idx_1][ref_idx_1].do_ref) {
        // Check whether we should check the closest, if no then there no need to move forward and return false
        if (!ctx->ref_pruning_ctrls.closest_refs[inter_cand_group]) {
            return false;
        }

        // Else check if ref are LAST and BWD, if not then return false
        if (ref_idx_0 || ref_idx_1) {
            return false;
        }
    }
    return true;
}

#define BIPRED_3x3_REFINMENT_POSITIONS 8

static int8_t allow_refinement_flag[BIPRED_3x3_REFINMENT_POSITIONS] = {1, 0, 1, 0, 1, 0, 1, 0};
static int8_t bipred_3x3_x_pos[BIPRED_3x3_REFINMENT_POSITIONS]      = {-1, -1, 0, 1, 1, 1, 0, -1};
static int8_t bipred_3x3_y_pos[BIPRED_3x3_REFINMENT_POSITIONS]      = {0, 1, 1, 1, 0, -1, -1, -1};

#if OPT_PER_BLK_INTRA
static INLINE uint8_t is_dc_only_safe(PictureControlSet* pcs, ModeDecisionContext* ctx) {
#if FIX_IS_DC_ONLY_SAFE
    // Early exit if pruning not enabled, SB-128, NSQ, or 4x4 (no variance available)
    if (!ctx->intra_ctrls.prune_using_edge_info || pcs->scs->super_block_size == 128 || ctx->shape != PART_N ||
        ctx->blk_geom->sq_size == 4) {
        return 0;
    }

    // Block variance lookup
    int            blk_idx;
    int            sub_idx[4];
    const Position blk_org = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
    svt_aom_get_blk_var_map(ctx->blk_geom->sq_size, blk_org.x, blk_org.y, &blk_idx, sub_idx);

    uint16_t* sb_var  = pcs->ppcs->variance[ctx->sb_index];
    uint32_t  blk_var = sb_var[blk_idx];

    // For 8x8, we do not have 4x4 sub-variance, skip spread check
    if (ctx->blk_geom->sq_size == 8) {
        return (blk_var < 2000);
    }

    // For 16x16 and above, compute spread from sub-blocks
    uint32_t min_var = UINT32_MAX;
    uint32_t max_var = 0;

    for (int i = 0; i < 4; i++) {
        uint32_t v = sb_var[sub_idx[i]];
        min_var    = MIN(min_var, v);
        max_var    = MAX(max_var, v);
    }

    uint32_t spread_var = max_var - min_var;

    return (blk_var < 2000 && spread_var < 4000);
#else
    // Early exit if pruning not enabled, SB-128, NSQ
    if (!ctx->intra_ctrls.prune_using_edge_info || pcs->scs->super_block_size == 128 || ctx->shape != PART_N) {
        return 0;
    }

    // Block variance lookup
    int blk_idx;
    int sub_idx[4];
    // Get origin of the block relative to SB origin
    const Position blk_org = {.x = ctx->blk_org_x - ctx->sb_origin_x, .y = ctx->blk_org_y - ctx->sb_origin_y};
    svt_aom_get_blk_var_map(ctx->blk_geom->sq_size, blk_org.x, blk_org.y, &blk_idx, sub_idx);

    uint16_t* sb_var  = pcs->ppcs->variance[ctx->sb_index];
    uint32_t  blk_var = sb_var[blk_idx];

    uint32_t min_var = UINT32_MAX;
    uint32_t max_var = 0;

    for (int i = 0; i < 4; i++) {
        uint32_t v = sb_var[sub_idx[i]];
        min_var    = MIN(min_var, v);
        max_var    = MAX(max_var, v);
    }

    uint32_t spread_var = max_var - min_var;

    // Safe if uniform block
    return (blk_var < 2000 && spread_var < 4000);
#endif
}
#endif
// Inject inter-intra, WM, OBMC for unipred simple-trans candidate
//
// total_cand_count is the index to ctx->fast_cand_array for the next candidate injected (which is the
// same as the number of candidates injected so far).  It is assumed the simple-trans candidate to base
// the other candidtes on is the previously injected candidate (at index total_cand_count - 1).
//
// enable_ii, enable_wm, and enable_obmc allow the caller to disable some modes explicitly; if enabled, the
// mode will be injected if the block size/candidate type supports the mode. The enable signals are left as
// arguments because some candidates do not inject all modes (e.g. unipred does not inject WM/OBMC).
static void inj_non_simple_modes(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* total_cand_count,
                                 const bool enable_ii, const bool enable_wm, const bool enable_obmc) {
    // index of simple translation candidate (to be used to copy cand info for other modes)
    // assumes the simple trans cand is the previously injected candidate
    const uint32_t                     simple_trans_cand_idx = *total_cand_count - 1;
    const ModeDecisionCandidate* const simple_trans_cand     = &ctx->fast_cand_array[simple_trans_cand_idx];

    // The candidate count to be used to track number of inj cands, and the index of fast_cand_array for new candidates
    uint32_t cand_count = *total_cand_count;

    assert(simple_trans_cand->block_mi.ref_frame[1] == NONE_FRAME);
    const uint8_t list_idx = get_list_idx(simple_trans_cand->block_mi.ref_frame[0]);
    const uint8_t ref_idx  = get_ref_frame_idx(simple_trans_cand->block_mi.ref_frame[0]);

    // INJECT INTER-INTRA
    const uint8_t is_ii_allowed = svt_aom_is_valid_unipred_ref(ctx, INTER_INTRA_GROUP, list_idx, ref_idx) &&
        svt_is_interintra_allowed(ctx->inter_intra_comp_ctrls.enabled,
                                  ctx->blk_geom->bsize,
                                  simple_trans_cand->block_mi.mode,
                                  simple_trans_cand->block_mi.ref_frame);
    if (enable_ii && is_ii_allowed) {
        ModeDecisionCandidate* cand = &ctx->fast_cand_array[cand_count];
        svt_memcpy(cand, simple_trans_cand, sizeof(ModeDecisionCandidate));

        inter_intra_search(pcs, ctx, cand);
        cand->block_mi.is_interintra_used = 1;
        cand->block_mi.ref_frame[1]       = INTRA_FRAME;
        const InterIntraMode ii_mode      = cand->block_mi.interintra_mode;
        INC_MD_CAND_CNT(cand_count, pcs->ppcs->max_can_count);

        // if ii_wedge_mode is 1, then inject wedge/non-wedge as separate candidates; OW, only inject the best (above)
        const uint8_t ii_wedge_mode = ctx->shape == PART_N ? ctx->inter_intra_comp_ctrls.wedge_mode_sq
                                                           : ctx->inter_intra_comp_ctrls.wedge_mode_nsq;
        if (ii_wedge_mode == 1) {
            cand = &ctx->fast_cand_array[cand_count];
            svt_memcpy(cand, simple_trans_cand, sizeof(ModeDecisionCandidate));

            cand->block_mi.is_interintra_used   = 1;
            cand->block_mi.ref_frame[1]         = INTRA_FRAME;
            cand->block_mi.interintra_mode      = ii_mode;
            cand->block_mi.use_wedge_interintra = 0;
            INC_MD_CAND_CNT(cand_count, pcs->ppcs->max_can_count);
        }
    }

#if CONFIG_ENABLE_OBMC
    // INJECT WARP
    const uint8_t is_warp_allowed = warped_motion_mode_allowed(pcs, ctx) &&
        svt_aom_is_valid_unipred_ref(ctx, WARP_GROUP, list_idx, ref_idx);
    if (enable_wm && is_warp_allowed) {
        ModeDecisionCandidate* cand = &ctx->fast_cand_array[cand_count];
        svt_memcpy(cand, simple_trans_cand, sizeof(ModeDecisionCandidate));

        cand->block_mi.is_interintra_used = 0;
        cand->block_mi.motion_mode        = WARPED_CAUSAL;
        cand->wm_params_l0.wmtype         = AFFINE;

        uint8_t motion_mode_valid = 1;
        if (cand->block_mi.mode == NEWMV && ctx->wm_ctrls.refinement_iterations && ctx->wm_ctrls.refine_level == 0) {
            // Perform refinement; if refinement is off, then MV is valid, since it's been checked above
            motion_mode_valid = svt_aom_wm_motion_refinement(pcs, ctx, cand, 0);
        }

        if (motion_mode_valid) {
            motion_mode_valid = svt_aom_warped_motion_parameters(ctx,
                                                                 cand->block_mi.mv[0],
                                                                 ctx->blk_geom,
                                                                 cand->block_mi.ref_frame[0],
                                                                 &cand->wm_params_l0,
                                                                 &cand->block_mi.num_proj_ref,
                                                                 ctx->wm_ctrls.lower_band_th,
                                                                 ctx->wm_ctrls.upper_band_th,
                                                                 0);
        }

        if (motion_mode_valid) {
            INC_MD_CAND_CNT(cand_count, pcs->ppcs->max_can_count);
        }
    }

    // INJECT OBMC
    const uint8_t is_obmc_allowed = svt_aom_is_valid_unipred_ref(ctx, OBMC_GROUP, list_idx, ref_idx) &&
        (svt_aom_obmc_motion_mode_allowed(pcs,
                                          ctx,
                                          ctx->blk_geom->bsize,
                                          0,
                                          simple_trans_cand->block_mi.ref_frame[0],
                                          simple_trans_cand->block_mi.ref_frame[1],
                                          simple_trans_cand->block_mi.mode) == OBMC_CAUSAL);
    if (enable_obmc && is_obmc_allowed) {
        ModeDecisionCandidate* cand = &ctx->fast_cand_array[cand_count];
        svt_memcpy(cand, simple_trans_cand, sizeof(ModeDecisionCandidate));

        cand->block_mi.is_interintra_used = 0;
        cand->block_mi.motion_mode        = OBMC_CAUSAL;

        uint8_t motion_mode_valid = 1;
        if (cand->block_mi.mode == NEWMV && ctx->obmc_ctrls.refine_level == 0) {
            assert(cand->block_mi.ref_frame[1] == NONE_FRAME);
            motion_mode_valid = svt_aom_obmc_motion_refinement(pcs, ctx, cand, ctx->obmc_ctrls.refine_level);
        }

        if (motion_mode_valid) {
            INC_MD_CAND_CNT(cand_count, pcs->ppcs->max_can_count);
        }
    }
#else
    UNUSED(enable_wm);
    UNUSED(enable_obmc);
#endif // CONFIG_ENABLE_OBMC

    *total_cand_count = cand_count;
}

// Determines if inter MVP compound modes should be skipped based on info from neighbouring blocks/ref frame types.
static bool skip_compound_on_ref_types(ModeDecisionContext* ctx, MvReferenceFrame rf[2]) {
    if (!ctx->inter_comp_ctrls.skip_on_ref_info) {
        return false;
    }

    MacroBlockD* xd = ctx->blk_ptr->av1xd;

    // If both references are from the same list, skip compound
    const uint8_t list_idx_0 = get_list_idx(rf[0]);
    const uint8_t list_idx_1 = get_list_idx(rf[1]);
    if (list_idx_0 == list_idx_1) {
        return true;
    }

    // Skip compound unless neighbours selected the ref frames
    bool skip_comp = true;
    if (!xd->left_available && !xd->up_available) {
        return false;
    }

    if (xd->left_available) {
        const BlockModeInfo* const left_mi = &xd->left_mbmi->block_mi;
        if ((is_inter_singleref_mode(left_mi->mode) &&
             (left_mi->ref_frame[0] == rf[0] || left_mi->ref_frame[0] == rf[1])) ||
            (is_inter_compound_mode(left_mi->mode) &&
             (left_mi->ref_frame[0] == rf[0] && left_mi->ref_frame[1] == rf[1]))) {
            return false;
        }
    }
    if (xd->up_available) {
        const BlockModeInfo* const above_mi = &xd->above_mbmi->block_mi;
        if ((is_inter_singleref_mode(above_mi->mode) &&
             (above_mi->ref_frame[0] == rf[0] || above_mi->ref_frame[0] == rf[1])) ||
            (is_inter_compound_mode(above_mi->mode) &&
             (above_mi->ref_frame[0] == rf[0] && above_mi->ref_frame[1] == rf[1]))) {
            return false;
        }
    }

    return skip_comp;
}

// Inject inter-inter compound types (DIST, DIFF, WEDGE) for a bipred AVG candidate
//
// total_cand_count is the index to ctx->fast_cand_array for the next candidate injected (which is the
// same as the number of candidates injected so far).  It is assumed the AVG candidate to base
// the other candidtes on is the previously injected candidate (at index total_cand_count - 1).
static void inj_comp_modes(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* total_cand_count) {
    // index of MD_COMP_AVG candidate (to be used to copy cand info for other modes)
    // assumes the avg cand is the previously injected candidate
    const uint32_t         avg_cand_idx = *total_cand_count - 1;
    ModeDecisionCandidate* avg_cand     = &ctx->fast_cand_array[avg_cand_idx];

    // Get allowable compound types based on settings and block size
    MD_COMP_TYPE tot_comp_types = get_tot_comp_types_bsize(ctx->inter_comp_ctrls.tot_comp_types, ctx->blk_geom->bsize);
    if (tot_comp_types == MD_COMP_DIST) {
        return;
    }

    // Distortion-based ref pruning for compound types
    const uint8_t ref_idx_0  = get_ref_frame_idx(avg_cand->block_mi.ref_frame[0]);
    const uint8_t ref_idx_1  = get_ref_frame_idx(avg_cand->block_mi.ref_frame[1]);
    const uint8_t list_idx_0 = get_list_idx(avg_cand->block_mi.ref_frame[0]);
    const uint8_t list_idx_1 = get_list_idx(avg_cand->block_mi.ref_frame[1]);
    if (!is_valid_bipred_ref(ctx, INTER_COMP_GROUP, list_idx_0, ref_idx_0, list_idx_1, ref_idx_1)) {
        return;
    }

    // Skip compound on neighbour info
    if (skip_compound_on_ref_types(ctx, avg_cand->block_mi.ref_frame)) {
        return;
    }

    // Skip compound on MV length
    if (ctx->inter_comp_ctrls.max_mv_length) {
        const uint16_t max_mv_length = ctx->inter_comp_ctrls.max_mv_length;
        if (abs(avg_cand->block_mi.mv[0].x) > max_mv_length || abs(avg_cand->block_mi.mv[0].y) > max_mv_length ||
            abs(avg_cand->block_mi.mv[1].x) > max_mv_length || abs(avg_cand->block_mi.mv[1].y) > max_mv_length) {
            return;
        }
    }
    // If compound modes are to be tested for this block, generate the buffers that will be used in the DIFF/WEDGE search.
    // Even if DIFF/WEDGE are not used, still call the function because it is needed for pred0_to_pred1_mult to work.
    if (tot_comp_types > MD_COMP_DIST) {
        if (svt_aom_calc_pred_masked_compound(pcs, ctx, avg_cand)) {
            return;
        }
    }

    // The candidate count to be used to track number of inj cands, and the index of fast_cand_array for new candidates
    uint32_t cand_count = *total_cand_count;
    for (MD_COMP_TYPE cur_type = MD_COMP_DIST; cur_type < tot_comp_types; cur_type++) {
        if (ctx->inter_comp_ctrls.no_sym_dist && cur_type == MD_COMP_DIST && ref_idx_0 == 0 && ref_idx_1 == 0) {
            continue;
        }
        ModeDecisionCandidate* cand = &ctx->fast_cand_array[cand_count];
        svt_memcpy(cand, &ctx->fast_cand_array[avg_cand_idx], sizeof(ModeDecisionCandidate));
        cand->skip_mode_allowed = false;
        determine_compound_mode(pcs, ctx, cand, cur_type);
        INC_MD_CAND_CNT(cand_count, pcs->ppcs->max_can_count);
    }
    *total_cand_count = cand_count;
}

static void unipred_3x3_candidates_injection(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                             uint32_t* candidate_total_cnt) {
    uint32_t               cand_total_cnt          = (*candidate_total_cnt);
    const uint8_t          allow_high_precision_mv = pcs->ppcs->frm_hdr.allow_high_precision_mv;
    MeSbResults*           me_results              = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
    const uint8_t          total_me_cnt            = me_results->total_me_candidate_index[ctx->me_block_offset];
    const MeCandidate*     me_block_results        = &me_results->me_candidate_array[ctx->me_cand_offset];
    ModeDecisionCandidate* cand_array              = ctx->fast_cand_array;

    // (8 Best_L0 neighbors)
    for (uint8_t me_candidate_index = 0; me_candidate_index < total_me_cnt; ++me_candidate_index) {
        const MeCandidate* me_block_results_ptr = &me_block_results[me_candidate_index];
        const uint8_t      inter_direction      = me_block_results_ptr->direction;
        const uint8_t      list0_ref_index      = me_block_results_ptr->ref_idx_l0;
        const uint8_t      list1_ref_index      = me_block_results_ptr->ref_idx_l1;
        if (inter_direction == BI_PRED) {
            continue;
        }
        assert(inter_direction == 0 || inter_direction == 1);
        const uint8_t list_idx = inter_direction;
        const uint8_t ref_idx  = list_idx == REF_LIST_0 ? list0_ref_index : list1_ref_index;
        if (!svt_aom_is_valid_unipred_ref(ctx, MIN(TOT_INTER_GROUP - 1, UNI_3x3_GROUP), list_idx, ref_idx)) {
            continue;
        }
        for (int unipred_index = 0; unipred_index < BIPRED_3x3_REFINMENT_POSITIONS; ++unipred_index) {
            /**************
            NEWMV L0
            ************* */
            if (ctx->unipred3x3_injection >= 2) {
                if (allow_refinement_flag[unipred_index] == 0) {
                    continue;
                }
            }
            Mv to_inj_mv = ctx->sb_me_mv[list_idx][ref_idx];
            to_inj_mv.x += (bipred_3x3_x_pos[unipred_index] << !allow_high_precision_mv);
            to_inj_mv.y += (bipred_3x3_y_pos[unipred_index] << !allow_high_precision_mv);
            const uint8_t    to_inject_ref_type = svt_get_ref_frame_type(list_idx, ref_idx);
            MvReferenceFrame rf[2]              = {to_inject_ref_type, NONE_FRAME};
            if ((ctx->injected_mv_count == 0 ||
                 mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, to_inject_ref_type) == false)) {
                uint8_t drl_index       = 0;
                Mv      best_pred_mv[2] = {{{0}}, {{0}}};
                svt_aom_choose_best_av1_mv_pred(
                    ctx, to_inject_ref_type, NEWMV, to_inj_mv, (Mv){{0}}, &drl_index, best_pred_mv);
                if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, 0)) {
                    ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->block_mi.mode               = NEWMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.is_interintra_used = 0;
                    cand->drl_index                   = drl_index;
                    cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                    cand->block_mi.num_proj_ref       = ctx->wm_sample_info[to_inject_ref_type].num;

                    INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                    const bool enable_ii = true;
                    // OBMC and WM perform a refinement search around the ME MV, so they are not injected as unipred3x3 candidates,
                    // since this is effectively a refinement search
                    const bool enable_obmc = false;
                    const bool enable_warp = false;
                    inj_non_simple_modes(pcs, ctx, &cand_total_cnt, enable_ii, enable_warp, enable_obmc);

                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                    ++ctx->injected_mv_count;
                }
            }
        }
    }

    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;

    return;
}

static void bipred_3x3_candidates_injection(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                            uint32_t* candidate_total_cnt) {
    uint32_t               cand_total_cnt          = (*candidate_total_cnt);
    const uint8_t          allow_high_precision_mv = pcs->ppcs->frm_hdr.allow_high_precision_mv;
    const MeSbResults*     me_results              = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
    const uint8_t          total_me_cnt            = me_results->total_me_candidate_index[ctx->me_block_offset];
    const MeCandidate*     me_block_results        = &me_results->me_candidate_array[ctx->me_cand_offset];
    ModeDecisionCandidate* cand_array              = ctx->fast_cand_array;
    Mv                     best_pred_mv[2]         = {{{0}}, {{0}}};

    /**************
    NEW_NEWMV
    ************* */
    for (uint8_t me_candidate_index = 0; me_candidate_index < total_me_cnt; ++me_candidate_index) {
        const MeCandidate* me_block_results_ptr = &me_block_results[me_candidate_index];
        const uint8_t      inter_direction      = me_block_results_ptr->direction;
        const uint8_t      list0_ref_index      = me_block_results_ptr->ref_idx_l0;
        const uint8_t      list1_ref_index      = me_block_results_ptr->ref_idx_l1;
        if (inter_direction < BI_PRED) {
            continue;
        }
        assert(inter_direction == BI_PRED);

        const uint8_t ref0_list = me_block_results_ptr->ref0_list;
        const uint8_t ref1_list = me_block_results_ptr->ref1_list;
        if (!is_valid_bipred_ref(ctx, BI_3x3_GROUP, ref0_list, list0_ref_index, ref1_list, list1_ref_index)) {
            continue;
        }

        int8_t best_list = -1;
        int    diff      = ((int)ctx->post_subpel_me_mv_cost[ref0_list][list0_ref_index] -
                    (int)ctx->post_subpel_me_mv_cost[ref1_list][list1_ref_index]) *
            100;

        if (ctx->bipred3x3_ctrls.use_l0_l1_dev != (uint8_t)~0) {
            if (abs(diff) >
                (ctx->bipred3x3_ctrls.use_l0_l1_dev * (int)ctx->post_subpel_me_mv_cost[ref0_list][list0_ref_index])) {
                return;
            }
        }

        // Best list in terms of distortion reduction
        if (ctx->bipred3x3_ctrls.use_best_list) {
            best_list = ref0_list;
            if (diff > 0) {
                best_list = ref1_list;
            }
        }

        MvReferenceFrame rf[2]              = {svt_get_ref_frame_type(ref0_list, list0_ref_index),
                                               svt_get_ref_frame_type(ref1_list, list1_ref_index)};
        const uint8_t    to_inject_ref_type = av1_ref_frame_type(rf);
        if (best_list == -1 || best_list == ref0_list) {
            // (Best_L0, 8 Best_L1 neighbors)
            for (uint32_t bipred_index = 0; bipred_index < BIPRED_3x3_REFINMENT_POSITIONS; ++bipred_index) {
                if (!ctx->bipred3x3_ctrls.search_diag) {
                    if (allow_refinement_flag[bipred_index] == 0) {
                        continue;
                    }
                }
                Mv to_inj_mv0 = ctx->sb_me_mv[ref0_list][list0_ref_index];
                Mv to_inj_mv1 = ctx->sb_me_mv[ref1_list][list1_ref_index];
                to_inj_mv1.x += (bipred_3x3_x_pos[bipred_index] << !allow_high_precision_mv);
                to_inj_mv1.y += (bipred_3x3_y_pos[bipred_index] << !allow_high_precision_mv);
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, to_inject_ref_type) == false)) {
                    uint8_t drl_index = 0;
                    svt_aom_choose_best_av1_mv_pred(
                        ctx, to_inject_ref_type, NEW_NEWMV, to_inj_mv0, to_inj_mv1, &drl_index, best_pred_mv);
                    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, 1)) {
                        ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                        cand->block_mi.use_intrabc        = 0;
                        cand->skip_mode_allowed           = false;
                        cand->drl_index                   = drl_index;
                        cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                        cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                        cand->block_mi.mode               = NEW_NEWMV;
                        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                        cand->block_mi.is_interintra_used = 0;
                        cand->block_mi.ref_frame[0]       = rf[0];
                        cand->block_mi.ref_frame[1]       = rf[1];
                        cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                        cand->pred_mv[1].as_int           = best_pred_mv[1].as_int;
                        determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                        if (ctx->inter_comp_ctrls.do_3x3_bi) {
                            ctx->cmp_store.pred0_cnt = 0;
                            ctx->cmp_store.pred1_cnt = 0;
                            inj_comp_modes(pcs, ctx, &cand_total_cnt);
                        }
                        ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                        ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                        ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                        ++ctx->injected_mv_count;
                    }
                }
            }
        }
        if (best_list == -1 || best_list == ref1_list) {
            // (8 Best_L0 neighbors, Best_L1) :
            for (uint32_t bipred_index = 0; bipred_index < BIPRED_3x3_REFINMENT_POSITIONS; ++bipred_index) {
                if (!ctx->bipred3x3_ctrls.search_diag) {
                    if (allow_refinement_flag[bipred_index] == 0) {
                        continue;
                    }
                }
                Mv to_inj_mv0 = ctx->sb_me_mv[ref0_list][list0_ref_index];
                to_inj_mv0.x += (bipred_3x3_x_pos[bipred_index] << !allow_high_precision_mv);
                to_inj_mv0.y += (bipred_3x3_y_pos[bipred_index] << !allow_high_precision_mv);
                Mv to_inj_mv1 = ctx->sb_me_mv[ref1_list][list1_ref_index];
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, to_inject_ref_type) == false)) {
                    uint8_t drl_index = 0;
                    svt_aom_choose_best_av1_mv_pred(
                        ctx, to_inject_ref_type, NEW_NEWMV, to_inj_mv0, to_inj_mv1, &drl_index, best_pred_mv);
                    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, 1)) {
                        ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                        cand->block_mi.use_intrabc        = 0;
                        cand->skip_mode_allowed           = false;
                        cand->drl_index                   = drl_index;
                        cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                        cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                        cand->block_mi.mode               = NEW_NEWMV;
                        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                        cand->block_mi.is_interintra_used = 0;
                        cand->block_mi.ref_frame[0]       = rf[0];
                        cand->block_mi.ref_frame[1]       = rf[1];
                        cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                        cand->pred_mv[1].as_int           = best_pred_mv[1].as_int;
                        determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                        if (ctx->inter_comp_ctrls.do_3x3_bi) {
                            ctx->cmp_store.pred0_cnt = 0;
                            ctx->cmp_store.pred1_cnt = 0;
                            inj_comp_modes(pcs, ctx, &cand_total_cnt);
                        }
                        ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                        ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                        ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                        ++ctx->injected_mv_count;
                    }
                }
            }
        }
    }

    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;

    return;
}

/*********************************************************************
**********************************************************************
        Upto 12 inter Candidated injected
        Min 6 inter Candidated injected
UniPred L0 : NEARST         + upto 3x NEAR
UniPred L1 : NEARST         + upto 3x NEAR
BIPred     : NEARST_NEARST  + upto 3x NEAR_NEAR
**********************************************************************
**********************************************************************/
static void inject_mvp_candidates_ii_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* candTotCnt,
                                               const bool allow_bipred) {
    FrameHeader*           frm_hdr    = &pcs->ppcs->frm_hdr;
    uint32_t               cand_idx   = *candTotCnt;
    ModeDecisionCandidate* cand_array = ctx->fast_cand_array;
    MacroBlockD*           xd         = ctx->blk_ptr->av1xd;

    //all of ref pairs: (1)single-ref List0  (2)single-ref List1  (3)compound Bi-Dir List0-List1
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        //single ref/list
        if (rf[1] == NONE_FRAME) {
            MvReferenceFrame frame_type = rf[0];
            uint8_t          list_idx   = get_list_idx(rf[0]);
            if (ctx->cand_reduction_ctrls.lpd1_mvp_best_me_list) {
                const MeSbResults* me_results           = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
                const uint8_t      total_me_cnt         = me_results->total_me_candidate_index[ctx->me_block_offset];
                const MeCandidate* me_block_results     = &me_results->me_candidate_array[ctx->me_cand_offset];
                const MeCandidate* me_block_results_ptr = &me_block_results[0];
                const uint8_t      inter_direction      = me_block_results_ptr->direction;
                if (total_me_cnt && list_idx != inter_direction) {
                    continue;
                }
            }
            //NEAREST
            // Don't check if MV is already injected b/c NEAREST is the first INTER MV injected
            Mv to_inj_mv = {.as_int = ctx->ref_mv_stack[frame_type][0].this_mv.as_int};

            ModeDecisionCandidate* cand       = &cand_array[cand_idx];
            cand->block_mi.mode               = NEARESTMV;
            cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
            cand->skip_mode_allowed           = false;
            cand->drl_index                   = 0;
            cand->block_mi.ref_frame[0]       = rf[0];
            cand->block_mi.ref_frame[1]       = rf[1];
            cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
            cand->block_mi.num_proj_ref       = ctx->wm_sample_info[frame_type].num;
            cand->block_mi.use_intrabc        = 0;
            cand->block_mi.is_interintra_used = 0;
            INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

            ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
            ctx->injected_ref_types[ctx->injected_mv_count]     = frame_type;
            ++ctx->injected_mv_count;
            //NEAR
            const uint8_t max_drl_index     = svt_aom_get_max_drl_index(xd->ref_mv_count[frame_type], NEARMV);
            uint8_t       cap_max_drl_index = 0;
            if (ctx->cand_reduction_ctrls.near_count_ctrls.enabled) {
                cap_max_drl_index = MIN(ctx->cand_reduction_ctrls.near_count_ctrls.near_count, max_drl_index);
            }
            for (uint8_t drli = 0; drli < cap_max_drl_index; drli++) {
                to_inj_mv.as_int = ctx->ref_mv_stack[frame_type][1 + drli].this_mv.as_int;

                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, frame_type) == false)) {
                    cand                              = &cand_array[cand_idx];
                    cand->block_mi.mode               = NEARMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->skip_mode_allowed           = false;
                    cand->drl_index                   = drli;
                    cand->block_mi.use_intrabc        = 0;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                    cand->block_mi.num_proj_ref       = ctx->wm_sample_info[frame_type].num;
                    INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = frame_type;
                    ++ctx->injected_mv_count;
                }
            }
        } else if (allow_bipred) {
            //NEAREST_NEAREST
            // Don't check if MV is already injected b/c NEAREST_NEAREST is the first bipred INTER candidate injected
            Mv         to_inj_mv0   = {.as_int = ctx->ref_mv_stack[ref_pair][0].this_mv.as_int};
            Mv         to_inj_mv1   = {.as_int = ctx->ref_mv_stack[ref_pair][0].comp_mv.as_int};
            const bool is_skip_mode = !svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) &&
                frm_hdr->skip_mode_params.skip_mode_flag && (rf[0] == frm_hdr->skip_mode_params.ref_frame_idx_0) &&
                (rf[1] == frm_hdr->skip_mode_params.ref_frame_idx_1);
            ModeDecisionCandidate* cand         = &cand_array[cand_idx];
            cand->block_mi.mode                 = NEAREST_NEARESTMV;
            cand->block_mi.motion_mode          = SIMPLE_TRANSLATION;
            cand->skip_mode_allowed             = is_skip_mode;
            cand->block_mi.mv[0].as_int         = to_inj_mv0.as_int;
            cand->block_mi.mv[1].as_int         = to_inj_mv1.as_int;
            cand->drl_index                     = 0;
            cand->block_mi.use_intrabc          = 0;
            cand->block_mi.is_interintra_used   = 0;
            cand->block_mi.ref_frame[0]         = rf[0];
            cand->block_mi.ref_frame[1]         = rf[1];
            cand->block_mi.comp_group_idx       = 0;
            cand->block_mi.compound_idx         = 1;
            cand->block_mi.interinter_comp.type = COMPOUND_AVERAGE;

            INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

            ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
            ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
            ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
            ++ctx->injected_mv_count;

            //NEAR_NEAR
            const uint8_t max_drl_index     = svt_aom_get_max_drl_index(xd->ref_mv_count[ref_pair], NEAR_NEARMV);
            uint8_t       cap_max_drl_index = 0;
            if (ctx->cand_reduction_ctrls.near_count_ctrls.enabled) {
                cap_max_drl_index = MIN(ctx->cand_reduction_ctrls.near_count_ctrls.near_near_count, max_drl_index);
            }
            for (uint8_t drli = 0; drli < cap_max_drl_index; drli++) {
                to_inj_mv0.as_int = ctx->ref_mv_stack[ref_pair][1 + drli].this_mv.as_int;
                to_inj_mv1.as_int = ctx->ref_mv_stack[ref_pair][1 + drli].comp_mv.as_int;
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false)) {
                    cand                                = &cand_array[cand_idx];
                    cand->block_mi.mode                 = NEAR_NEARMV;
                    cand->block_mi.motion_mode          = SIMPLE_TRANSLATION;
                    cand->skip_mode_allowed             = false;
                    cand->block_mi.use_intrabc          = 0;
                    cand->block_mi.is_interintra_used   = 0;
                    cand->block_mi.mv[0].as_int         = to_inj_mv0.as_int;
                    cand->block_mi.mv[1].as_int         = to_inj_mv1.as_int;
                    cand->drl_index                     = drli;
                    cand->block_mi.ref_frame[0]         = rf[0];
                    cand->block_mi.ref_frame[1]         = rf[1];
                    cand->block_mi.comp_group_idx       = 0;
                    cand->block_mi.compound_idx         = 1;
                    cand->block_mi.interinter_comp.type = COMPOUND_AVERAGE;

                    INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                    ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                    ++ctx->injected_mv_count;
                }
            }
        }
    }
    //update tot Candidate count
    *candTotCnt = cand_idx;
}

/*********************************************************************
**********************************************************************
        Upto 12 inter Candidated injected
        Min 6 inter Candidated injected
UniPred L0 : NEARST         + upto 3x NEAR
UniPred L1 : NEARST         + upto 3x NEAR
BIPred     : NEARST_NEARST  + upto 3x NEAR_NEAR
**********************************************************************
**********************************************************************/
static void inject_mvp_candidates_ii(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* cand_total_cnt,
                                     const bool allow_bipred) {
    BlkStruct*             blk_ptr    = ctx->blk_ptr;
    FrameHeader*           frm_hdr    = &pcs->ppcs->frm_hdr;
    uint32_t               cand_idx   = *cand_total_cnt;
    ModeDecisionCandidate* cand_array = ctx->fast_cand_array;
    MacroBlockD*           xd         = blk_ptr->av1xd;
    Mv                     nearestmv[2], nearmv[2], ref_mv[2];

    //all of ref pairs: (1)single-ref List0  (2)single-ref List1  (3)compound Bi-Dir List0-List1  (4)compound Uni-Dir List0-List0  (5)compound Uni-Dir List1-List1
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);
        //single ref/list
        if (rf[1] == NONE_FRAME) {
            MvReferenceFrame frame_type = rf[0];
            uint8_t          list_idx   = get_list_idx(rf[0]);
            uint8_t          ref_idx    = get_ref_frame_idx(rf[0]);
            // Always consider the 2 closet ref frames (i.e. ref_idx=0) @ MVP cand generation
            if (!svt_aom_is_valid_unipred_ref(ctx, MIN(TOT_INTER_GROUP - 1, NRST_NEAR_GROUP), list_idx, ref_idx)) {
                continue;
            }
            //NEAREST
            Mv to_inj_mv = {.as_int = ctx->ref_mv_stack[frame_type][0].this_mv.as_int};
            if ((ctx->injected_mv_count == 0 ||
                 mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, frame_type) == false)) {
                assert(list_idx == 0 || list_idx == 1);
                ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                cand->block_mi.mode               = NEARESTMV;
                cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                cand->block_mi.use_intrabc        = 0;
                cand->skip_mode_allowed           = false;
                cand->drl_index                   = 0;
                cand->block_mi.ref_frame[0]       = rf[0];
                cand->block_mi.ref_frame[1]       = rf[1];
                cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                cand->block_mi.is_interintra_used = 0;
                cand->block_mi.num_proj_ref       = ctx->wm_sample_info[frame_type].num;
                INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                const bool enable_ii   = true;
                const bool enable_obmc = true;
                const bool enable_warp = ctx->wm_ctrls.use_wm_for_mvp ? true : false;
                inj_non_simple_modes(pcs, ctx, &cand_idx, enable_ii, enable_warp, enable_obmc);
                ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                ctx->injected_ref_types[ctx->injected_mv_count]     = frame_type;
                ++ctx->injected_mv_count;
            }

            //NEAR
            const uint8_t max_drl_index     = svt_aom_get_max_drl_index(xd->ref_mv_count[frame_type], NEARMV);
            uint8_t       cap_max_drl_index = 0;
            if (ctx->cand_reduction_ctrls.near_count_ctrls.enabled) {
                cap_max_drl_index = MIN(ctx->cand_reduction_ctrls.near_count_ctrls.near_count, max_drl_index);
            }
            for (uint8_t drli = 0; drli < cap_max_drl_index; drli++) {
                svt_aom_get_av1_mv_pred_drl(ctx, blk_ptr, frame_type, 0, NEARMV, drli, nearestmv, nearmv, ref_mv);

                to_inj_mv.as_int = nearmv[0].as_int;
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, frame_type) == false)) {
                    assert(list_idx == 0 || list_idx == 1);
                    ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                    cand->block_mi.mode               = NEARMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->drl_index                   = drli;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.num_proj_ref       = ctx->wm_sample_info[frame_type].num;
                    INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                    const bool enable_ii   = true;
                    const bool enable_obmc = true;
                    const bool enable_warp = ctx->wm_ctrls.use_wm_for_mvp ? true : false;
                    inj_non_simple_modes(pcs, ctx, &cand_idx, enable_ii, enable_warp, enable_obmc);
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = frame_type;
                    ++ctx->injected_mv_count;
                }
            }
        } else if (allow_bipred) {
            const uint8_t ref_idx_0 = get_ref_frame_idx(rf[0]);
            const uint8_t ref_idx_1 = get_ref_frame_idx(rf[1]);

            const uint8_t list_idx_0 = get_list_idx(rf[0]);
            const uint8_t list_idx_1 = get_list_idx(rf[1]);

            ctx->cmp_store.pred0_cnt = 0;
            ctx->cmp_store.pred1_cnt = 0;

            // Always consider the 2 closet ref frames (i.e. ref_idx=0) @ MVP cand generation
            if (!is_valid_bipred_ref(ctx, NRST_NEAR_GROUP, list_idx_0, ref_idx_0, list_idx_1, ref_idx_1)) {
                continue;
            }

            //NEAREST_NEAREST
            Mv to_inj_mv0 = {.as_int = ctx->ref_mv_stack[ref_pair][0].this_mv.as_int};
            Mv to_inj_mv1 = {.as_int = ctx->ref_mv_stack[ref_pair][0].comp_mv.as_int};
            if ((ctx->injected_mv_count == 0 ||
                 mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false)) {
                const bool is_skip_mode = !svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) &&
                    frm_hdr->skip_mode_params.skip_mode_flag && (rf[0] == frm_hdr->skip_mode_params.ref_frame_idx_0) &&
                    (rf[1] == frm_hdr->skip_mode_params.ref_frame_idx_1);
                ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                cand->block_mi.mode               = NEAREST_NEARESTMV;
                cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                cand->block_mi.is_interintra_used = 0;
                cand->block_mi.use_intrabc        = 0;
                cand->skip_mode_allowed           = /*cur_type == MD_COMP_AVG &&*/ is_skip_mode ? true : false;
                cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                cand->drl_index                   = 0;
                cand->block_mi.ref_frame[0]       = rf[0];
                cand->block_mi.ref_frame[1]       = rf[1];
                determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                if (ctx->inter_comp_ctrls.do_nearest_nearest) {
                    // Don't reset ctx->cmp_store.pred0_cnt for MVP
                    inj_comp_modes(pcs, ctx, &cand_idx);
                }
                ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                ++ctx->injected_mv_count;
            }

            //NEAR_NEAR
            const uint8_t max_drl_index     = svt_aom_get_max_drl_index(xd->ref_mv_count[ref_pair], NEAR_NEARMV);
            uint8_t       cap_max_drl_index = 0;
            if (ctx->cand_reduction_ctrls.near_count_ctrls.enabled) {
                cap_max_drl_index = MIN(ctx->cand_reduction_ctrls.near_count_ctrls.near_near_count, max_drl_index);
            }
            for (uint8_t drli = 0; drli < cap_max_drl_index; drli++) {
                svt_aom_get_av1_mv_pred_drl(ctx, blk_ptr, ref_pair, 1, NEAR_NEARMV, drli, nearestmv, nearmv, ref_mv);

                to_inj_mv0.as_int = nearmv[0].as_int;
                to_inj_mv1.as_int = nearmv[1].as_int;
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false)) {
                    ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                    cand->block_mi.mode               = NEAR_NEARMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                    cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                    cand->drl_index                   = drli;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                    INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                    if (ctx->inter_comp_ctrls.do_near_near) {
                        // Don't reset ctx->cmp_store.pred0_cnt for MVP
                        inj_comp_modes(pcs, ctx, &cand_idx);
                    }
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                    ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                    ++ctx->injected_mv_count;
                }
            }
        }
    }
    //update tot Candidate count
    *cand_total_cnt = cand_idx;
}

static void inject_new_nearest_new_comb_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                   uint32_t* cand_tot_cnt) {
    uint32_t               cand_idx   = *cand_tot_cnt;
    ModeDecisionCandidate* cand_array = ctx->fast_cand_array;
    MacroBlockD*           xd         = ctx->blk_ptr->av1xd;
    Mv                     nearestmv[2], nearmv[2], ref_mv[2];

    //all of ref pairs: (1)single-ref List0  (2)single-ref List1  (3)compound Bi-Dir List0-List1  (4)compound Uni-Dir List0-List0  (5)compound Uni-Dir List1-List1
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);
        if (rf[1] != NONE_FRAME) {
            const uint8_t ref_idx_0  = get_ref_frame_idx(rf[0]);
            const uint8_t ref_idx_1  = get_ref_frame_idx(rf[1]);
            const uint8_t list_idx_0 = get_list_idx(rf[0]);
            const uint8_t list_idx_1 = get_list_idx(rf[1]);
            if (list_idx_0 != INVALID_REF) {
                if (!svt_aom_is_valid_unipred_ref(
                        ctx, MIN(TOT_INTER_GROUP - 1, NRST_NEW_NEAR_GROUP), list_idx_0, ref_idx_0)) {
                    continue;
                }
            }
            if (list_idx_1 != INVALID_REF) {
                if (!svt_aom_is_valid_unipred_ref(
                        ctx, MIN(TOT_INTER_GROUP - 1, NRST_NEW_NEAR_GROUP), list_idx_1, ref_idx_1)) {
                    continue;
                }
            }

            {
                //NEAREST_NEWMV
                const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
                Mv                 to_inj_mv0 = {.as_int = ctx->ref_mv_stack[ref_pair][0].this_mv.as_int};
                Mv                 to_inj_mv1 = ctx->sb_me_mv[list_idx_1][ref_idx_1];
                uint8_t            inj_mv     = (ctx->injected_mv_count == 0 ||
                                  mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false);
                inj_mv                        = inj_mv &&
                    svt_aom_is_me_data_present(
                             ctx->me_block_offset, ctx->me_cand_offset, me_results, get_list_idx(rf[1]), ref_idx_1);
                if (inj_mv) {
                    svt_aom_get_av1_mv_pred_drl(ctx,
                                                ctx->blk_ptr,
                                                ref_pair,
                                                1, // is_compound
                                                NEAREST_NEWMV,
                                                0, //not needed drli,
                                                nearestmv,
                                                nearmv,
                                                ref_mv);

                    ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                    cand->block_mi.mode               = NEAREST_NEWMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                    cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                    cand->drl_index                   = 0;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    cand->pred_mv[1].as_int           = ref_mv[1].as_int;
                    determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                    INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                    if (ctx->inter_comp_ctrls.do_nearest_near_new) {
                        ctx->cmp_store.pred0_cnt = 0;
                        ctx->cmp_store.pred1_cnt = 0;
                        inj_comp_modes(pcs, ctx, &cand_idx);
                    }
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                    ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                    ++ctx->injected_mv_count;
                }
            }

            {
                //NEW_NEARESTMV
                const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
                Mv                 to_inj_mv0 = ctx->sb_me_mv[list_idx_0][ref_idx_0];
                Mv                 to_inj_mv1 = {.as_int = ctx->ref_mv_stack[ref_pair][0].comp_mv.as_int};
                uint8_t            inj_mv     = (ctx->injected_mv_count == 0 ||
                                  mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false);
                inj_mv                        = inj_mv &&
                    svt_aom_is_me_data_present(ctx->me_block_offset, ctx->me_cand_offset, me_results, 0, ref_idx_0);
                if (inj_mv) {
                    svt_aom_get_av1_mv_pred_drl(ctx,
                                                ctx->blk_ptr,
                                                ref_pair,
                                                1, // is_compound
                                                NEW_NEARESTMV,
                                                0, //not needed drli,
                                                nearestmv,
                                                nearmv,
                                                ref_mv);

                    ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                    cand->block_mi.mode               = NEW_NEARESTMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                    cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                    cand->drl_index                   = 0;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    cand->pred_mv[0].as_int           = ref_mv[0].as_int;
                    determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                    INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                    if (ctx->inter_comp_ctrls.do_nearest_near_new) {
                        ctx->cmp_store.pred0_cnt = 0;
                        ctx->cmp_store.pred1_cnt = 0;
                        inj_comp_modes(pcs, ctx, &cand_idx);
                    }
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                    ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                    ++ctx->injected_mv_count;
                }
            }
            // For level 2, only inject NEAREST_NEW/NEW_NEAREST candidates
            if (ctx->new_nearest_near_comb_injection >= 2) {
                continue;
            }

            //NEW_NEARMV
            {
                const uint8_t max_drl_index = svt_aom_get_max_drl_index(xd->ref_mv_count[ref_pair], NEW_NEARMV);

                for (uint8_t drli = 0; drli < max_drl_index; drli++) {
                    svt_aom_get_av1_mv_pred_drl(
                        ctx, ctx->blk_ptr, ref_pair, 1, NEW_NEARMV, drli, nearestmv, nearmv, ref_mv);

                    //NEW_NEARMV
                    const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
                    Mv                 to_inj_mv0 = ctx->sb_me_mv[list_idx_0][ref_idx_0];
                    Mv                 to_inj_mv1 = {.as_int = nearmv[1].as_int};
                    uint8_t            inj_mv     = (ctx->injected_mv_count == 0 ||
                                      mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false);
                    inj_mv                        = inj_mv &&
                        svt_aom_is_me_data_present(ctx->me_block_offset, ctx->me_cand_offset, me_results, 0, ref_idx_0);
                    if (inj_mv) {
                        ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                        cand->block_mi.mode               = NEW_NEARMV;
                        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                        cand->block_mi.is_interintra_used = 0;
                        cand->block_mi.use_intrabc        = 0;
                        cand->skip_mode_allowed           = false;
                        cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                        cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                        cand->drl_index                   = drli;
                        cand->block_mi.ref_frame[0]       = rf[0];
                        cand->block_mi.ref_frame[1]       = rf[1];
                        cand->pred_mv[0].as_int           = ref_mv[0].as_int;
                        determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                        INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                        if (ctx->inter_comp_ctrls.do_nearest_near_new) {
                            ctx->cmp_store.pred0_cnt = 0;
                            ctx->cmp_store.pred1_cnt = 0;
                            inj_comp_modes(pcs, ctx, &cand_idx);
                        }
                        ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                        ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                        ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                        ++ctx->injected_mv_count;
                    }
                }
            }
            //NEAR_NEWMV
            {
                uint8_t max_drl_index = svt_aom_get_max_drl_index(xd->ref_mv_count[ref_pair], NEAR_NEWMV);

                for (uint8_t drli = 0; drli < max_drl_index; drli++) {
                    svt_aom_get_av1_mv_pred_drl(
                        ctx, ctx->blk_ptr, ref_pair, 1, NEAR_NEWMV, drli, nearestmv, nearmv, ref_mv);

                    //NEAR_NEWMV
                    const MeSbResults* me_results = pcs->ppcs->pa_me_data->me_results[ctx->me_sb_addr];
                    Mv                 to_inj_mv0 = {.as_int = nearmv[0].as_int};
                    Mv                 to_inj_mv1 = ctx->sb_me_mv[list_idx_1][ref_idx_1];
                    uint8_t            inj_mv     = (ctx->injected_mv_count == 0 ||
                                      mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, ref_pair) == false);
                    inj_mv                        = inj_mv &&
                        svt_aom_is_me_data_present(
                                 ctx->me_block_offset, ctx->me_cand_offset, me_results, list_idx_1, ref_idx_1);

                    if (inj_mv) {
                        ModeDecisionCandidate* cand       = &cand_array[cand_idx];
                        cand->block_mi.mode               = NEAR_NEWMV;
                        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                        cand->block_mi.is_interintra_used = 0;
                        cand->block_mi.use_intrabc        = 0;
                        cand->skip_mode_allowed           = false;
                        cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                        cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                        cand->drl_index                   = drli;
                        cand->block_mi.ref_frame[0]       = rf[0];
                        cand->block_mi.ref_frame[1]       = rf[1];
                        cand->pred_mv[1].as_int           = ref_mv[1].as_int;
                        determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                        INC_MD_CAND_CNT(cand_idx, pcs->ppcs->max_can_count);

                        if (ctx->inter_comp_ctrls.do_nearest_near_new) {
                            ctx->cmp_store.pred0_cnt = 0;
                            ctx->cmp_store.pred1_cnt = 0;
                            inj_comp_modes(pcs, ctx, &cand_idx);
                        }
                        ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                        ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                        ctx->injected_ref_types[ctx->injected_mv_count]     = ref_pair;
                        ++ctx->injected_mv_count;
                    }
                }
            }
        }
    }
    //update tot Candidate count
    *cand_tot_cnt = cand_idx;
}

// Refine the WM MV (8 bit search).  Return true if search found a valid MV; false otherwise
uint8_t svt_aom_wm_motion_refinement(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidate* cand,
                                     const bool shut_approx) {
    PictureParentControlSet* ppcs         = pcs->ppcs;
    const Mv                 neighbors[9] = {
        {{0, 0}}, {{-1, 0}}, {{0, 1}}, {{1, 0}}, {{0, -1}}, {{1, -1}}, {{1, 1}}, {{-1, 1}}, {{-1, -1}}};

    // Set info used to get MV cost
    int*        mvjcost       = ctx->md_rate_est_ctx->nmv_vec_cost;
    const int** mvcost        = ctx->md_rate_est_ctx->nmvcoststack;
    uint32_t    full_lambda   = ctx->full_lambda_md[EB_8_BIT_MD]; // 8bit only
    int         error_per_bit = full_lambda >> RD_EPB_SHIFT;
    error_per_bit += (error_per_bit == 0);
    uint32_t             blk_origin_index   = 0;
    EbPictureBufferDesc* input_pic          = ppcs->enhanced_pic; // 10BIT not supported
    uint32_t             input_origin_index = (ctx->blk_org_y + input_pic->org_y) * input_pic->stride_y +
        (ctx->blk_org_x + input_pic->org_x);
    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[ctx->blk_geom->bsize];
    unsigned int            sse;
    uint8_t*                src_y = input_pic->buffer_y + input_origin_index;

    int mv_prec_shift = ppcs->frm_hdr.allow_high_precision_mv ? 0 : 1;
    int best_cost     = INT_MAX;
    // local WM always uses one ref - MV for ref0 stored in idx0
    assert(cand->block_mi.ref_frame[1] == NONE_FRAME);
    Mv       search_centre_mv = {.as_int = cand->block_mi.mv[0].as_int};
    Mv       best_mv          = {.as_int = cand->block_mi.mv[0].as_int};
    Mv       prev_mv          = {.as_int = cand->block_mi.mv[0].as_int};
    const Mv ref_mv           = {.as_int = cand->pred_mv[0].as_int};

    int      max_iterations  = ctx->wm_ctrls.refinement_iterations;
    int      tot_checked_pos = 0;
    uint32_t mv_record[256];
    for (int iter = 0; iter < max_iterations; iter++) {
        // search the (0,0) offset position only for the first search iteration
        for (int i = (iter ? 1 : 0); i < (ctx->wm_ctrls.refine_diag ? 9 : 5); i++) {
            const Mv test_mv = (Mv){{search_centre_mv.x + (neighbors[i].x << mv_prec_shift),
                                     search_centre_mv.y + (neighbors[i].y << mv_prec_shift)}};

            // Don't re-test previously tested positions
            if (iter) {
                if (prev_mv.as_int == test_mv.as_int) {
                    continue;
                }
                int match_found = 0;
                for (int j = 0; j < tot_checked_pos; j++) {
                    if (test_mv.as_int == mv_record[j]) {
                        match_found = 1;
                    }
                }
                if (match_found) {
                    continue;
                }
            }
            mv_record[tot_checked_pos++] = test_mv.as_int;
            uint8_t local_warp_valid     = svt_aom_warped_motion_parameters(ctx,
                                                                        test_mv,
                                                                        ctx->blk_geom,
                                                                        cand->block_mi.ref_frame[0],
                                                                        &cand->wm_params_l0,
                                                                        &cand->block_mi.num_proj_ref,
                                                                        ctx->wm_ctrls.lower_band_th,
                                                                        ctx->wm_ctrls.upper_band_th,
                                                                        shut_approx);
            if (!local_warp_valid) {
                continue;
            }
            assert(cand->block_mi.ref_frame[1] == NONE_FRAME);
            EbPictureBufferDesc* ref_pic_0 = svt_aom_get_ref_pic_buffer(pcs, cand->block_mi.ref_frame[0]);
            EbPictureBufferDesc* ref_pic_1 = NULL; // will stay NULL b/c this is unipred candidate

            // update MV to be testing MV before calling prediction function
            cand->block_mi.mv[0].as_int = test_mv.as_int;
            svt_aom_inter_prediction(pcs->scs,
                                     pcs,
                                     &cand->block_mi,
                                     &cand->wm_params_l0,
                                     &cand->wm_params_l1,
                                     ctx->blk_ptr,
                                     ctx->blk_geom->bsize,
                                     ctx->shape,
                                     // If using 8bit MD for HBD content, can't use pre-computed OBMC/II to
                                     // generate conformant recon
                                     true, //use_precomputed_obmc - not used here
                                     true, //use_precomputed_ii - not used here
                                     ctx,
                                     ctx->recon_neigh_y,
                                     ctx->recon_neigh_cb,
                                     ctx->recon_neigh_cr,
                                     ref_pic_0,
                                     ref_pic_1, // this is NULL
                                     ctx->blk_org_x,
                                     ctx->blk_org_y,
                                     ctx->scratch_prediction_ptr,
                                     0,
                                     0,
                                     PICTURE_BUFFER_DESC_LUMA_MASK,
                                     EB_EIGHT_BIT,
                                     0); // is_16bit_pipeline

            int var = fn_ptr->vf(ctx->scratch_prediction_ptr->buffer_y + blk_origin_index,
                                 ctx->scratch_prediction_ptr->stride_y,
                                 src_y,
                                 input_pic->stride_y,
                                 &sse);
            if (ctx->approx_inter_rate) {
                var += svt_aom_mv_err_cost_light(&test_mv, &ref_mv);
            } else {
                var += svt_aom_mv_err_cost(&test_mv, &ref_mv, mvjcost, mvcost, error_per_bit);
            }

            if (var < best_cost) {
                best_mv.as_int = test_mv.as_int;
                best_cost      = var;
            }
        }
        prev_mv.as_int          = search_centre_mv.as_int;
        search_centre_mv.as_int = best_mv.as_int;
        if (prev_mv.as_int == best_mv.as_int) {
            break;
        }
    }
    cand->block_mi.mv[0].as_int = best_mv.as_int;

    // Derive pred MV for best WM position
    Mv best_pred_mv[2] = {{{0}}, {{0}}};
    svt_aom_choose_best_av1_mv_pred(ctx,
                                    cand->block_mi.ref_frame[0], // WM only allowed for unipred cands
                                    cand->block_mi.mode,
                                    cand->block_mi.mv[0],
                                    (Mv){{0}},
                                    &cand->drl_index,
                                    best_pred_mv);
    cand->pred_mv[0].as_int = best_pred_mv[0].as_int;

    // Check that final chosen MV is valid
    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, best_mv, best_mv, 0)) {
        return 1;
    }

    return 0;
}

static INLINE void setup_pred_plane(Buf2D* dst, BlockSize bsize, uint8_t* src, int width, int height, int stride,
                                    int mi_row, int mi_col, int subsampling_x, int subsampling_y) {
    // Offset the buffer pointer
    if (subsampling_y && (mi_row & 0x01) && (mi_size_high[bsize] == 1)) {
        mi_row -= 1;
    }
    if (subsampling_x && (mi_col & 0x01) && (mi_size_wide[bsize] == 1)) {
        mi_col -= 1;
    }

    const int x = (MI_SIZE * mi_col) >> subsampling_x;
    const int y = (MI_SIZE * mi_row) >> subsampling_y;
    dst->buf    = src + (y * stride + x); // scaled_buffer_offset(x, y, stride, scale);
    dst->buf0   = src;
    dst->width  = width;
    dst->height = height;
    dst->stride = stride;
}

void svt_av1_setup_pred_block(BlockSize bsize, Buf2D dst[MAX_MB_PLANE], const Yv12BufferConfig* src, int mi_row,
                              int mi_col) {
    dst[0].buf    = src->y_buffer;
    dst[0].stride = src->y_stride;
    dst[1].buf    = src->u_buffer;
    dst[2].buf    = src->v_buffer;
    dst[1].stride = dst[2].stride = src->uv_stride;

    setup_pred_plane(
        dst, bsize, dst[0].buf, src->y_crop_width, src->y_crop_height, dst[0].stride, mi_row, mi_col, 0, 0);
}

static int sad_per_bit_lut_8[QINDEX_RANGE];
static int sad_per_bit_lut_10[QINDEX_RANGE];

// Get the sad per bit for the relevant qindex and bit depth
int svt_aom_get_sad_per_bit(int qidx, EbBitDepth is_hbd) {
    return is_hbd ? sad_per_bit_lut_10[qidx] : sad_per_bit_lut_8[qidx];
}

static void init_me_luts_bd(int* bit16lut, int range, EbBitDepth bit_depth) {
    int i;
    // Initialize the sad lut tables using a formulaic calculation for now.
    // This is to make it easier to resolve the impact of experimental changes
    // to the quantizer tables.
    for (i = 0; i < range; i++) {
        const double q = svt_av1_convert_qindex_to_q(i, bit_depth);
        bit16lut[i]    = (int)(0.0418 * q + 2.4107);
    }
}

void svt_av1_init_me_luts(void) {
    init_me_luts_bd(sad_per_bit_lut_8, QINDEX_RANGE, EB_EIGHT_BIT);
    init_me_luts_bd(sad_per_bit_lut_10, QINDEX_RANGE, EB_TEN_BIT);
}

#if CONFIG_ENABLE_OBMC
static void single_motion_search(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidate* cand,
                                 Mv best_pred_mv, IntraBcContext* x, BlockSize bsize, Mv* ref_mv, int* rate_mv,
                                 int refine_level) {
    bool do_full_refine = 0;
    bool do_frac_refine = 0;
    switch (refine_level) {
    case 0:
    case 1:
    case 3:
        do_full_refine = 1;
        do_frac_refine = 1;
        break;
    case 2:
    case 4:
        do_full_refine = 0;
        do_frac_refine = 1;
        break;
    default:
        break;
    }
    const Av1Common* const cm      = pcs->ppcs->av1_cm;
    FrameHeader*           frm_hdr = &pcs->ppcs->frm_hdr;
    // single_motion_search supports 8bit path only
    uint32_t full_lambda = ctx->full_lambda_md[EB_8_BIT_MD];

    x->xd            = ctx->blk_ptr->av1xd;
    const int mi_row = -x->xd->mb_to_top_edge / (8 * MI_SIZE);
    const int mi_col = -x->xd->mb_to_left_edge / (8 * MI_SIZE);

    x->nmv_vec_cost  = ctx->md_rate_est_ctx->nmv_vec_cost;
    x->mv_cost_stack = ctx->md_rate_est_ctx->nmvcoststack;
    // Set up limit values for MV components.
    // Mv beyond the range do not produce new/different prediction block.
    const int mi_width   = mi_size_wide[bsize];
    const int mi_height  = mi_size_high[bsize];
    x->mv_limits.row_min = -(((mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND);
    x->mv_limits.col_min = -(((mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND);
    x->mv_limits.row_max = (cm->mi_rows - mi_row) * MI_SIZE + AOM_INTERP_EXTEND;
    x->mv_limits.col_max = (cm->mi_cols - mi_col) * MI_SIZE + AOM_INTERP_EXTEND;
    //set search paramters
    x->sadperbit16 = svt_aom_get_sad_per_bit(frm_hdr->quantization_params.base_q_idx, 0);
    x->errorperbit = full_lambda >> RD_EPB_SHIFT;
    x->errorperbit += (x->errorperbit == 0);
    if (do_full_refine) {
        int      sadpb         = x->sadperbit16;
        MvLimits tmp_mv_limits = x->mv_limits;

        // Note: MV limits are modified here. Always restore the original values
        // after full-pixel motion search.
        svt_av1_set_mv_search_range(&x->mv_limits, ref_mv);

        Mv mvp_full = best_pred_mv; // mbmi->mv[0].as_mv;

        // TODO: should use get_fullmv_from_mv instead of shifting
        mvp_full.x >>= 3;
        mvp_full.y >>= 3;

        x->best_mv.as_int = x->second_best_mv.as_int = INVALID_MV; //D

        switch (cand->block_mi.motion_mode) {
        case OBMC_CAUSAL:
            svt_av1_obmc_full_pixel_search(
                ctx, x, &mvp_full, sadpb, &svt_aom_mefn_ptr[bsize], ref_mv, &(x->best_mv), 0);
            break;
        default:
            assert(0 && "Invalid motion mode!\n");
        }

        x->mv_limits = tmp_mv_limits;
    } else { // round-up the default
        x->best_mv.x = best_pred_mv.x >> 3;
        x->best_mv.y = best_pred_mv.y >> 3;
    }

    if (do_frac_refine) {
        int          dis; /* TODO: use dis in distortion calculation later. */
        unsigned int sse1; //unused
        switch (cand->block_mi.motion_mode) {
        case OBMC_CAUSAL:
            svt_av1_find_best_obmc_sub_pixel_tree_up(ctx,
                                                     x,
                                                     cm,
                                                     mi_row,
                                                     mi_col,
                                                     &x->best_mv,
                                                     ref_mv,
                                                     frm_hdr->allow_high_precision_mv,
                                                     x->errorperbit,
                                                     &svt_aom_mefn_ptr[bsize],
                                                     0, // mv.subpel_force_stop
                                                     2, //  mv.subpel_iters_per_step
                                                     x->nmv_vec_cost,
                                                     x->mv_cost_stack,
                                                     &dis,
                                                     &sse1,
                                                     0,
                                                     USE_8_TAPS);

            break;
        default:
            assert(0 && "Invalid motion mode!\n");
        }
    } else {
        x->best_mv.x <<= 3;
        x->best_mv.y <<= 3;
    }
    if (ctx->approx_inter_rate) {
        *rate_mv = svt_av1_mv_bit_cost_light(&x->best_mv, ref_mv);
    } else {
        *rate_mv = svt_av1_mv_bit_cost(&x->best_mv, ref_mv, x->nmv_vec_cost, x->mv_cost_stack, MV_COST_WEIGHT);
    }
}

// Refine the OBMC MV (8 bit search). Return true if search found a valid MV; false otherwise
uint8_t svt_aom_obmc_motion_refinement(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidate* cand,
                                       int refine_level) {
    if (block_size_wide[ctx->blk_geom->bsize] > ctx->obmc_ctrls.max_blk_size_to_refine ||
        block_size_high[ctx->blk_geom->bsize] > ctx->obmc_ctrls.max_blk_size_to_refine) {
        return 1;
    }

    if (ctx->obmc_weighted_pred_ready == false) {
        int mi_row = ctx->blk_org_y >> 2;
        int mi_col = ctx->blk_org_x >> 2;

        DECLARE_ALIGNED(16, uint8_t, dst_buf1_8b[4 * MAX_MB_PLANE * MAX_SB_SQUARE]);

        uint8_t* dst_buf2_8b = dst_buf1_8b + 2 * MAX_MB_PLANE * MAX_SB_SQUARE;
        if (ctx->obmc_is_luma_neigh_10bit) {
            svt_aom_un_pack2d((uint16_t*)ctx->obmc_buff_0,
                              ctx->blk_geom->bwidth,
                              dst_buf1_8b,
                              ctx->blk_geom->bwidth,
                              NULL,
                              ctx->blk_geom->bwidth,
                              ctx->blk_geom->bwidth,
                              ctx->blk_geom->bheight);

            svt_aom_un_pack2d((uint16_t*)ctx->obmc_buff_1,
                              ctx->blk_geom->bwidth,
                              dst_buf2_8b,
                              ctx->blk_geom->bwidth,
                              NULL,
                              ctx->blk_geom->bwidth,
                              ctx->blk_geom->bwidth,
                              ctx->blk_geom->bheight);
        }

        calc_target_weighted_pred(pcs,
                                  ctx,
                                  pcs->ppcs->av1_cm,
                                  ctx->blk_ptr->av1xd,
                                  mi_row,
                                  mi_col,
                                  ctx->obmc_is_luma_neigh_10bit ? dst_buf1_8b : ctx->obmc_buff_0,
                                  ctx->blk_geom->bwidth,
                                  ctx->obmc_is_luma_neigh_10bit ? dst_buf2_8b : ctx->obmc_buff_1,
                                  ctx->blk_geom->bwidth);

        ctx->obmc_weighted_pred_ready = true;
    }
    Mv              best_pred_mv[2] = {{{0}}, {{0}}};
    IntraBcContext  x_st;
    IntraBcContext* x = &x_st;

    MacroBlockD* xd;
    xd = x->xd       = ctx->blk_ptr->av1xd;
    const int mi_row = -xd->mb_to_top_edge / (8 * MI_SIZE);
    const int mi_col = -xd->mb_to_left_edge / (8 * MI_SIZE);

    {
        assert(cand->block_mi.ref_frame[1] == NONE_FRAME); // OBMC only allowed for unipred cands
        uint8_t ref_idx  = get_ref_frame_idx(cand->block_mi.ref_frame[0]);
        uint8_t list_idx = get_list_idx(cand->block_mi.ref_frame[0]);

        assert(list_idx < MAX_NUM_OF_REF_PIC_LIST);
        EbPictureBufferDesc* reference_picture =
            ((EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr)->reference_picture;

        svt_aom_use_scaled_rec_refs_if_needed(pcs,
                                              pcs->ppcs->enhanced_pic,
                                              (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr,
                                              &reference_picture,
                                              EB_8_BIT_MD);
        Yv12BufferConfig ref_buf;
        svt_aom_link_eb_to_aom_buffer_desc_8bit(reference_picture, &ref_buf);

        Buf2D yv12_mb[MAX_MB_PLANE];
        svt_av1_setup_pred_block(ctx->blk_geom->bsize, yv12_mb, &ref_buf, mi_row, mi_col);
        for (int i = 0; i < 1; ++i) {
            x->xdplane[i].pre[0] = yv12_mb[i]; //ref in ME
        }

        x->plane[0].src.buf  = 0; // x->xdplane[0].pre[0];
        x->plane[0].src.buf0 = 0;
    }

    Mv  best_mv = {.as_int = cand->block_mi.mv[0].as_int};
    int tmp_rate_mv;

    Mv ref_mv = {.as_int = cand->pred_mv[0].as_int};

    single_motion_search(pcs, ctx, cand, best_mv, x, ctx->blk_geom->bsize, &ref_mv, &tmp_rate_mv, refine_level);
    cand->block_mi.mv[0].as_int = x->best_mv.as_int;
    svt_aom_choose_best_av1_mv_pred(ctx,
                                    cand->block_mi.ref_frame[0], // OBMC only allowed for unipred candidtes
                                    cand->block_mi.mode,
                                    cand->block_mi.mv[0],
                                    (Mv){{0}},
                                    &cand->drl_index,
                                    best_pred_mv);
    cand->pred_mv[0].as_int = best_pred_mv[0].as_int;
    // Check that final chosen MV is valid
    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, cand->block_mi.mv[0], cand->block_mi.mv[0], 0)) {
        return 1;
    }

    return 0;
}
#endif // CONFIG_ENABLE_OBMC

/*
   inject ME candidates for Light PD0
*/
static void inject_new_candidates_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                            uint32_t* candidate_total_cnt, const bool allow_bipred) {
    const uint32_t         me_sb_addr       = ctx->me_sb_addr;
    const uint32_t         me_block_offset  = ctx->me_block_offset;
    ModeDecisionCandidate* cand_array       = ctx->fast_cand_array;
    uint32_t               cand_total_cnt   = (*candidate_total_cnt);
    const MeSbResults*     me_results       = pcs->ppcs->pa_me_data->me_results[me_sb_addr];
    const uint8_t          total_me_cnt     = me_results->total_me_candidate_index[me_block_offset];
    const MeCandidate*     me_block_results = &me_results->me_candidate_array[ctx->me_cand_offset];

    const uint8_t max_refs = pcs->ppcs->pa_me_data->max_refs;
    const uint8_t max_l0   = pcs->ppcs->pa_me_data->max_l0;

    for (uint8_t me_candidate_index = 0; me_candidate_index < total_me_cnt; ++me_candidate_index) {
        const MeCandidate* me_block_results_ptr = &me_block_results[me_candidate_index];
        const uint8_t      inter_direction      = me_block_results_ptr->direction;
        const uint8_t      list0_ref_index      = me_block_results_ptr->ref_idx_l0;
        const uint8_t      list1_ref_index      = me_block_results_ptr->ref_idx_l1;

        if (ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0 && inter_direction == BI_PRED) {
            continue;
        }

        /**************
            NEWMV
        ************* */
        if (inter_direction < BI_PRED) {
            const uint8_t list_idx = inter_direction;
            const uint8_t ref_idx  = inter_direction ? list1_ref_index : list0_ref_index;
            const int16_t to_inject_mv_x =
                (me_results->me_mv_array[me_block_offset * max_refs + (inter_direction ? max_l0 : 0) + ref_idx].x) << 3;
            const int16_t to_inject_mv_y =
                (me_results->me_mv_array[me_block_offset * max_refs + (inter_direction ? max_l0 : 0) + ref_idx].y) << 3;
            const uint8_t to_inject_ref_type = svt_get_ref_frame_type(list_idx, ref_idx);

            ModeDecisionCandidate* cand = &cand_array[cand_total_cnt];
            cand->block_mi.mode         = NEWMV;
            cand->block_mi.mv[0]        = (Mv){{to_inject_mv_x, to_inject_mv_y}};
            cand->block_mi.ref_frame[0] = to_inject_ref_type;
            cand->block_mi.ref_frame[1] = NONE_FRAME;
            INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
            if (cand_total_cnt > 2) {
                break;
            }
        } else if (allow_bipred) {
            assert(inter_direction == BI_PRED);
            /**************
               NEW_NEWMV
            ************* */
            const uint32_t ref0_offset = me_block_offset * max_refs +
                (me_block_results_ptr->ref0_list > 0 ? max_l0 : 0) + list0_ref_index;
            const uint32_t ref1_offset = me_block_offset * max_refs +
                (me_block_results_ptr->ref1_list > 0 ? max_l0 : 0) + list1_ref_index;
            const int16_t to_inject_mv_x_l0 = (me_results->me_mv_array[ref0_offset].x) << 3;
            const int16_t to_inject_mv_y_l0 = (me_results->me_mv_array[ref0_offset].y) << 3;
            const int16_t to_inject_mv_x_l1 = (me_results->me_mv_array[ref1_offset].x) << 3;
            const int16_t to_inject_mv_y_l1 = (me_results->me_mv_array[ref1_offset].y) << 3;

            MvReferenceFrame rf[2] = {svt_get_ref_frame_type(me_block_results_ptr->ref0_list, list0_ref_index),
                                      svt_get_ref_frame_type(me_block_results_ptr->ref1_list, list1_ref_index)};

            // Inject AVG candidate only
            ModeDecisionCandidate* cand   = &cand_array[cand_total_cnt];
            cand->block_mi.mv[REF_LIST_0] = (Mv){{to_inject_mv_x_l0, to_inject_mv_y_l0}};
            cand->block_mi.mv[REF_LIST_1] = (Mv){{to_inject_mv_x_l1, to_inject_mv_y_l1}};
            cand->block_mi.mode           = NEW_NEWMV;
            cand->block_mi.ref_frame[0]   = rf[0];
            cand->block_mi.ref_frame[1]   = rf[1];
            determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
            INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
            if (cand_total_cnt > 2) {
                break;
            }
        }
    }
    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;
}

static void inject_new_candidates_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                            uint32_t* candidate_total_cnt, const bool allow_bipred) {
    const uint32_t         me_sb_addr       = ctx->me_sb_addr;
    const uint32_t         me_block_offset  = ctx->me_block_offset;
    ModeDecisionCandidate* cand_array       = ctx->fast_cand_array;
    Mv                     best_pred_mv[2]  = {{{0}}, {{0}}};
    uint32_t               cand_total_cnt   = (*candidate_total_cnt);
    const MeSbResults*     me_results       = pcs->ppcs->pa_me_data->me_results[me_sb_addr];
    const uint8_t          total_me_cnt     = me_results->total_me_candidate_index[me_block_offset];
    const MeCandidate*     me_block_results = &me_results->me_candidate_array[ctx->me_cand_offset];

    for (uint8_t me_candidate_index = 0; me_candidate_index < total_me_cnt; ++me_candidate_index) {
        const MeCandidate* me_block_results_ptr = &me_block_results[me_candidate_index];
        const uint8_t      inter_direction      = me_block_results_ptr->direction;
        const uint8_t      list0_ref_index      = me_block_results_ptr->ref_idx_l0;
        const uint8_t      list1_ref_index      = me_block_results_ptr->ref_idx_l1;

        if (ctx->cand_reduction_ctrls.reduce_unipred_candidates >= 2) {
            if ((total_me_cnt > 1) && (inter_direction != 2)) {
                continue;
            }
        } else if (ctx->cand_reduction_ctrls.reduce_unipred_candidates) {
            if ((total_me_cnt > 3) && (inter_direction != 2)) {
                continue;
            }
        }

        /**************
            NEWMV
        ************* */
        if (inter_direction < BI_PRED) {
            const uint8_t list_idx           = inter_direction;
            const uint8_t ref_idx            = inter_direction ? list1_ref_index : list0_ref_index;
            Mv            to_inj_mv          = ctx->sb_me_mv[list_idx][ref_idx];
            const uint8_t to_inject_ref_type = svt_get_ref_frame_type(list_idx, ref_idx);
            if (ctx->injected_mv_count == 0 ||
                mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, to_inject_ref_type) == false) {
                uint8_t drl_index = 0;
                svt_aom_choose_best_av1_mv_pred(
                    ctx, to_inject_ref_type, NEWMV, to_inj_mv, (Mv){{0}}, &drl_index, best_pred_mv);
                if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, 0)) {
                    ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                    cand->block_mi.use_intrabc        = 0;
                    cand->block_mi.is_interintra_used = 0;
                    cand->skip_mode_allowed           = false;
                    cand->block_mi.mode               = NEWMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->drl_index                   = drl_index;
                    cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                    cand->block_mi.ref_frame[0]       = to_inject_ref_type;
                    cand->block_mi.ref_frame[1]       = NONE_FRAME;
                    cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                    cand->block_mi.num_proj_ref       = ctx->wm_sample_info[to_inject_ref_type].num;
                    INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
                    // Add the injected MV to the list of injected MVs
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                    ++ctx->injected_mv_count;
                }
            }
        } else if (allow_bipred && inter_direction == 2 &&
                   !(ctx->is_intra_bordered && ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled)) {
            /**************
               NEW_NEWMV
            ************* */
            Mv               to_inj_mv0 = ctx->sb_me_mv[me_block_results_ptr->ref0_list][list0_ref_index];
            Mv               to_inj_mv1 = ctx->sb_me_mv[me_block_results_ptr->ref1_list][list1_ref_index];
            MvReferenceFrame rf[2]      = {svt_get_ref_frame_type(me_block_results_ptr->ref0_list, list0_ref_index),
                                           svt_get_ref_frame_type(me_block_results_ptr->ref1_list, list1_ref_index)};
            uint8_t          to_inject_ref_type = av1_ref_frame_type(rf);
            if ((ctx->injected_mv_count == 0 ||
                 mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, to_inject_ref_type) == false)) {
                uint8_t drl_index = 0;
                svt_aom_choose_best_av1_mv_pred(
                    ctx, to_inject_ref_type, NEW_NEWMV, to_inj_mv0, to_inj_mv1, &drl_index, best_pred_mv);
                if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, 1)) {
                    ModeDecisionCandidate* cand         = &cand_array[cand_total_cnt];
                    cand->block_mi.use_intrabc          = 0;
                    cand->block_mi.is_interintra_used   = 0;
                    cand->skip_mode_allowed             = false;
                    cand->drl_index                     = drl_index;
                    cand->block_mi.mv[0].as_int         = to_inj_mv0.as_int;
                    cand->block_mi.mv[1].as_int         = to_inj_mv1.as_int;
                    cand->block_mi.mode                 = NEW_NEWMV;
                    cand->block_mi.motion_mode          = SIMPLE_TRANSLATION;
                    cand->block_mi.ref_frame[0]         = rf[0];
                    cand->block_mi.ref_frame[1]         = rf[1];
                    cand->pred_mv[0].as_int             = best_pred_mv[0].as_int;
                    cand->pred_mv[1].as_int             = best_pred_mv[1].as_int;
                    cand->block_mi.comp_group_idx       = 0;
                    cand->block_mi.compound_idx         = 1;
                    cand->block_mi.interinter_comp.type = COMPOUND_AVERAGE;
                    INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                    // Add the injected MV to the list of injected MVs
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                    ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                    ++ctx->injected_mv_count;
                }
            }
        }
    }
    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;
}

static void inject_new_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* candidate_total_cnt,
                                  const bool allow_bipred) {
    const uint32_t         me_sb_addr       = ctx->me_sb_addr;
    const uint32_t         me_block_offset  = ctx->me_block_offset;
    ModeDecisionCandidate* cand_array       = ctx->fast_cand_array;
    Mv                     best_pred_mv[2]  = {{{0}}, {{0}}};
    uint32_t               cand_total_cnt   = (*candidate_total_cnt);
    const MeSbResults*     me_results       = pcs->ppcs->pa_me_data->me_results[me_sb_addr];
    const uint8_t          total_me_cnt     = me_results->total_me_candidate_index[me_block_offset];
    const MeCandidate*     me_block_results = &me_results->me_candidate_array[ctx->me_cand_offset];

    for (uint8_t me_candidate_index = 0; me_candidate_index < total_me_cnt; ++me_candidate_index) {
        const MeCandidate* me_block_results_ptr = &me_block_results[me_candidate_index];
        const uint8_t      inter_direction      = me_block_results_ptr->direction;
        const uint8_t      list0_ref_index      = me_block_results_ptr->ref_idx_l0;
        const uint8_t      list1_ref_index      = me_block_results_ptr->ref_idx_l1;

        if (ctx->cand_reduction_ctrls.reduce_unipred_candidates) {
            if ((total_me_cnt > 3) && (inter_direction != 2)) {
                continue;
            }
        }

        /**************
            NEWMV unipred
        ************* */
        if (inter_direction < BI_PRED) {
            const uint8_t list_idx = inter_direction;
            const uint8_t ref_idx  = list_idx == REF_LIST_0 ? list0_ref_index : list1_ref_index;
            if (!svt_aom_is_valid_unipred_ref(ctx, MIN(TOT_INTER_GROUP - 1, PA_ME_GROUP), list_idx, ref_idx)) {
                continue;
            }
            Mv      to_inj_mv          = ctx->sb_me_mv[list_idx][ref_idx];
            uint8_t to_inject_ref_type = svt_get_ref_frame_type(list_idx, ref_idx);
            if ((ctx->injected_mv_count == 0 ||
                 mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, to_inject_ref_type) == false)) {
                uint8_t drl_index = 0;
                svt_aom_choose_best_av1_mv_pred(
                    ctx, to_inject_ref_type, NEWMV, to_inj_mv, (Mv){{0}}, &drl_index, best_pred_mv);
                if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, 0)) {
                    ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->block_mi.mode               = NEWMV;
                    cand->drl_index                   = drl_index;
                    cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                    cand->block_mi.ref_frame[0]       = to_inject_ref_type;
                    cand->block_mi.ref_frame[1]       = NONE_FRAME;
                    cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.num_proj_ref       = ctx->wm_sample_info[to_inject_ref_type].num;
                    INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                    const bool enable_ii   = true;
                    const bool enable_obmc = true;
                    const bool enable_warp = true;
                    inj_non_simple_modes(pcs, ctx, &cand_total_cnt, enable_ii, enable_warp, enable_obmc);

                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                    ++ctx->injected_mv_count;
                }
            }
        } else if (allow_bipred &&
                   !(ctx->is_intra_bordered && ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled)) {
            assert(inter_direction == BI_PRED);
            /**************
               NEW_NEWMV
            ************* */
            if (!is_valid_bipred_ref(ctx,
                                     PA_ME_GROUP,
                                     me_block_results_ptr->ref0_list,
                                     list0_ref_index,
                                     me_block_results_ptr->ref1_list,
                                     list1_ref_index)) {
                continue;
            }
            Mv      to_inj_mv0         = ctx->sb_me_mv[me_block_results_ptr->ref0_list][list0_ref_index];
            Mv      to_inj_mv1         = ctx->sb_me_mv[me_block_results_ptr->ref1_list][list1_ref_index];
            uint8_t to_inject_ref_type = av1_ref_frame_type(
                (const MvReferenceFrame[]){svt_get_ref_frame_type(me_block_results_ptr->ref0_list, list0_ref_index),
                                           svt_get_ref_frame_type(me_block_results_ptr->ref1_list, list1_ref_index)});
            if ((ctx->injected_mv_count == 0 ||
                 mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, to_inject_ref_type) == false)) {
                uint8_t drl_index = 0;
                svt_aom_choose_best_av1_mv_pred(
                    ctx, to_inject_ref_type, NEW_NEWMV, to_inj_mv0, to_inj_mv1, &drl_index, best_pred_mv);
                if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, 1)) {
                    MvReferenceFrame rf[2] = {svt_get_ref_frame_type(me_block_results_ptr->ref0_list, list0_ref_index),
                                              svt_get_ref_frame_type(me_block_results_ptr->ref1_list, list1_ref_index)};
                    ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                    cand->block_mi.use_intrabc        = 0;
                    cand->skip_mode_allowed           = false;
                    cand->drl_index                   = drl_index;
                    cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                    cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                    cand->block_mi.mode               = NEW_NEWMV;
                    cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                    cand->block_mi.is_interintra_used = 0;
                    cand->block_mi.ref_frame[0]       = rf[0];
                    cand->block_mi.ref_frame[1]       = rf[1];
                    cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                    cand->pred_mv[1].as_int           = best_pred_mv[1].as_int;
                    determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                    INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                    if (ctx->inter_comp_ctrls.do_me) {
                        ctx->cmp_store.pred0_cnt = 0;
                        ctx->cmp_store.pred1_cnt = 0;
                        inj_comp_modes(pcs, ctx, &cand_total_cnt);
                    }
                    ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                    ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                    ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                    ++ctx->injected_mv_count;
                }
            }
        }
    }
    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;
}

static void inject_global_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* candidate_total_cnt,
                                     const bool allow_bipred) {
    ModeDecisionCandidate* cand_array     = ctx->fast_cand_array;
    uint32_t               cand_total_cnt = (*candidate_total_cnt);
    uint32_t               mi_row         = ctx->blk_org_y >> MI_SIZE_LOG2;
    uint32_t               mi_col         = ctx->blk_org_x >> MI_SIZE_LOG2;

    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        //single ref/list
        if (rf[1] == NONE_FRAME) {
            MvReferenceFrame frame_type = rf[0];
            uint8_t          list_idx   = get_list_idx(rf[0]);
            uint8_t          ref_idx    = get_ref_frame_idx(rf[0]);

            if (!svt_aom_is_valid_unipred_ref(ctx, GLOBAL_GROUP, list_idx, ref_idx)) {
                continue;
            }
            // Get gm params
            WarpedMotionParams* gm_params = &pcs->ppcs->global_motion[frame_type];
            if (pcs->ppcs->gm_ctrls.skip_identity && gm_params->wmtype == IDENTITY) {
                continue;
            }
            Mv to_inj_mv = svt_aom_gm_get_motion_vector_enc(gm_params,
                                                            pcs->ppcs->frm_hdr.allow_high_precision_mv,
                                                            ctx->blk_geom->bsize,
                                                            mi_col,
                                                            mi_row,
                                                            0 /* force_integer_mv */);

            assert(list_idx == 0 || list_idx == 1);
            ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
            cand->block_mi.mode               = GLOBALMV;
            cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
            cand->block_mi.is_interintra_used = 0;
            cand->wm_params_l0                = *gm_params;
            cand->wm_params_l1                = *gm_params;
            cand->block_mi.use_intrabc        = 0;
            cand->skip_mode_allowed           = false;
            cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
            cand->drl_index                   = 0;
            cand->block_mi.ref_frame[0]       = rf[0];
            cand->block_mi.ref_frame[1]       = rf[1];
            cand->block_mi.num_proj_ref       = ctx->wm_sample_info[frame_type].num;
            INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

            const bool enable_ii   = true;
            const bool enable_obmc = false;
            const bool enable_warp = false;
            inj_non_simple_modes(pcs, ctx, &cand_total_cnt, enable_ii, enable_warp, enable_obmc);
            ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
            ctx->injected_ref_types[ctx->injected_mv_count]     = frame_type;
            ++ctx->injected_mv_count;
        } else if (allow_bipred) {
            uint8_t ref_idx_0  = get_ref_frame_idx(rf[0]);
            uint8_t ref_idx_1  = get_ref_frame_idx(rf[1]);
            uint8_t list_idx_0 = get_list_idx(rf[0]);
            uint8_t list_idx_1 = get_list_idx(rf[1]);

            if (!is_valid_bipred_ref(ctx, GLOBAL_GROUP, list_idx_0, ref_idx_0, list_idx_1, ref_idx_1)) {
                return;
            }
            // Get gm params
            WarpedMotionParams* gm_params_0 = &pcs->ppcs->global_motion[svt_get_ref_frame_type(list_idx_0, ref_idx_0)];

            WarpedMotionParams* gm_params_1 = &pcs->ppcs->global_motion[svt_get_ref_frame_type(list_idx_1, ref_idx_1)];

            if (pcs->ppcs->gm_ctrls.skip_identity &&
                (gm_params_0->wmtype == IDENTITY || gm_params_1->wmtype == IDENTITY)) {
                continue;
            }
            Mv to_inj_mv0 = svt_aom_gm_get_motion_vector_enc(gm_params_0,
                                                             pcs->ppcs->frm_hdr.allow_high_precision_mv,
                                                             ctx->blk_geom->bsize,
                                                             mi_col,
                                                             mi_row,
                                                             0 /* force_integer_mv */);

            Mv      to_inj_mv1         = svt_aom_gm_get_motion_vector_enc(gm_params_1,
                                                             pcs->ppcs->frm_hdr.allow_high_precision_mv,
                                                             ctx->blk_geom->bsize,
                                                             mi_col,
                                                             mi_row,
                                                             0 /* force_integer_mv */);
            uint8_t to_inject_ref_type = av1_ref_frame_type(rf);

            ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
            cand->block_mi.use_intrabc        = 0;
            cand->skip_mode_allowed           = false;
            cand->block_mi.mode               = GLOBAL_GLOBALMV;
            cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
            cand->wm_params_l0                = *gm_params_0;
            cand->wm_params_l1                = *gm_params_1;
            cand->block_mi.is_interintra_used = 0;
            cand->drl_index                   = 0;
            cand->block_mi.ref_frame[0]       = rf[0];
            cand->block_mi.ref_frame[1]       = rf[1];
            cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
            cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
            determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
            INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

            if (ctx->inter_comp_ctrls.do_global) {
                ctx->cmp_store.pred0_cnt = 0;
                ctx->cmp_store.pred1_cnt = 0;
                inj_comp_modes(pcs, ctx, &cand_total_cnt);
            }
            ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
            ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
            ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
            ++ctx->injected_mv_count;
        }
    }
    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;
}

static void inject_pme_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* candidate_total_cnt,
                                  const bool allow_bipred) {
    ModeDecisionCandidate* cand_array      = ctx->fast_cand_array;
    Mv                     best_pred_mv[2] = {{{0}}, {{0}}};
    uint32_t               cand_total_cnt  = (*candidate_total_cnt);
    for (uint32_t ref_it = 0; ref_it < ctx->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = ctx->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        //single ref/list
        if (rf[1] == NONE_FRAME) {
            MvReferenceFrame frame_type = rf[0];
            uint8_t          list_idx   = get_list_idx(rf[0]);
            uint8_t          ref_idx    = get_ref_frame_idx(rf[0]);

            if (ctx->valid_pme_mv[list_idx][ref_idx]) {
                Mv to_inj_mv = ctx->best_pme_mv[list_idx][ref_idx];
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv, to_inj_mv, frame_type) == false)) {
                    uint8_t drl_index = 0;
                    svt_aom_choose_best_av1_mv_pred(
                        ctx, frame_type, NEWMV, to_inj_mv, (Mv){{0}}, &drl_index, best_pred_mv);
                    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv, to_inj_mv, 0)) {
                        ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                        cand->block_mi.use_intrabc        = 0;
                        cand->skip_mode_allowed           = false;
                        cand->block_mi.mode               = NEWMV;
                        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                        cand->block_mi.is_interintra_used = 0;
                        cand->drl_index                   = drl_index;
                        cand->block_mi.mv[0].as_int       = to_inj_mv.as_int;
                        cand->block_mi.ref_frame[0]       = rf[0];
                        cand->block_mi.ref_frame[1]       = rf[1];
                        cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                        cand->block_mi.num_proj_ref       = ctx->wm_sample_info[frame_type].num;
                        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                        const bool enable_ii   = true;
                        const bool enable_obmc = true;
                        const bool enable_warp = true;
                        inj_non_simple_modes(pcs, ctx, &cand_total_cnt, enable_ii, enable_warp, enable_obmc);
                        ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv.as_int;
                        ctx->injected_ref_types[ctx->injected_mv_count]     = frame_type;
                        ++ctx->injected_mv_count;
                    }
                }
            }
        } else if (allow_bipred) {
            uint8_t ref_idx_0  = get_ref_frame_idx(rf[0]);
            uint8_t ref_idx_1  = get_ref_frame_idx(rf[1]);
            uint8_t list_idx_0 = get_list_idx(rf[0]);
            uint8_t list_idx_1 = get_list_idx(rf[1]);

            if (ctx->valid_pme_mv[list_idx_0][ref_idx_0] && ctx->valid_pme_mv[list_idx_1][ref_idx_1]) {
                Mv            to_inj_mv0         = ctx->best_pme_mv[list_idx_0][ref_idx_0];
                Mv            to_inj_mv1         = ctx->best_pme_mv[list_idx_1][ref_idx_1];
                const uint8_t to_inject_ref_type = av1_ref_frame_type((const MvReferenceFrame[]){
                    svt_get_ref_frame_type(list_idx_0, ref_idx_0),
                    svt_get_ref_frame_type(list_idx_1, ref_idx_1),
                });
                if ((ctx->injected_mv_count == 0 ||
                     mv_is_already_injected(ctx, to_inj_mv0, to_inj_mv1, to_inject_ref_type) == false)) {
                    uint8_t drl_index = 0;
                    svt_aom_choose_best_av1_mv_pred(
                        ctx, to_inject_ref_type, NEW_NEWMV, to_inj_mv0, to_inj_mv1, &drl_index, best_pred_mv);
                    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, to_inj_mv0, to_inj_mv1, 1)) {
                        ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
                        cand->block_mi.use_intrabc        = 0;
                        cand->skip_mode_allowed           = false;
                        cand->drl_index                   = drl_index;
                        cand->block_mi.mv[0].as_int       = to_inj_mv0.as_int;
                        cand->block_mi.mv[1].as_int       = to_inj_mv1.as_int;
                        cand->block_mi.mode               = NEW_NEWMV;
                        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
                        cand->block_mi.is_interintra_used = 0;
                        cand->block_mi.ref_frame[0]       = rf[0];
                        cand->block_mi.ref_frame[1]       = rf[1];
                        cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
                        cand->pred_mv[1].as_int           = best_pred_mv[1].as_int;
                        determine_compound_mode(pcs, ctx, cand, MD_COMP_AVG);
                        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);

                        if (ctx->inter_comp_ctrls.do_pme) {
                            ctx->cmp_store.pred0_cnt = 0;
                            ctx->cmp_store.pred1_cnt = 0;
                            inj_comp_modes(pcs, ctx, &cand_total_cnt);
                        }
                        ctx->injected_mvs[ctx->injected_mv_count][0].as_int = to_inj_mv0.as_int;
                        ctx->injected_mvs[ctx->injected_mv_count][1].as_int = to_inj_mv1.as_int;
                        ctx->injected_ref_types[ctx->injected_mv_count]     = to_inject_ref_type;
                        ++ctx->injected_mv_count;
                    }
                }
            }
        }
    }
    (*candidate_total_cnt) = cand_total_cnt;
}

static void inject_inter_candidates_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                              uint32_t* candidate_total_cnt) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    // Bipred prediction is only allowed when both dimensions are > 4 and the frame-header reference mode allows it.
    // See AV1 spec 5.11.25
    const bool allow_bipred = (frm_hdr->reference_mode == SINGLE_REFERENCE || ctx->blk_geom->bwidth == 4 ||
                               ctx->blk_geom->bheight == 4)
        ? false
        : true;

    inject_new_candidates_light_pd0(pcs, ctx, candidate_total_cnt, allow_bipred);
}

static void inject_inter_candidates_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                              uint32_t* cand_total_cnt) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    // Bipred prediction is only allowed when both dimensions are > 4 and the frame-header reference mode allows it.
    // See AV1 spec 5.11.25
    const bool allow_bipred = (frm_hdr->reference_mode == SINGLE_REFERENCE || ctx->blk_geom->bwidth == 4 ||
                               ctx->blk_geom->bheight == 4)
        ? false
        : true;
    // Needed in case WM/OBMC is on at the frame level (even though not used in light-PD1 path)
    if (frm_hdr->is_motion_mode_switchable) {
        const uint16_t mi_row = ctx->blk_org_y >> MI_SIZE_LOG2;
        const uint16_t mi_col = ctx->blk_org_x >> MI_SIZE_LOG2;
        svt_av1_count_overlappable_neighbors(pcs, ctx->blk_ptr, ctx->blk_geom->bsize, mi_row, mi_col);
    } else {
        // Overlappable neighbours only needed for non-"SIMPLE_TRANSLATION" candidates
        ctx->blk_ptr->overlappable_neighbors = 0;
    }
    svt_aom_init_wm_samples(pcs, ctx);
    // Inject MVP candidates
    if (ctx->new_nearest_injection &&
        !(ctx->is_intra_bordered && ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled)) {
        inject_mvp_candidates_ii_light_pd1(pcs, ctx, cand_total_cnt, allow_bipred);
    }

    // Inject ME candidates
    if (ctx->inject_new_me) {
        inject_new_candidates_light_pd1(pcs, ctx, cand_total_cnt, allow_bipred);
    }
}

static void svt_aom_inject_inter_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                            uint32_t* cand_total_cnt) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    // Bipred prediction is only allowed when both dimensions are > 4 and the frame-header reference mode allows it.
    // See AV1 spec 5.11.25
    const bool allow_bipred = (frm_hdr->reference_mode == SINGLE_REFERENCE || ctx->blk_geom->bwidth == 4 ||
                               ctx->blk_geom->bheight == 4)
        ? false
        : true;

    const uint32_t mi_row = ctx->blk_org_y >> MI_SIZE_LOG2;
    const uint32_t mi_col = ctx->blk_org_x >> MI_SIZE_LOG2;

    svt_av1_count_overlappable_neighbors(pcs, ctx->blk_ptr, ctx->blk_geom->bsize, mi_row, mi_col);
    svt_aom_init_wm_samples(pcs, ctx);
#if CONFIG_ENABLE_OBMC
    if (ctx->obmc_ctrls.enabled && ctx->obmc_ctrls.refine_level == 0) {
        const uint8_t is_obmc_allowed = svt_aom_obmc_motion_mode_allowed(
                                            pcs, ctx, ctx->blk_geom->bsize, 1, LAST_FRAME, -1, NEWMV) == OBMC_CAUSAL;
        if (is_obmc_allowed) {
            svt_aom_precompute_obmc_data(pcs, ctx, PICTURE_BUFFER_DESC_LUMA_MASK);
        }
    }
#endif
    /**************
         MVP
    ************* */
    if (ctx->new_nearest_injection &&
        !(ctx->is_intra_bordered && ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled)) {
        inject_mvp_candidates_ii(pcs, ctx, cand_total_cnt, allow_bipred);
    }
    //----------------------
    //    NEAREST_NEWMV, NEW_NEARESTMV, NEAR_NEWMV, NEW_NEARMV.
    //----------------------
    if (ctx->new_nearest_near_comb_injection && allow_bipred) {
        inject_new_nearest_new_comb_candidates(pcs, ctx, cand_total_cnt);
    }
    if (ctx->inject_new_me) {
        inject_new_candidates(pcs, ctx, cand_total_cnt, allow_bipred);
    }
    if (ctx->global_mv_injection) {
        inject_global_candidates(pcs, ctx, cand_total_cnt, allow_bipred);
    }
    if (ctx->bipred3x3_ctrls.enabled && allow_bipred) {
        bipred_3x3_candidates_injection(pcs, ctx, cand_total_cnt);
    }

    if (ctx->unipred3x3_injection) {
        unipred_3x3_candidates_injection(pcs, ctx, cand_total_cnt);
    }

    // determine when to inject pme candidates based on size and resolution of block
    if (ctx->inject_new_pme && ctx->updated_enable_pme) {
        inject_pme_candidates(pcs, ctx, cand_total_cnt, allow_bipred);
    }
}

static const TxType g_intra_mode_to_tx_type[INTRA_MODES] = {
    DCT_DCT, // DC
    ADST_DCT, // V
    DCT_ADST, // H
    DCT_DCT, // D45
    ADST_ADST, // D135
    ADST_DCT, // D117
    DCT_ADST, // D153
    DCT_ADST, // D207
    ADST_DCT, // D63
    ADST_ADST, // SMOOTH
    ADST_DCT, // SMOOTH_V
    DCT_ADST, // SMOOTH_H
    ADST_ADST, // PAETH
};

static INLINE TxType intra_mode_to_tx_type(PredictionMode pred_mode, UvPredictionMode pred_mode_uv,
                                           PlaneType plane_type) {
    const PredictionMode mode = (plane_type == PLANE_TYPE_Y) ? pred_mode : get_uv_mode(pred_mode_uv);
    assert(mode < INTRA_MODES);
    return g_intra_mode_to_tx_type[mode];
}

/* For intra prediction, the chroma transform type may not follow the luma type.
This function will return the intra chroma TX type to be used, which is based on TX size and chroma mode.
Refer to section 5.11.40 of the AV1 spec (compute_tx_type). */
TxType svt_aom_get_intra_uv_tx_type(UvPredictionMode pred_mode_uv, TxSize tx_size, int32_t reduced_tx_set) {
    if (txsize_sqr_up_map[tx_size] > TX_32X32) {
        return DCT_DCT;
    }

    // In intra mode, uv planes don't share the same prediction mode as y
    // plane, so the tx_type should not be shared. Pass DC_PRED as luma mode because the argument
    // will not be used.
    TxType tx_type = intra_mode_to_tx_type(DC_PRED, pred_mode_uv, PLANE_TYPE_UV);
    assert(tx_type < TX_TYPES);
    const TxSetType tx_set_type = get_ext_tx_set_type(tx_size, /*is_inter*/ 0, reduced_tx_set);
    return !av1_ext_tx_used[tx_set_type][tx_type] ? DCT_DCT : tx_type;
}

// Values are now correlated to quantizer.
static INLINE int mv_check_bounds(const MvLimits* mv_limits, const Mv* mv) {
    return (mv->y >> 3) < mv_limits->row_min || (mv->y >> 3) > mv_limits->row_max ||
        (mv->x >> 3) < mv_limits->col_min || (mv->x >> 3) > mv_limits->col_max;
}

static void assert_release(int statement) {
    if (statement == 0) {
        SVT_LOG("ASSERT_ERRRR\n");
    }
}

static void intra_bc_search(PictureControlSet* pcs, ModeDecisionContext* ctx, const SequenceControlSet* scs,
                            BlkStruct* blk_ptr, Mv* dv_cand, uint8_t* num_dv_cand) {
    IntraBcContext  x_st;
    IntraBcContext* x           = &x_st;
    uint32_t        full_lambda = ctx->hbd_md ? ctx->full_lambda_md[EB_10_BIT_MD] : ctx->full_lambda_md[EB_8_BIT_MD];
    //fill x with what needed.
    x->is_exhaustive_allowed = ctx->blk_geom->bwidth == 4 || ctx->blk_geom->bheight == 4 ? 1 : 0;
    svt_memcpy(&x->crc_calculator, &pcs->crc_calculator, sizeof(pcs->crc_calculator));
    x->approx_inter_rate = ctx->approx_inter_rate;
    x->xd                = blk_ptr->av1xd;
    x->nmv_vec_cost      = ctx->md_rate_est_ctx->nmv_vec_cost;
    x->mv_cost_stack     = ctx->md_rate_est_ctx->nmvcoststack;
    BlockSize bsize      = ctx->blk_geom->bsize;
    assert(bsize < BLOCK_SIZES_ALL);
    FrameHeader*           frm_hdr    = &pcs->ppcs->frm_hdr;
    const Av1Common* const cm         = pcs->ppcs->av1_cm;
    MvReferenceFrame       ref_frame  = INTRA_FRAME;
    const int              num_planes = 3;
    MacroBlockD*           xd         = blk_ptr->av1xd;
    const TileInfo*        tile       = &xd->tile;
    const int              mi_row     = -xd->mb_to_top_edge / (8 * MI_SIZE);
    const int              mi_col     = -xd->mb_to_left_edge / (8 * MI_SIZE);
    const int              w          = block_size_wide[bsize];
    const int              h          = block_size_high[bsize];
    const int              sb_row     = mi_row >> scs->seq_header.sb_size_log2;
    const int              sb_col     = mi_col >> scs->seq_header.sb_size_log2;

    // Set up limit values for MV components.
    // Mv beyond the range do not produce new/different prediction block.
    const int mi_width   = mi_size_wide[bsize];
    const int mi_height  = mi_size_high[bsize];
    x->mv_limits.row_min = -(((mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND);
    x->mv_limits.col_min = -(((mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND);
    x->mv_limits.row_max = (cm->mi_rows - mi_row) * MI_SIZE + AOM_INTERP_EXTEND;
    x->mv_limits.col_max = (cm->mi_cols - mi_col) * MI_SIZE + AOM_INTERP_EXTEND;
    //set search paramters
    x->sadperbit16 = svt_aom_get_sad_per_bit(frm_hdr->quantization_params.base_q_idx, 0);
    x->errorperbit = full_lambda >> RD_EPB_SHIFT;
    x->errorperbit += (x->errorperbit == 0);
    //temp buffer for hash me
    for (int i = 0; i < 2; i++) {
        EB_MALLOC_ARRAY_NO_CHECK(x->hash_value_buffer[i], AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
    }

    Mv nearestmv, nearmv;
    svt_av1_find_best_ref_mvs_from_stack(0, ctx->ref_mv_stack /*mbmi_ext*/, xd, ref_frame, &nearestmv, &nearmv, 0);
    if (nearestmv.as_int == INVALID_MV) {
        nearestmv.as_int = 0;
    }
    if (nearmv.as_int == INVALID_MV) {
        nearmv.as_int = 0;
    }
    Mv dv_ref = nearestmv.as_int == 0 ? nearmv : nearestmv;
    if (dv_ref.as_int == 0) {
        svt_aom_find_ref_dv(&dv_ref, tile, scs->seq_header.sb_mi_size, mi_row, mi_col);
    }
    // Ref DV should not have sub-pel.
    assert((dv_ref.x & 7) == 0);
    assert((dv_ref.y & 7) == 0);
    ctx->ref_mv_stack[INTRA_FRAME][0].this_mv = dv_ref;

    /* pointer to current frame */
    Yv12BufferConfig cur_buf;
    svt_aom_link_eb_to_aom_buffer_desc_8bit(pcs->ppcs->enhanced_pic, &cur_buf);
    struct Buf2D yv12_mb[MAX_MB_PLANE];
    svt_av1_setup_pred_block(bsize, yv12_mb, &cur_buf, mi_row, mi_col);
    for (int i = 0; i < num_planes; ++i) {
        x->xdplane[i].pre[0] = yv12_mb[i]; // ref in ME
    }
    // setup src for DV search same as ref
    x->plane[0].src = x->xdplane[0].pre[0];
    // up to two dv candidates will be generated
    // IBC Modes:   0: OFF 1:Slow   2:Faster   3:Fastest
    enum IntrabcMotionDirection max_dir = pcs->ppcs->intraBC_ctrls.ibc_direction ? IBC_MOTION_LEFT
                                                                                 : IBC_MOTION_DIRECTIONS;

    for (enum IntrabcMotionDirection dir = IBC_MOTION_ABOVE; dir < max_dir; ++dir) {
        const MvLimits tmp_mv_limits = x->mv_limits;

        switch (dir) {
        case IBC_MOTION_ABOVE:
            x->mv_limits.col_min = (tile->mi_col_start - mi_col) * MI_SIZE;
            x->mv_limits.col_max = (tile->mi_col_end - mi_col) * MI_SIZE - w;
            x->mv_limits.row_min = (tile->mi_row_start - mi_row) * MI_SIZE;
            x->mv_limits.row_max = (sb_row * scs->seq_header.sb_mi_size - mi_row) * MI_SIZE - h;
            break;
        case IBC_MOTION_LEFT:
            x->mv_limits.col_min = (tile->mi_col_start - mi_col) * MI_SIZE;
            x->mv_limits.col_max = (sb_col * scs->seq_header.sb_mi_size - mi_col) * MI_SIZE - w;
            // TODO: Minimize the overlap between above and
            // left areas.
            x->mv_limits.row_min     = (tile->mi_row_start - mi_row) * MI_SIZE;
            int bottom_coded_mi_edge = AOMMIN((sb_row + 1) * scs->seq_header.sb_mi_size, tile->mi_row_end);
            x->mv_limits.row_max     = (bottom_coded_mi_edge - mi_row) * MI_SIZE - h;
            break;
        default:
            assert(0);
        }
        assert_release(x->mv_limits.col_min >= tmp_mv_limits.col_min);
        assert_release(x->mv_limits.col_max <= tmp_mv_limits.col_max);
        assert_release(x->mv_limits.row_min >= tmp_mv_limits.row_min);
        assert_release(x->mv_limits.row_max <= tmp_mv_limits.row_max);

        svt_av1_set_mv_search_range(&x->mv_limits, &dv_ref);

        if (x->mv_limits.col_max < x->mv_limits.col_min || x->mv_limits.row_max < x->mv_limits.row_min) {
            x->mv_limits = tmp_mv_limits;
            continue;
        }

        int step_param = 0;
        Mv  mvp_full   = dv_ref;
        // TODO: should use get_fullmv_from_mv instead of shifting
        mvp_full.x >>= 3;
        mvp_full.y >>= 3;
        const int sadpb   = x->sadperbit16;
        x->best_mv.as_int = 0;

#define INT_VAR_MAX 2147483647 // maximum (signed) int value

        const int bestsme = svt_av1_full_pixel_search(
            pcs, x, bsize, &mvp_full, step_param, sadpb, NULL, &dv_ref, MI_SIZE * mi_col, MI_SIZE * mi_row, 1);

        x->mv_limits = tmp_mv_limits;
        if (bestsme == INT_VAR_MAX) {
            continue;
        }
        mvp_full = x->best_mv;

        const Mv dv = {.x = mvp_full.x * 8, .y = mvp_full.y * 8};
        if (mv_check_bounds(&x->mv_limits, &dv)) {
            continue;
        }
        if (!svt_aom_is_dv_valid(dv, xd, mi_row, mi_col, bsize, scs->seq_header.sb_size_log2)) {
            continue;
        }

        // DV should not have sub-pel.
        assert_release((dv.x & 7) == 0);
        assert_release((dv.y & 7) == 0);

        //store output
        dv_cand[*num_dv_cand] = dv;
        (*num_dv_cand)++;
    }

    for (int i = 0; i < 2; i++) {
        EB_FREE_ARRAY(x->hash_value_buffer[i]);
    }
}

static void inject_intra_bc_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx, const SequenceControlSet* scs,
                                       BlkStruct* blk_ptr, uint32_t* cand_cnt) {
    Mv      dv_cand[2];
    uint8_t num_dv_cand = 0;

    //perform dv-pred + search up to 2 dv(s)
    intra_bc_search(pcs, ctx, scs, blk_ptr, dv_cand, &num_dv_cand);

    ModeDecisionCandidate* cand_array = ctx->fast_cand_array;

    for (uint32_t dv_i = 0; dv_i < num_dv_cand; dv_i++) {
        ModeDecisionCandidate* cand               = &cand_array[*cand_cnt];
        cand->palette_info                        = NULL;
        cand->block_mi.use_intrabc                = 1;
        cand->block_mi.angle_delta[PLANE_TYPE_Y]  = 0;
        cand->block_mi.angle_delta[PLANE_TYPE_UV] = 0;
        cand->block_mi.uv_mode                    = UV_DC_PRED;
        cand->block_mi.cfl_alpha_signs            = 0;
        cand->block_mi.cfl_alpha_idx              = 0;
        cand->transform_type[0]                   = DCT_DCT;
        cand->transform_type_uv                   = DCT_DCT;
        cand->block_mi.ref_frame[0]               = INTRA_FRAME;
        cand->block_mi.ref_frame[1]               = NONE_FRAME;
        cand->block_mi.mode                       = DC_PRED;
        cand->block_mi.filter_intra_mode          = FILTER_INTRA_MODES;
        //inter ralated
        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
        cand->block_mi.is_interintra_used = 0;
        cand->skip_mode_allowed           = false;
        cand->block_mi.mv[0].as_int       = dv_cand[dv_i].as_int;
        cand->pred_mv[0].as_int           = ctx->ref_mv_stack[INTRA_FRAME][0].this_mv.as_int;
        cand->drl_index                   = 0;
        cand->block_mi.interp_filters     = av1_broadcast_interp_filter(BILINEAR);
        INC_MD_CAND_CNT((*cand_cnt), pcs->ppcs->max_can_count);
    }
}

static void inject_intra_candidates_light_pd0(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                              uint32_t* candidate_total_cnt) {
    uint32_t               cand_total_cnt     = 0;
    ModeDecisionCandidate* cand               = &ctx->fast_cand_array[cand_total_cnt];
    cand->skip_mode_allowed                   = false;
    cand->palette_info                        = NULL;
    cand->block_mi.use_intrabc                = 0;
    cand->block_mi.filter_intra_mode          = FILTER_INTRA_MODES;
    cand->block_mi.angle_delta[PLANE_TYPE_Y]  = 0;
    cand->block_mi.uv_mode                    = UV_DC_PRED;
    cand->block_mi.angle_delta[PLANE_TYPE_UV] = 0;
    cand->block_mi.cfl_alpha_signs            = 0;
    cand->block_mi.cfl_alpha_idx              = 0;
    cand->transform_type[0]                   = DCT_DCT;
    cand->transform_type_uv                   = DCT_DCT;
    cand->block_mi.ref_frame[0]               = INTRA_FRAME;
    cand->block_mi.ref_frame[1]               = NONE_FRAME;
    cand->block_mi.mode                       = DC_PRED;
    cand->block_mi.motion_mode                = SIMPLE_TRANSLATION;
    cand->block_mi.is_interintra_used         = 0;
    INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;
    return;
}

static void inject_intra_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx, const bool dc_cand_only_flag,
                                    uint32_t* candidate_total_cnt) {
    FrameHeader*           frm_hdr          = &pcs->ppcs->frm_hdr;
    PredictionMode         intra_mode_start = DC_PRED;
    PredictionMode         intra_mode_end   = dc_cand_only_flag ? DC_PRED : ctx->intra_ctrls.intra_mode_end;
    uint32_t               cand_total_cnt   = *candidate_total_cnt;
    ModeDecisionCandidate* cand_array       = ctx->fast_cand_array;
    const bool    use_angle_delta = ctx->intra_ctrls.angular_pred_level ? av1_use_angle_delta(ctx->blk_geom->bsize) : 0;
    const uint8_t disable_angle_prediction                = (ctx->intra_ctrls.angular_pred_level == 0);
    uint8_t       directional_mode_skip_mask[INTRA_MODES] = {0};
    if (ctx->intra_ctrls.angular_pred_level >= 4) {
        for (uint8_t i = D45_PRED; i < INTRA_MODE_END; i++) {
            directional_mode_skip_mask[i] = 1;
        }
    }
    const TxSize tx_size_uv = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);

    for (PredictionMode intra_mode = intra_mode_start; intra_mode <= intra_mode_end; ++intra_mode) {
        if (av1_is_directional_mode(intra_mode) &&
            (disable_angle_prediction || directional_mode_skip_mask[intra_mode])) {
            continue;
        }

        const uint8_t angle_delta_count = av1_is_directional_mode(intra_mode) &&
                ctx->intra_ctrls.angular_pred_level <= 2 && use_angle_delta
            ? 7
            : 1;

        for (uint8_t angle_delta_counter = 0; angle_delta_counter < angle_delta_count; ++angle_delta_counter) {
            int32_t angle_delta = CLIP((angle_delta_count == 1 ? 0 : angle_delta_counter - MAX_ANGLE_DELTA),
                                       -MAX_ANGLE_DELTA,
                                       MAX_ANGLE_DELTA);
            if ((ctx->intra_ctrls.angular_pred_level >= 2 &&
                 (angle_delta == -1 || angle_delta == 1 || angle_delta == -2 || angle_delta == 2)) ||
                (ctx->intra_ctrls.angular_pred_level >= 3 && angle_delta != 0)) {
                continue;
            }
            ModeDecisionCandidate* cand               = &cand_array[cand_total_cnt];
            cand->skip_mode_allowed                   = false;
            cand->palette_info                        = NULL;
            cand->block_mi.mode                       = intra_mode;
            cand->block_mi.use_intrabc                = 0;
            cand->block_mi.filter_intra_mode          = FILTER_INTRA_MODES;
            cand->block_mi.angle_delta[PLANE_TYPE_Y]  = angle_delta;
            cand->block_mi.uv_mode                    = ctx->ind_uv_avail ? ctx->best_uv_mode[intra_mode]
                                                                          : intra_luma_to_chroma[intra_mode];
            cand->block_mi.angle_delta[PLANE_TYPE_UV] = ctx->ind_uv_avail ? ctx->best_uv_angle[intra_mode]
                                                                          : cand->block_mi.angle_delta[PLANE_TYPE_Y];
            cand->block_mi.cfl_alpha_signs            = 0;
            cand->block_mi.cfl_alpha_idx              = 0;
            cand->transform_type[0]                   = DCT_DCT;
            cand->transform_type_uv                   = svt_aom_get_intra_uv_tx_type(
                cand->block_mi.uv_mode, tx_size_uv, frm_hdr->reduced_tx_set);

            if (svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) && cand->transform_type_uv != DCT_DCT) {
                continue;
            }
            cand->block_mi.ref_frame[0]       = INTRA_FRAME;
            cand->block_mi.ref_frame[1]       = NONE_FRAME;
            cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
            cand->block_mi.is_interintra_used = 0;
            INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
        }
    }

    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;

    return;
}

static void inject_filter_intra_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                           uint32_t* candidate_total_cnt) {
    FilterIntraMode intra_mode_start = FILTER_DC_PRED;
    FilterIntraMode intra_mode_end   = ctx->intra_ctrls.intra_mode_end == PAETH_PRED ? FILTER_PAETH_PRED
          : ctx->intra_ctrls.intra_mode_end >= D157_PRED                             ? FILTER_D157_PRED
          : ctx->intra_ctrls.intra_mode_end >= H_PRED                                ? FILTER_H_PRED
          : ctx->intra_ctrls.intra_mode_end >= V_PRED                                ? FILTER_V_PRED
                                                                                     : FILTER_DC_PRED;
    intra_mode_end                   = MIN(intra_mode_end, ctx->filter_intra_ctrls.max_filter_intra_mode);

    const TxSize           tx_size_uv     = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
    uint32_t               cand_total_cnt = *candidate_total_cnt;
    ModeDecisionCandidate* cand_array     = ctx->fast_cand_array;
    FrameHeader*           frm_hdr        = &pcs->ppcs->frm_hdr;

    for (FilterIntraMode filter_intra_mode = intra_mode_start; filter_intra_mode <= intra_mode_end;
         filter_intra_mode++) {
        ModeDecisionCandidate* cand              = &cand_array[cand_total_cnt];
        cand->skip_mode_allowed                  = false;
        cand->block_mi.mode                      = DC_PRED;
        cand->block_mi.use_intrabc               = 0;
        cand->block_mi.filter_intra_mode         = filter_intra_mode;
        cand->palette_info                       = NULL;
        cand->block_mi.angle_delta[PLANE_TYPE_Y] = 0;

        cand->block_mi.uv_mode = ctx->ind_uv_avail ? ctx->best_uv_mode[fimode_to_intramode[filter_intra_mode]]
                                                   : intra_luma_to_chroma[fimode_to_intramode[filter_intra_mode]];
        cand->block_mi.angle_delta[PLANE_TYPE_UV] = ctx->ind_uv_avail
            ? ctx->best_uv_angle[fimode_to_intramode[filter_intra_mode]]
            : cand->block_mi.angle_delta[PLANE_TYPE_Y];

        cand->block_mi.cfl_alpha_signs = 0;
        cand->block_mi.cfl_alpha_idx   = 0;
        cand->transform_type[0]        = DCT_DCT;
        cand->transform_type_uv        = svt_aom_get_intra_uv_tx_type(
            cand->block_mi.uv_mode, tx_size_uv, frm_hdr->reduced_tx_set);
        if (svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) && cand->transform_type_uv != DCT_DCT) {
            continue;
        }
        cand->block_mi.ref_frame[0]       = INTRA_FRAME;
        cand->block_mi.ref_frame[1]       = NONE_FRAME;
        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
        cand->block_mi.is_interintra_used = 0;
        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
    }

    // update the total number of candidates injected
    (*candidate_total_cnt) = cand_total_cnt;

    return;
}

static void inject_zz_backup_candidate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                       uint32_t* candidate_total_cnt) {
    ModeDecisionCandidate* cand_array      = ctx->fast_cand_array;
    Mv                     best_pred_mv[2] = {{{0}}, {{0}}};
    uint32_t               cand_total_cnt  = (*candidate_total_cnt);
    cand_array[cand_total_cnt].drl_index   = 0;
    svt_aom_choose_best_av1_mv_pred(ctx,
                                    svt_get_ref_frame_type(REF_LIST_0, 0),
                                    NEWMV,
                                    (Mv){{0}},
                                    (Mv){{0}},
                                    &cand_array[cand_total_cnt].drl_index,
                                    best_pred_mv);
    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, (Mv){{0, 0}}, (Mv){{0, 0}}, 0)) {
        ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
        cand->block_mi.use_intrabc        = 0;
        cand->skip_mode_allowed           = false;
        cand->block_mi.mode               = NEWMV;
        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
        cand->block_mi.mv[0]              = (Mv){{0, 0}};
        cand->block_mi.ref_frame[0]       = svt_get_ref_frame_type(REF_LIST_0, 0);
        cand->block_mi.ref_frame[1]       = NONE_FRAME;
        cand->transform_type[0]           = DCT_DCT;
        cand->transform_type_uv           = DCT_DCT;
        cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
        cand->block_mi.is_interintra_used = 0;
        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
        cand->block_mi.num_proj_ref       = ctx->wm_sample_info[svt_get_ref_frame_type(REF_LIST_0, 0)].num;
        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
        // update the total number of candidates injected
        (*candidate_total_cnt) = cand_total_cnt;
    }
}

int svt_av1_allow_palette(int allow_palette, BlockSize bsize) {
    assert(bsize < BLOCK_SIZES_ALL);
    return allow_palette && block_size_wide[bsize] <= 64 && block_size_high[bsize] <= 64 && bsize >= BLOCK_8X8;
}

void search_palette_luma(PictureControlSet* pcs, ModeDecisionContext* ctx, PaletteInfo* palette_cand,
                         uint8_t* palette_size_array, uint32_t* tot_palette_cands);

static void inject_palette_candidates(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* candidate_total_cnt) {
    uint32_t               can_total_cnt      = *candidate_total_cnt;
    ModeDecisionCandidate* cand_array         = ctx->fast_cand_array;
    const TxSize           tx_size_uv         = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
    uint32_t               tot_palette_cands  = 0;
    PaletteInfo*           palette_cand_array = ctx->palette_cand_array;
    // MD palette search
    uint8_t* palette_size_array_0 = ctx->palette_size_array_0;

    search_palette_luma(pcs, ctx, palette_cand_array, palette_size_array_0, &tot_palette_cands);

    for (uint32_t cand_i = 0; cand_i < tot_palette_cands; ++cand_i) {
        ModeDecisionCandidate* cand       = &cand_array[can_total_cnt];
        cand->block_mi.is_interintra_used = 0;
        cand->palette_size[0]             = palette_size_array_0[cand_i];
        // Palette is not supported for chroma
        cand->palette_size[1] = 0;
        cand->palette_info    = &palette_cand_array[cand_i];
        assert(palette_size_array_0[cand_i] < 9);
        //to re check these fields
        cand->skip_mode_allowed    = false;
        cand->block_mi.mode        = DC_PRED;
        cand->block_mi.use_intrabc = 0;

        cand->block_mi.filter_intra_mode         = FILTER_INTRA_MODES;
        cand->block_mi.angle_delta[PLANE_TYPE_Y] = 0;
        // Palette is not supported for chroma mode, so we can set the intra chroma mode to anything. To use palette
        // for chroma, we must force DC_PRED to be used for the intra chroma mode
        assert(cand_array[can_total_cnt].palette_size[1] == 0);
        cand->block_mi.uv_mode = ctx->ind_uv_avail ? ctx->best_uv_mode[DC_PRED] : intra_luma_to_chroma[DC_PRED];
        cand->block_mi.angle_delta[PLANE_TYPE_UV] = ctx->ind_uv_avail ? ctx->best_uv_angle[DC_PRED]
                                                                      : cand->block_mi.angle_delta[PLANE_TYPE_Y];
        cand->block_mi.cfl_alpha_signs            = 0;
        cand->block_mi.cfl_alpha_idx              = 0;
        cand->transform_type[0]                   = DCT_DCT;
        cand->transform_type_uv                   = svt_aom_get_intra_uv_tx_type(
            cand->block_mi.uv_mode, tx_size_uv, pcs->ppcs->frm_hdr.reduced_tx_set);
        if (svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id) && cand->transform_type_uv != DCT_DCT) {
            continue;
        }
        cand->block_mi.ref_frame[0] = INTRA_FRAME;
        cand->block_mi.ref_frame[1] = NONE_FRAME;
        cand->block_mi.motion_mode  = SIMPLE_TRANSLATION;
        INC_MD_CAND_CNT(can_total_cnt, pcs->ppcs->max_can_count);
    }

    // update the total number of candidates injected
    (*candidate_total_cnt) = can_total_cnt;

    return;
}

static INLINE void eliminate_candidate_based_on_pme_me_results(ModeDecisionContext* ctx, uint8_t* dc_cand_only_flag) {
    if (ctx->md_pme_dist != (uint32_t)~0 || ctx->md_me_dist != (uint32_t)~0) {
        uint32_t th = ctx->cand_reduction_ctrls.cand_elimination_ctrls.dc_only_th;
        th *= ctx->blk_geom->bheight * ctx->blk_geom->bwidth;
        const uint32_t best_me_distotion = MIN(ctx->md_pme_dist, ctx->md_me_dist);
        if (best_me_distotion < th) {
            *dc_cand_only_flag = 1;
        }
    }
}

static bool valid_ref_frame_type(MvReferenceFrame rf[2], const MvReferenceFrame ref_frame_type_arr[],
                                 uint8_t tot_ref_frame_types) {
    // INTRA_FRAME is added in candidates sometimes, skip validation
    if (rf[0] == INTRA_FRAME) {
        return true;
    }

    for (uint8_t i = 0; i < tot_ref_frame_types; i++) {
        MvReferenceFrame rf_in_arr[2];
        av1_set_ref_frame(rf_in_arr, ref_frame_type_arr[i]);
        if (rf[0] == rf_in_arr[0] && rf[1] == rf_in_arr[1]) {
            return true;
        }
    }
    return false;
}

// refer to inject_zz_backup_candidate, but use BWD ref instead of LAST
static void inject_sframe_backup_candidate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                           uint32_t* candidate_total_cnt) {
    ModeDecisionCandidate* cand_array      = ctx->fast_cand_array;
    Mv                     best_pred_mv[2] = {{{0}}, {{0}}};
    uint32_t               cand_total_cnt  = (*candidate_total_cnt);
    cand_array[cand_total_cnt].drl_index   = 0;
    svt_aom_choose_best_av1_mv_pred(ctx,
                                    svt_get_ref_frame_type(REF_LIST_1, 0),
                                    NEWMV,
                                    (Mv){{0}},
                                    (Mv){{0}},
                                    &cand_array[cand_total_cnt].drl_index,
                                    best_pred_mv);
    if (!ctx->corrupted_mv_check || is_valid_mv_diff(best_pred_mv, (Mv){{0, 0}}, (Mv){{0, 0}}, 0)) {
        ModeDecisionCandidate* cand       = &cand_array[cand_total_cnt];
        cand->block_mi.use_intrabc        = 0;
        cand->skip_mode_allowed           = false;
        cand->block_mi.mode               = NEWMV;
        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
        cand->block_mi.mv[0]              = (Mv){{0, 0}};
        cand->block_mi.ref_frame[0]       = svt_get_ref_frame_type(REF_LIST_1, 0);
        cand->block_mi.ref_frame[1]       = NONE_FRAME;
        cand->transform_type[0]           = DCT_DCT;
        cand->transform_type_uv           = DCT_DCT;
        cand->pred_mv[0].as_int           = best_pred_mv[0].as_int;
        cand->block_mi.is_interintra_used = 0;
        cand->block_mi.motion_mode        = SIMPLE_TRANSLATION;
        cand->block_mi.num_proj_ref       = ctx->wm_sample_info[svt_get_ref_frame_type(REF_LIST_1, 0)].num;
        INC_MD_CAND_CNT(cand_total_cnt, pcs->ppcs->max_can_count);
        // update the total number of candidates injected
        (*candidate_total_cnt) = cand_total_cnt;
    }
}

// in MD stage 0, candidates are injected by different tools, but for S-Frame in RA mode
// the ref frame types in ref_list0 has be pruned in PD for the reversed direction of ref MVs
// here to check and reject the candidates if mismatches the available frame types array
static uint32_t reject_candidate_sframe(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t cand_total_cnt) {
    for (uint32_t i = 0; i < cand_total_cnt;) {
        if (!valid_ref_frame_type(
                ctx->fast_cand_array[i].block_mi.ref_frame, ctx->ref_frame_type_arr, ctx->tot_ref_frame_types)) {
            for (uint32_t j = i; j < cand_total_cnt; j++) {
                memcpy(&ctx->fast_cand_array[j], &ctx->fast_cand_array[j + 1], sizeof(ModeDecisionCandidate));
            }
            cand_total_cnt--;
            continue;
        }
        i++;
    }
    // zero candidate in fast cand array risks in md stage 0, add a candidate from ref list1 as backup
    if (cand_total_cnt == 0) {
        inject_sframe_backup_candidate(pcs, ctx, &cand_total_cnt);
    }
    assert(cand_total_cnt > 0);
    return cand_total_cnt;
}

EbErrorType generate_md_stage_0_cand_light_pd0(ModeDecisionContext* ctx, uint32_t* candidate_total_count_ptr,
                                               PictureControlSet* pcs) {
    const SliceType slice_type     = pcs->slice_type;
    uint32_t        cand_total_cnt = 0;
    //----------------------
    // Intra
    if (ctx->blk_geom->sq_size < 128 && ctx->intra_ctrls.enable_intra) {
        inject_intra_candidates_light_pd0(pcs, ctx, &cand_total_cnt);
    }

    if (slice_type != I_SLICE) {
        inject_inter_candidates_light_pd0(pcs, ctx, &cand_total_cnt);
    }

    // For I_SLICE, DC is always injected, and therefore there is no a risk of no candidates @ md_stage_0()
    // For non I_SLICE, there is a risk of no candidates @ md_stage_0() because of the INTER candidates pruning techniques
    if (slice_type != I_SLICE && cand_total_cnt == 0) {
        inject_zz_backup_candidate(pcs, ctx, &cand_total_cnt);
    }

    if (pcs->ppcs->sframe_ref_pruned) {
        cand_total_cnt = reject_candidate_sframe(pcs, ctx, cand_total_cnt);
    }

    *candidate_total_count_ptr = cand_total_cnt;

    return EB_ErrorNone;
}

/*
   generate candidates for light pd1
*/
void generate_md_stage_0_cand_light_pd1(ModeDecisionContext* ctx, uint32_t* candidate_total_count_ptr,
                                        PictureControlSet* pcs) {
    const SliceType slice_type     = pcs->slice_type;
    uint32_t        cand_total_cnt = 0;
    // Reset duplicates variables
    ctx->injected_mv_count = 0;
    ctx->inject_new_me     = 1;
    if (slice_type != I_SLICE) {
        inject_inter_candidates_light_pd1(pcs, ctx, &cand_total_cnt);
    }
    //----------------------
    // Intra
    if (ctx->intra_ctrls.enable_intra && ctx->blk_geom->sq_size < 128) {
#if OPT_PER_BLK_INTRA // max
        uint8_t dc_cand_only_flag = ctx->intra_ctrls.intra_mode_end == DC_PRED || is_dc_only_safe(pcs, ctx);
#else
        uint8_t dc_cand_only_flag = (ctx->intra_ctrls.intra_mode_end == DC_PRED);
#endif
        if (ctx->cand_reduction_ctrls.cand_elimination_ctrls.enabled && !dc_cand_only_flag &&
            ctx->md_me_dist != (uint32_t)~0) {
            uint32_t th = ctx->cand_reduction_ctrls.cand_elimination_ctrls.dc_only_th;
            th *= (ctx->blk_geom->bheight * ctx->blk_geom->bwidth);
            if (ctx->md_me_dist < th) {
                dc_cand_only_flag = 1;
            }
        }
        inject_intra_candidates(pcs, ctx, dc_cand_only_flag, &cand_total_cnt);
    }

    // For I_SLICE, DC is always injected, and therefore there is no a risk of no candidates @ md_syage_0()
    // For non I_SLICE, there is a risk of no candidates @ md_stage_0() because of the INTER candidates pruning techniques
    if (slice_type != I_SLICE && cand_total_cnt == 0) {
        inject_zz_backup_candidate(pcs, ctx, &cand_total_cnt);
    }

    if (pcs->ppcs->sframe_ref_pruned) {
        cand_total_cnt = reject_candidate_sframe(pcs, ctx, cand_total_cnt);
    }

    *candidate_total_count_ptr = cand_total_cnt;
}

EbErrorType generate_md_stage_0_cand(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                     uint32_t* candidate_total_count_ptr) {
    const SequenceControlSet* scs            = pcs->scs;
    const SliceType           slice_type     = pcs->slice_type;
    uint32_t                  cand_total_cnt = 0;
    // Reset duplicates variables
    ctx->injected_mv_count = 0;
    ctx->inject_new_me     = 1;
    ctx->inject_new_pme    = 1;
#if !OPT_PER_BLK_INTRA // max
    uint8_t dc_cand_only_flag = ctx->intra_ctrls.enable_intra && (ctx->intra_ctrls.intra_mode_end == DC_PRED);
    if (ctx->cand_reduction_ctrls.cand_elimination_ctrls.enabled) {
        eliminate_candidate_based_on_pme_me_results(ctx, &dc_cand_only_flag);
    }
#endif
    //----------------------
    // Intra
    if (ctx->intra_ctrls.enable_intra) {
#if OPT_PER_BLK_INTRA
        uint8_t dc_cand_only_flag = ctx->intra_ctrls.intra_mode_end == DC_PRED || is_dc_only_safe(pcs, ctx);
        if (ctx->cand_reduction_ctrls.cand_elimination_ctrls.enabled) {
            eliminate_candidate_based_on_pme_me_results(ctx, &dc_cand_only_flag);
        }
#endif
        if (ctx->blk_geom->sq_size < 128) {
            inject_intra_candidates(pcs, ctx, dc_cand_only_flag, &cand_total_cnt);
        }
        if (ctx->filter_intra_ctrls.enabled && svt_aom_filter_intra_allowed_bsize(ctx->blk_geom->bsize)) {
            inject_filter_intra_candidates(pcs, ctx, &cand_total_cnt);
        }

        if (ctx->md_allow_intrabc) {
            inject_intra_bc_candidates(pcs, ctx, scs, ctx->blk_ptr, &cand_total_cnt);
        }

        if (svt_av1_allow_palette(ctx->md_palette_level, ctx->blk_geom->bsize)) {
            inject_palette_candidates(pcs, ctx, &cand_total_cnt);
        }
    }
    if (slice_type != I_SLICE) {
        svt_aom_inject_inter_candidates(pcs, ctx, &cand_total_cnt);
    }
    // For I_SLICE, DC is always injected, and therefore there is no a risk of no candidates @ md_syage_0()
    // For non I_SLICE, there is a risk of no candidates @ md_stage_0() because of the INTER candidates pruning techniques
    if (slice_type != I_SLICE && cand_total_cnt == 0) {
        inject_zz_backup_candidate(pcs, ctx, &cand_total_cnt);
    }

    if (pcs->ppcs->sframe_ref_pruned) {
        cand_total_cnt = reject_candidate_sframe(pcs, ctx, cand_total_cnt);
    }

    *candidate_total_count_ptr = cand_total_cnt;

    memset(ctx->md_stage_0_count, 0, CAND_CLASS_TOTAL * sizeof(uint32_t));
    bool merge_inter_cands = 0;
    if (ctx->nic_ctrls.pruning_ctrls.merge_inter_cands_mult != (uint8_t)~0) {
        uint16_t th = (ctx->nic_ctrls.pruning_ctrls.merge_inter_cands_mult * (63 - pcs->scs->static_config.qp)) >> 1;
        if ((MIN(ctx->md_me_dist, ctx->md_pme_dist) / (ctx->blk_geom->bwidth * ctx->blk_geom->bheight)) < th) {
            merge_inter_cands = 1;
        }
    }

    for (uint32_t cand_i = 0; cand_i < cand_total_cnt; cand_i++) {
        ModeDecisionCandidate* cand = &ctx->fast_cand_array[cand_i];
        if (is_intra_mode(cand->block_mi.mode)) {
            // Intra prediction
            if (cand->palette_info == NULL || cand->palette_size[0] == 0) {
                cand->cand_class = CAND_CLASS_0;
                ctx->md_stage_0_count[CAND_CLASS_0]++;
            } else {
                // Palette Prediction
                cand->cand_class = CAND_CLASS_3;
                ctx->md_stage_0_count[CAND_CLASS_3]++;
            }
        } else { // INTER
            if (cand->block_mi.mode == NEWMV || cand->block_mi.mode == NEW_NEWMV || merge_inter_cands) {
                // MV Prediction
                cand->cand_class = CAND_CLASS_2;
                ctx->md_stage_0_count[CAND_CLASS_2]++;
            } else {
                //MVP Prediction
                cand->cand_class = CAND_CLASS_1;
                ctx->md_stage_0_count[CAND_CLASS_1]++;
            }
        }
    }
    return EB_ErrorNone;
}

uint8_t av1_drl_ctx(const CandidateMv* ref_mv_stack, int32_t ref_idx);

/***************************************
* Update symbols for light-PD1 path
***************************************/
void svt_aom_product_full_mode_decision_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                                  ModeDecisionCandidateBuffer* cand_bf) {
    BlkStruct*             blk_ptr = ctx->blk_ptr;
    ModeDecisionCandidate* cand    = cand_bf->cand;
    blk_ptr->total_rate            = cand_bf->total_rate;

    // Set common signals (INTER/INTRA)
    svt_memcpy(&blk_ptr->block_mi, &cand->block_mi, sizeof(BlockModeInfo));
    blk_ptr->palette_size[0] = blk_ptr->palette_size[1] = 0;

    // Set INTER mode signals
    if (is_inter_mode(cand->block_mi.mode)) {
        blk_ptr->drl_index = cand->drl_index;
        assert(IMPLIES(
            is_inter_compound_mode(cand->block_mi.mode) && blk_ptr->block_mi.interinter_comp.type == COMPOUND_AVERAGE,
            (blk_ptr->block_mi.comp_group_idx == 0 && blk_ptr->block_mi.compound_idx == 1)));

        // Set MVs
        blk_ptr->predmv[0].as_int = cand->pred_mv[0].as_int;
        if (has_second_ref(&blk_ptr->block_mi)) {
            blk_ptr->predmv[1].as_int = cand->pred_mv[1].as_int;
        }

        const int8_t ref_frame_type = av1_ref_frame_type(blk_ptr->block_mi.ref_frame);
        // Store winning inter_mode_ctx in blk to avoid storing for all ref frames for EC
        blk_ptr->inter_mode_ctx = ctx->inter_mode_ctx[ref_frame_type];
        // Store drl_ctx in blk to avoid storing final_ref_mv_stack for EC
        if (blk_ptr->block_mi.mode == NEWMV || blk_ptr->block_mi.mode == NEW_NEWMV) {
            for (uint8_t idx = 0; idx < 2; ++idx) {
                if (blk_ptr->av1xd->ref_mv_count[ref_frame_type] > idx + 1) {
                    blk_ptr->drl_ctx[idx] = av1_drl_ctx(ctx->ref_mv_stack[ref_frame_type], idx);
                } else {
                    blk_ptr->drl_ctx[idx] = -1;
                }
            }
        }

        if (have_nearmv_in_inter_mode(blk_ptr->block_mi.mode)) {
            // TODO(jingning): Temporary solution to compensate the NEARESTMV offset.
            for (uint8_t idx = 1; idx < 3; ++idx) {
                if (blk_ptr->av1xd->ref_mv_count[ref_frame_type] > idx + 1) {
                    blk_ptr->drl_ctx_near[idx - 1] = av1_drl_ctx(ctx->ref_mv_stack[ref_frame_type], idx);
                } else {
                    blk_ptr->drl_ctx_near[idx - 1] = -1;
                }
            }
        }
    } else { // Set INTRA mode signals
        cand->skip_mode_allowed = false;
    }
    // Set TX and coeff-related data
    blk_ptr->block_has_coeff   = ((cand_bf->block_has_coeff) > 0) ? true : false;
    ctx->blk_ptr->cnt_nz_coeff = cand_bf->cnt_nz_coeff;

    // If skip_mode is allowed, and block has no coeffs, use skip_mode
    if (cand->skip_mode_allowed == true) {
        blk_ptr->block_mi.skip_mode |= !blk_ptr->block_has_coeff;
    }

    assert(IMPLIES(pcs->ppcs->frm_hdr.interpolation_filter == SWITCHABLE && blk_ptr->block_mi.skip_mode,
                   cand->block_mi.interp_filters == 0));
    if (blk_ptr->block_mi.skip_mode) {
        blk_ptr->block_has_coeff = 0;
        cand_bf->y_has_coeff     = 0;
        cand_bf->u_has_coeff     = 0;
        cand_bf->v_has_coeff     = 0;
    }
    blk_ptr->block_mi.skip = !blk_ptr->block_has_coeff;

    const uint16_t txb_itr       = 0;
    const int32_t  txb_1d_offset = 0, txb_1d_offset_uv = 0;
    blk_ptr->y_has_coeff         = cand_bf->y_has_coeff;
    blk_ptr->u_has_coeff         = cand_bf->u_has_coeff;
    blk_ptr->v_has_coeff         = cand_bf->v_has_coeff;
    blk_ptr->tx_type[txb_itr]    = cand->transform_type[txb_itr];
    blk_ptr->tx_type_uv          = cand->transform_type_uv;
    blk_ptr->quant_dc.y[txb_itr] = cand_bf->quant_dc.y[txb_itr];
    blk_ptr->quant_dc.u[txb_itr] = cand_bf->quant_dc.u[txb_itr];
    blk_ptr->quant_dc.v[txb_itr] = cand_bf->quant_dc.v[txb_itr];

    if (ctx->bypass_encdec) {
        blk_ptr->eob.y[txb_itr] = cand_bf->eob.y[txb_itr];
        blk_ptr->eob.u[txb_itr] = cand_bf->eob.u[txb_itr];
        blk_ptr->eob.v[txb_itr] = cand_bf->eob.v[txb_itr];
        int32_t* src_ptr;
        int32_t* dst_ptr;

        const TxSize tx_size   = tx_depth_to_tx_size[blk_ptr->block_mi.tx_depth][ctx->blk_geom->bsize];
        const int    tx_width  = tx_size_wide[tx_size];
        const int    tx_height = tx_size_high[tx_size];

        // only one TX unit, so no need to bitmask
        if (blk_ptr->y_has_coeff) {
            src_ptr = &(((int32_t*)cand_bf->quant->buffer_y)[txb_1d_offset]);
            dst_ptr = ((int32_t*)pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index]->buffer_y) + ctx->coded_area_sb;
            svt_memcpy(dst_ptr, src_ptr, tx_width * tx_height * sizeof(int32_t));
        }
        ctx->coded_area_sb += tx_width * tx_height;

        const TxSize tx_size_uv   = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
        const int    tx_width_uv  = tx_size_wide[tx_size_uv];
        const int    tx_height_uv = tx_size_high[tx_size_uv];
        // Cb
        // only one TX unit, so no need to bitmask
        if (blk_ptr->u_has_coeff) {
            src_ptr = &(((int32_t*)cand_bf->quant->buffer_cb)[txb_1d_offset_uv]);
            dst_ptr = ((int32_t*)pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index]->buffer_cb) +
                ctx->coded_area_sb_uv;
            svt_memcpy(dst_ptr, src_ptr, tx_width_uv * tx_height_uv * sizeof(int32_t));
        }

        // Cr
        // only one TX unit, so no need to bitmask
        if (blk_ptr->v_has_coeff) {
            src_ptr = &(((int32_t*)cand_bf->quant->buffer_cr)[txb_1d_offset_uv]);
            dst_ptr = ((int32_t*)pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index]->buffer_cr) +
                ctx->coded_area_sb_uv;
            svt_memcpy(dst_ptr, src_ptr, tx_width_uv * tx_height_uv * sizeof(int32_t));
        }
        ctx->coded_area_sb_uv += tx_width_uv * tx_height_uv;
    }
}

static INLINE double derive_ssim_threshold_factor_for_full_md(SequenceControlSet* scs) {
    return scs->input_resolution >= INPUT_SIZE_1080p_RANGE ? 1.02 : 1.03;
}

/***************************************
* Full Mode Decision
***************************************/
uint32_t svt_aom_product_full_mode_decision(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                            ModeDecisionCandidateBuffer** buffer_ptr_array,
                                            uint32_t candidate_total_count, uint32_t* best_candidate_index_array) {
    SequenceControlSet* scs                = pcs->scs;
    BlkStruct*          blk_ptr            = ctx->blk_ptr;
    uint32_t            lowest_cost_index  = best_candidate_index_array[0];
    const bool          use_ssim_full_cost = ctx->tune_ssim_level > SSIM_LVL_0 ? true : false;

    // Find the candidate with the lowest cost
    // Only need to sort if have multiple candidates
    if (ctx->md_stage_3_total_count > 1) {
        if (use_ssim_full_cost) {
            // Pass one: find candidate with the lowest SSD cost
            uint64_t ssd_lowest_cost = 0xFFFFFFFFFFFFFFFFull;
            for (uint32_t i = 0; i < candidate_total_count; ++i) {
                uint32_t cand_index = best_candidate_index_array[i];
                uint64_t cost       = *(buffer_ptr_array[cand_index]->full_cost);
                if (cost < ssd_lowest_cost) {
                    lowest_cost_index = cand_index;
                    ssd_lowest_cost   = cost;
                }
            }

            // Pass two: among the candidates with SSD cost not greater than the threshold, find the one with the lowest SSIM cost
            const double   threshold_factor   = derive_ssim_threshold_factor_for_full_md(scs);
            const uint64_t ssd_cost_threshold = (uint64_t)(threshold_factor * ssd_lowest_cost);
            uint64_t       ssim_lowest_cost   = 0xFFFFFFFFFFFFFFFFull;
            for (uint32_t i = 0; i < candidate_total_count; ++i) {
                uint32_t cand_index = best_candidate_index_array[i];

                uint64_t ssim_cost = *(buffer_ptr_array[cand_index]->full_cost_ssim);
                uint64_t ssd_cost  = *(buffer_ptr_array[cand_index]->full_cost);
                if (ssim_cost < ssim_lowest_cost) {
                    if (ssd_cost <= ssd_cost_threshold) {
                        lowest_cost_index = cand_index;
                        ssim_lowest_cost  = ssim_cost;
                        ssd_lowest_cost   = ssd_cost;
                    }
                } else if (ssim_cost == ssim_lowest_cost) {
                    // if two candidates have the same ssim cost, choose the one with lower ssd cost
                    if (ssd_cost < ssd_lowest_cost) {
                        lowest_cost_index = cand_index;
                        ssim_lowest_cost  = ssim_cost;
                        ssd_lowest_cost   = ssd_cost;
                    }
                }
            }
        } else { // fallback to SSD based RD cost
            uint64_t lowest_cost = 0xFFFFFFFFFFFFFFFFull;
            for (uint32_t i = 0; i < candidate_total_count; ++i) {
                uint32_t cand_index = best_candidate_index_array[i];

                uint64_t cost = *(buffer_ptr_array[cand_index]->full_cost);
                if (scs->vq_ctrls.sharpness_ctrls.unipred_bias && pcs->ppcs->is_noise_level &&
                    is_inter_singleref_mode(buffer_ptr_array[cand_index]->cand->block_mi.mode)) {
                    cost = (cost * uni_psy_bias[pcs->ppcs->picture_qp]) / 100;
                }

                if (cost < lowest_cost) {
                    lowest_cost_index = cand_index;
                    lowest_cost       = cost;
                }
            }
        }
    }
    ModeDecisionCandidateBuffer* cand_bf = buffer_ptr_array[lowest_cost_index];
    ModeDecisionCandidate*       cand    = cand_bf->cand;
    blk_ptr->total_rate                  = cand_bf->total_rate;
    if (!(ctx->pd_pass == PD_PASS_1 && ctx->fixed_partition)) {
        // When lambda tuning is on, lambda of each block is set separately, however at interdepth decision the sb lambda is used
        uint32_t full_lambda = ctx->hbd_md ? ctx->full_sb_lambda_md[EB_10_BIT_MD] : ctx->full_sb_lambda_md[EB_8_BIT_MD];
        ctx->blk_ptr->cost   = RDCOST(full_lambda, cand_bf->total_rate, cand_bf->full_dist);
        ctx->blk_ptr->full_dist = cand_bf->full_dist;
    }

    // Set common signals (INTER/INTRA)
    svt_memcpy(&blk_ptr->block_mi, &cand->block_mi, sizeof(BlockModeInfo));
    // Set INTER mode signals
    // INTER signals set first b/c INTER shuts Palette, so INTRA must overwrite if Palette + intrabc is used
    if (is_inter_block(&blk_ptr->block_mi)) {
        blk_ptr->drl_index = cand->drl_index;
        assert(IMPLIES(
            is_inter_compound_mode(cand->block_mi.mode) && blk_ptr->block_mi.interinter_comp.type == COMPOUND_AVERAGE,
            (blk_ptr->block_mi.comp_group_idx == 0 && blk_ptr->block_mi.compound_idx == 1)));

        blk_ptr->palette_size[0] = blk_ptr->palette_size[1] = 0;
        // Set MVs
        blk_ptr->predmv[0].as_int = cand->pred_mv[0].as_int;
        if (has_second_ref(&blk_ptr->block_mi)) {
            blk_ptr->predmv[1].as_int = cand->pred_mv[1].as_int;
        }
        if (blk_ptr->block_mi.motion_mode == WARPED_CAUSAL ||
            (cand->block_mi.mode == GLOBALMV || cand->block_mi.mode == GLOBAL_GLOBALMV)) {
            svt_memcpy(&ctx->blk_ptr->wm_params_l0, &cand->wm_params_l0, sizeof(WarpedMotionParams));
            svt_memcpy(&ctx->blk_ptr->wm_params_l1, &cand->wm_params_l1, sizeof(WarpedMotionParams));
        }

        if (ctx->pd_pass == PD_PASS_1) {
            const int8_t ref_frame_type = av1_ref_frame_type(blk_ptr->block_mi.ref_frame);
            // Store winning inter_mode_ctx in blk to avoid storing for all ref frames for EC
            blk_ptr->inter_mode_ctx = ctx->inter_mode_ctx[ref_frame_type];
            // Store drl_ctx in blk to avoid storing final_ref_mv_stack for EC
            if (blk_ptr->block_mi.mode == NEWMV || blk_ptr->block_mi.mode == NEW_NEWMV) {
                for (uint8_t idx = 0; idx < 2; ++idx) {
                    if (blk_ptr->av1xd->ref_mv_count[ref_frame_type] > idx + 1) {
                        blk_ptr->drl_ctx[idx] = av1_drl_ctx(ctx->ref_mv_stack[ref_frame_type], idx);
                    } else {
                        blk_ptr->drl_ctx[idx] = -1;
                    }
                }
            }

            if (have_nearmv_in_inter_mode(blk_ptr->block_mi.mode)) {
                // TODO(jingning): Temporary solution to compensate the NEARESTMV offset.
                for (uint8_t idx = 1; idx < 3; ++idx) {
                    if (blk_ptr->av1xd->ref_mv_count[ref_frame_type] > idx + 1) {
                        blk_ptr->drl_ctx_near[idx - 1] = av1_drl_ctx(ctx->ref_mv_stack[ref_frame_type], idx);
                    } else {
                        blk_ptr->drl_ctx_near[idx - 1] = -1;
                    }
                }
            }
        }
    }

    // Set INTRA mode signals
    if (is_intra_mode(blk_ptr->block_mi.mode)) {
        if (!cand->palette_info) {
            blk_ptr->palette_size[0] = blk_ptr->palette_size[1] = 0;
        } else if (svt_av1_allow_palette(ctx->md_palette_level, ctx->blk_geom->bsize)) {
            memcpy(&blk_ptr->palette_info->pmi, &cand->palette_info->pmi, sizeof(PaletteModeInfo));
            memcpy(blk_ptr->palette_info->color_idx_map, cand->palette_info->color_idx_map, MAX_PALETTE_SQUARE);
            blk_ptr->palette_size[0] = cand->palette_size[0];
            blk_ptr->palette_size[1] = cand->palette_size[1];
        }

        if (blk_ptr->block_mi.use_intrabc == 0) {
            cand->skip_mode_allowed = false;
        }
    }

    // Set TX and coeff-related data
    blk_ptr->block_has_coeff   = ((cand_bf->block_has_coeff) > 0) ? true : false;
    ctx->blk_ptr->cnt_nz_coeff = cand_bf->cnt_nz_coeff;

    // If skip_mode is allowed, and block has no coeffs, use skip_mode
    if (cand->skip_mode_allowed == true) {
        blk_ptr->block_mi.skip_mode |= !blk_ptr->block_has_coeff;
    }

    assert(IMPLIES(pcs->ppcs->frm_hdr.interpolation_filter == SWITCHABLE && blk_ptr->block_mi.skip_mode,
                   cand->block_mi.interp_filters == 0));
    if (blk_ptr->block_mi.skip_mode) {
        blk_ptr->block_has_coeff = 0;
        cand_bf->y_has_coeff     = 0;
        cand_bf->u_has_coeff     = 0;
        cand_bf->v_has_coeff     = 0;
    }

    blk_ptr->block_mi.skip = !blk_ptr->block_has_coeff;
    blk_ptr->y_has_coeff   = cand_bf->y_has_coeff;
    blk_ptr->u_has_coeff   = cand_bf->u_has_coeff;
    blk_ptr->v_has_coeff   = cand_bf->v_has_coeff;
    svt_memcpy(blk_ptr->tx_type, cand->transform_type, sizeof(TxType) * MAX_TXB_COUNT);
    blk_ptr->tx_type_uv = cand->transform_type_uv;
    svt_memcpy(&blk_ptr->quant_dc, &cand_bf->quant_dc, sizeof(QuantDcData));
    svt_memcpy(&blk_ptr->eob, &cand_bf->eob, sizeof(EobData));

    // If bypassing EncDec, save recon/coeff
    if (ctx->bypass_encdec && ctx->pd_pass == PD_PASS_1) {
        const uint16_t tu_total_count = tx_blocks_per_depth[ctx->blk_geom->bsize][blk_ptr->block_mi.tx_depth];
        int32_t        txb_1d_offset = 0, txb_1d_offset_uv = 0;
        const TxSize   tx_size      = tx_depth_to_tx_size[blk_ptr->block_mi.tx_depth][ctx->blk_geom->bsize];
        const int      tx_width     = tx_size_wide[tx_size];
        const int      tx_height    = tx_size_high[tx_size];
        const TxSize   tx_size_uv   = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
        const int      tx_width_uv  = tx_size_wide[tx_size_uv];
        const int      tx_height_uv = tx_size_high[tx_size_uv];
        for (uint16_t txb_itr = 0; txb_itr < tu_total_count; txb_itr++) {
            const bool uv_pass = (blk_ptr->block_mi.tx_depth == 0 || txb_itr == 0);

            int32_t* src_ptr = &(((int32_t*)cand_bf->quant->buffer_y)[txb_1d_offset]);
            int32_t* dst_ptr = &(((int32_t*)ctx->blk_ptr->coeff_tmp->buffer_y)[txb_1d_offset]);

            if (ctx->fixed_partition) {
                dst_ptr = ((int32_t*)pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index]->buffer_y) +
                    ctx->coded_area_sb;
                ctx->coded_area_sb += tx_width * tx_height;
            }

            if (blk_ptr->y_has_coeff & (1 << txb_itr)) {
                svt_memcpy(dst_ptr, src_ptr, tx_width * tx_height * sizeof(int32_t));
            }

            txb_1d_offset += tx_width * tx_height;

            if (ctx->has_uv && uv_pass) {
                // Cb
                src_ptr = &(((int32_t*)cand_bf->quant->buffer_cb)[txb_1d_offset_uv]);
                dst_ptr = &(((int32_t*)ctx->blk_ptr->coeff_tmp->buffer_cb)[txb_1d_offset_uv]);

                if (ctx->fixed_partition) {
                    dst_ptr = ((int32_t*)pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index]->buffer_cb) +
                        ctx->coded_area_sb_uv;
                }

                if (blk_ptr->u_has_coeff & (1 << txb_itr)) {
                    svt_memcpy(dst_ptr, src_ptr, tx_width_uv * tx_height_uv * sizeof(int32_t));
                }

                // Cr
                src_ptr = &(((int32_t*)cand_bf->quant->buffer_cr)[txb_1d_offset_uv]);
                dst_ptr = &(((int32_t*)ctx->blk_ptr->coeff_tmp->buffer_cr)[txb_1d_offset_uv]);

                if (ctx->fixed_partition) {
                    dst_ptr = ((int32_t*)pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index]->buffer_cr) +
                        ctx->coded_area_sb_uv;
                    ctx->coded_area_sb_uv += tx_width_uv * tx_height_uv;
                }

                if (blk_ptr->v_has_coeff & (1 << txb_itr)) {
                    svt_memcpy(dst_ptr, src_ptr, tx_width_uv * tx_height_uv * sizeof(int32_t));
                }

                txb_1d_offset_uv += tx_width_uv * tx_height_uv;
            }
        }
    }

    return lowest_cost_index;
}

// Return the end column for the current superblock, in unit of TPL blocks.
static int get_superblock_tpl_column_end(PictureParentControlSet* ppcs, int mi_col, int num_mi_w) {
    const int mib_size_log2 = ppcs->scs->seq_header.sb_size == BLOCK_128X128 ? 5 : 4;
    // Find the start column of this superblock.
    const int sb_mi_col_start = (mi_col >> mib_size_log2) << mib_size_log2;
    // Same but in superres upscaled dimension.
    const int sb_mi_col_start_sr = coded_to_superres_mi(sb_mi_col_start, ppcs->superres_denom);
    // Width of this superblock in mi units.
    const int sb_mi_width = mi_size_wide[ppcs->scs->seq_header.sb_size];
    // Same but in superres upscaled dimension.
    const int sb_mi_width_sr = coded_to_superres_mi(sb_mi_width, ppcs->superres_denom);
    // Superblock end in mi units.
    const int sb_mi_end = sb_mi_col_start_sr + sb_mi_width_sr;
    // Superblock end in TPL units.
    return (sb_mi_end + num_mi_w - 1) / num_mi_w;
}

void aom_av1_set_ssim_rdmult(ModeDecisionContext* ctx, PictureControlSet* pcs, const int mi_row, const int mi_col) {
    const Av1Common* const cm    = pcs->ppcs->av1_cm;
    BlockSize              bsize = ctx->blk_geom->bsize;

    const int bsize_base = BLOCK_16X16;
    const int num_mi_w   = mi_size_wide[bsize_base];
    const int num_mi_h   = mi_size_high[bsize_base];
    const int num_cols   = (cm->mi_cols + num_mi_w - 1) / num_mi_w;
    const int num_rows   = (cm->mi_rows + num_mi_h - 1) / num_mi_h;
    const int num_bcols  = (mi_size_wide[bsize] + num_mi_w - 1) / num_mi_w;
    const int num_brows  = (mi_size_high[bsize] + num_mi_h - 1) / num_mi_h;
    int       row, col;
    double    num_of_mi          = 0.0;
    double    geom_mean_of_scale = 1.0;
    for (row = mi_row / num_mi_w; row < num_rows && row < mi_row / num_mi_w + num_brows; ++row) {
        for (col = mi_col / num_mi_h; col < num_cols && col < mi_col / num_mi_h + num_bcols; ++col) {
            const int index = row * num_cols + col;
            geom_mean_of_scale *= pcs->ppcs->pa_me_data->ssim_rdmult_scaling_factors[index];
            num_of_mi += 1.0;
        }
    }
    geom_mean_of_scale = pow(geom_mean_of_scale, (1.0 / num_of_mi));
    if (!pcs->ppcs->blk_lambda_tuning) {
        ctx->full_lambda_md[EB_8_BIT_MD] =
            (uint32_t)((double)ctx->ed_ctx->pic_full_lambda[EB_8_BIT_MD] * geom_mean_of_scale + 0.5);
        ctx->full_lambda_md[EB_10_BIT_MD] =
            (uint32_t)((double)ctx->ed_ctx->pic_full_lambda[EB_10_BIT_MD] * geom_mean_of_scale + 0.5);

        ctx->fast_lambda_md[EB_8_BIT_MD] =
            (uint32_t)((double)ctx->ed_ctx->pic_fast_lambda[EB_8_BIT_MD] * geom_mean_of_scale + 0.5);
        ctx->fast_lambda_md[EB_10_BIT_MD] =
            (uint32_t)((double)ctx->ed_ctx->pic_fast_lambda[EB_10_BIT_MD] * geom_mean_of_scale + 0.5);
    } else {
        ctx->full_lambda_md[EB_8_BIT_MD]  = (uint32_t)((double)ctx->full_lambda_md[EB_8_BIT_MD] * geom_mean_of_scale +
                                                      0.5);
        ctx->full_lambda_md[EB_10_BIT_MD] = (uint32_t)((double)ctx->full_lambda_md[EB_10_BIT_MD] * geom_mean_of_scale +
                                                       0.5);

        ctx->fast_lambda_md[EB_8_BIT_MD]  = (uint32_t)((double)ctx->fast_lambda_md[EB_8_BIT_MD] * geom_mean_of_scale +
                                                      0.5);
        ctx->fast_lambda_md[EB_10_BIT_MD] = (uint32_t)((double)ctx->fast_lambda_md[EB_10_BIT_MD] * geom_mean_of_scale +
                                                       0.5);
    }
}

void svt_aom_set_tuned_blk_lambda(ModeDecisionContext* ctx, PictureControlSet* pcs) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    Av1Common*               cm   = ppcs->av1_cm;

    BlockSize bsize  = ctx->blk_geom->bsize;
    int       mi_row = ctx->blk_org_y / 4;
    int       mi_col = ctx->blk_org_x / 4;

    const int mi_col_sr         = coded_to_superres_mi(mi_col, ppcs->superres_denom);
    const int mi_cols_sr        = ((ppcs->enhanced_unscaled_pic->width + 15) / 16) << 2; // picture column boundary
    const int block_mi_width_sr = coded_to_superres_mi(mi_size_wide[bsize], ppcs->superres_denom);
    const int bsize_base        = ppcs->tpl_ctrls.synth_blk_size == 32 ? BLOCK_32X32 : BLOCK_16X16;
    const int num_mi_w          = mi_size_wide[bsize_base];
    const int num_mi_h          = mi_size_high[bsize_base];
    const int num_cols          = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    const int num_rows          = (cm->mi_rows + num_mi_h - 1) / num_mi_h;
    const int num_bcols         = (block_mi_width_sr + num_mi_w - 1) / num_mi_w;
    const int num_brows         = (mi_size_high[bsize] + num_mi_h - 1) / num_mi_h;

    // This is required because the end col of superblock may be off by 1 in case
    // of superres.
    const int sb_bcol_end = get_superblock_tpl_column_end(ppcs, mi_col, num_mi_w);
    int       row, col;
    int32_t   base_block_count   = 0;
    double    geom_mean_of_scale = 0.0;
    for (row = mi_row / num_mi_w; row < num_rows && row < mi_row / num_mi_w + num_brows; ++row) {
        for (col = mi_col_sr / num_mi_h; col < num_cols && col < mi_col_sr / num_mi_h + num_bcols && col < sb_bcol_end;
             ++col) {
            const int index = row * num_cols + col;
            geom_mean_of_scale += log(ppcs->pa_me_data->tpl_sb_rdmult_scaling_factors[index]);
            ++base_block_count;
        }
    }
    // When superres is on, base_block_count could be zero.
    // This function's counterpart in AOM, av1_get_hier_tpl_rdmult, will encounter division by zero
    if (base_block_count == 0) {
        // return a large number to indicate invalid state
        ctx->full_lambda_md[EB_8_BIT_MD]  = SUPERRES_INVALID_STATE;
        ctx->full_lambda_md[EB_10_BIT_MD] = SUPERRES_INVALID_STATE;

        ctx->fast_lambda_md[EB_8_BIT_MD]  = SUPERRES_INVALID_STATE;
        ctx->fast_lambda_md[EB_10_BIT_MD] = SUPERRES_INVALID_STATE;
        return;
    }

    geom_mean_of_scale = exp(geom_mean_of_scale / base_block_count);

    ctx->full_lambda_md[EB_8_BIT_MD] =
        (uint32_t)((double)ctx->ed_ctx->pic_full_lambda[EB_8_BIT_MD] * geom_mean_of_scale + 0.5);
    ctx->full_lambda_md[EB_10_BIT_MD] =
        (uint32_t)((double)ctx->ed_ctx->pic_full_lambda[EB_10_BIT_MD] * geom_mean_of_scale + 0.5);

    ctx->fast_lambda_md[EB_8_BIT_MD] =
        (uint32_t)((double)ctx->ed_ctx->pic_fast_lambda[EB_8_BIT_MD] * geom_mean_of_scale + 0.5);
    ctx->fast_lambda_md[EB_10_BIT_MD] =
        (uint32_t)((double)ctx->ed_ctx->pic_fast_lambda[EB_10_BIT_MD] * geom_mean_of_scale + 0.5);
    if (ppcs->scs->static_config.tune == TUNE_SSIM || ppcs->scs->static_config.tune == TUNE_IQ ||
        ppcs->scs->static_config.tune == TUNE_MS_SSIM) {
        aom_av1_set_ssim_rdmult(ctx, pcs, mi_row, mi_col);
    }
}

double similarity(uint32_t sum_s, uint32_t sum_r, uint32_t sum_sq_s, uint32_t sum_sq_r, uint32_t sum_sxr, int count,
                  uint32_t bd);

double svt_ssim_4x4_c(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp) {
    const int32_t count = 4 * 4;

    uint32_t sum_s = 0, sum_r = 0, sum_sq_s = 0, sum_sq_r = 0, sum_sxr = 0;
    uint32_t i, j;
    for (i = 0; i < 4; i++) {
        for (j = 0; j < 4; j++) {
            sum_s += s[j];
            sum_r += r[j];
            sum_sq_s += s[j] * s[j];
            sum_sq_r += r[j] * r[j];
            sum_sxr += s[j] * r[j];
        }

        s += sp;
        r += rp;
    }

    //
    // similarity
    //
    double score = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, count, 8);
    return score;
}

double svt_ssim_8x8_c(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp) {
    const int32_t count = 8 * 8;

    //
    // is similar to svt_aom_ssim_parms_8x8_c, but supports MxN block size
    //
    uint32_t sum_s = 0, sum_r = 0, sum_sq_s = 0, sum_sq_r = 0, sum_sxr = 0;
    uint32_t i, j;
    for (i = 0; i < 8; i++) {
        for (j = 0; j < 8; j++) {
            sum_s += s[j];
            sum_r += r[j];
            sum_sq_s += s[j] * s[j];
            sum_sq_r += r[j] * r[j];
            sum_sxr += s[j] * r[j];
        }

        s += sp;
        r += rp;
    }

    //
    // similarity
    //
    double score = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, count, 8);
    return score;
}

double svt_ssim_4x4_hbd_c(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp) {
    const int32_t count = 4 * 4;

    uint32_t sum_s = 0, sum_r = 0, sum_sq_s = 0, sum_sq_r = 0, sum_sxr = 0;
    uint32_t i, j;
    for (i = 0; i < 4; i++) {
        for (j = 0; j < 4; j++) {
            sum_s += s[j];
            sum_r += r[j];
            sum_sq_s += s[j] * s[j];
            sum_sq_r += r[j] * r[j];
            sum_sxr += s[j] * r[j];
        }

        s += sp;
        r += rp;
    }

    //
    // similarity
    //
    double score = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, count, 10);
    return score;
}

double svt_ssim_8x8_hbd_c(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp) {
    const int32_t count = 8 * 8;

    uint32_t sum_s = 0, sum_r = 0, sum_sq_s = 0, sum_sq_r = 0, sum_sxr = 0;
    uint32_t i, j;
    for (i = 0; i < 8; i++) {
        for (j = 0; j < 8; j++) {
            sum_s += s[j];
            sum_r += r[j];
            sum_sq_s += s[j] * s[j];
            sum_sq_r += r[j] * r[j];
            sum_sxr += s[j] * r[j];
        }

        s += sp;
        r += rp;
    }

    //
    // similarity
    //
    double score = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, count, 10);
    return score;
}

static double ssim_8x8_blocks(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp, uint32_t width,
                              uint32_t height) {
    uint32_t i, j;
    int      samples    = 0;
    double   ssim_total = 0;

    // sample point start with each 4x4 location
    for (i = 0; i <= height - 8; i += 8, s += sp * 8, r += rp * 8) {
        for (j = 0; j <= width - 8; j += 8) {
            double v = svt_ssim_8x8(s + j, sp, r + j, rp);
            v        = CLIP3(0, 1, v);
            ssim_total += v;
            samples++;
        }
    }
    assert(samples > 0);
    ssim_total /= samples;
    assert(ssim_total <= 1.0 && ssim_total >= 0);
    return ssim_total;
}

static double ssim_4x4_blocks(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp, uint32_t width,
                              uint32_t height) {
    uint32_t i, j;
    int      samples    = 0;
    double   ssim_total = 0;

    // sample point start with each 2x2 location
    for (i = 0; i <= height - 4; i += 4, s += sp * 4, r += rp * 4) {
        for (j = 0; j <= width - 4; j += 4) {
            double v = svt_ssim_4x4(s + j, sp, r + j, rp);
            v        = CLIP3(0, 1, v);
            ssim_total += v;
            samples++;
        }
    }
    assert(samples > 0);
    ssim_total /= samples;
    assert(ssim_total <= 1.0 && ssim_total >= 0);
    return ssim_total;
}

static double ssim(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp, uint32_t width, uint32_t height) {
    assert((width % 4) == 0 && (height % 4) == 0);
    if ((width % 8) == 0 && (height % 8) == 0) {
        return ssim_8x8_blocks(s, sp, r, rp, width, height);
    } else {
        return ssim_4x4_blocks(s, sp, r, rp, width, height);
    }
}

static double ssim_8x8_blocks_hbd(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp, uint32_t width,
                                  uint32_t height) {
    uint32_t i, j;
    int      samples    = 0;
    double   ssim_total = 0;

    // sample point start with each 4x4 location
    for (i = 0; i <= height - 8; i += 8, s += sp * 8, r += rp * 8) {
        for (j = 0; j <= width - 8; j += 8) {
            double v = svt_ssim_8x8_hbd(s + j, sp, r + j, rp);
            v        = CLIP3(0, 1, v);
            ssim_total += v;
            samples++;
        }
    }
    assert(samples > 0);
    ssim_total /= samples;
    assert(ssim_total <= 1.0 && ssim_total >= 0);
    return ssim_total;
}

static double ssim_4x4_blocks_hbd(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp, uint32_t width,
                                  uint32_t height) {
    uint32_t i, j;
    int      samples    = 0;
    double   ssim_total = 0;

    // sample point start with each 2x2 location
    for (i = 0; i <= height - 4; i += 4, s += sp * 4, r += rp * 4) {
        for (j = 0; j <= width - 4; j += 4) {
            double v = svt_ssim_4x4_hbd(s + j, sp, r + j, rp);
            v        = CLIP3(0, 1, v);
            ssim_total += v;
            samples++;
        }
    }
    assert(samples > 0);
    ssim_total /= samples;
    assert(ssim_total <= 1.0 && ssim_total >= 0);
    return ssim_total;
}

static double ssim_hbd(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp, uint32_t width,
                       uint32_t height) {
    assert((width % 4) == 0 && (height % 4) == 0);
    if ((width % 8) == 0 && (height % 8) == 0) {
        return ssim_8x8_blocks_hbd(s, sp, r, rp, width, height);
    } else {
        return ssim_4x4_blocks_hbd(s, sp, r, rp, width, height);
    }
}

uint64_t svt_spatial_full_distortion_ssim_kernel(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                 uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                 uint32_t area_width, uint32_t area_height, bool hbd, double ac_bias) {
    uint8_t        m     = 1;
    const uint32_t count = area_width * area_height;

    // SSIM
    uint64_t spatial_distortion;
    double   ssim_score;

    // AC SAD
    uint64_t psy_distortion = 0;

    if (!hbd) {
        ssim_score = ssim(
            input + input_offset, input_stride, recon + recon_offset, recon_stride, area_width, area_height);
        if (ac_bias) {
            uint64_t ac_distortion = svt_psy_distortion(
                input + input_offset, input_stride, recon + recon_offset, recon_stride, area_width, area_height);
            psy_distortion = (uint64_t)(ac_distortion * ac_bias);
        }
    } else {
        m          = 8;
        ssim_score = ssim_hbd((uint16_t*)input + input_offset,
                              input_stride,
                              (uint16_t*)recon + recon_offset,
                              recon_stride,
                              area_width,
                              area_height);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
        if (ac_bias) {
            uint64_t ac_distortion = svt_psy_distortion_hbd((uint16_t*)input + input_offset,
                                                            input_stride,
                                                            (uint16_t*)recon + recon_offset,
                                                            recon_stride,
                                                            area_width,
                                                            area_height);
            psy_distortion         = (uint64_t)(ac_distortion * ac_bias);
        }
#endif
    }

    spatial_distortion        = (uint64_t)((1 - ssim_score) * count * 100 * 7 * m);
    uint64_t total_distortion = spatial_distortion + psy_distortion;

    return total_distortion;
}
