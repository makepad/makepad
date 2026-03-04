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

#ifndef EbRateDistortionCost_h
#define EbRateDistortionCost_h

/***************************************
 * Includes
 ***************************************/
#include "md_process.h"
#include "enc_intra_prediction.h"
#include "enc_inter_prediction.h"
#include "lambda_rate_tables.h"
#include "transforms.h"
#include "enc_dec_process.h"
#include "entropy_coding.h"

#ifdef __cplusplus
extern "C" {
#endif
uint64_t svt_av1_cost_coeffs_txb(ModeDecisionContext* ctx, uint8_t allow_update_cdf, FRAME_CONTEXT* ec_ctx,
                                 ModeDecisionCandidateBuffer* cand_bf, const TranLow* const qcoeff, uint16_t eob,
                                 PlaneType plane_type, TxSize transform_size, TxType transform_type,
                                 int16_t txb_skip_ctx, int16_t dc_sign_ctx, bool reduced_transform_set_flag);
void     svt_aom_coding_loop_context_generation(PictureControlSet* pcs, ModeDecisionContext* ctx);
#define RDDIV_BITS 7

#define RDCOST(RM, R, D)                                                         \
    (ROUND_POWER_OF_TWO(((int64_t)(R)) * ((int64_t)(RM)), AV1_PROB_COST_SHIFT) + \
     ((int64_t)(D) * ((int64_t)1 << RDDIV_BITS)))

int64_t  svt_aom_partition_rate_cost(PictureParentControlSet* pcs, const BlockSize bsize, const int mi_row,
                                     const int mi_col, MdRateEstimationContext* md_rate_est_ctx, PartitionType p,
                                     const PartitionContextType left_ctx, const PartitionContextType above_ctx);
uint64_t svt_aom_get_intra_uv_fast_rate(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                        ModeDecisionCandidateBuffer* cand_bf, bool use_accurate_cfl);
uint64_t svt_aom_intra_fast_cost(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                 uint64_t lambda, uint64_t luma_distortion);
uint64_t svt_aom_inter_fast_cost(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                 uint64_t lambda, uint64_t luma_distortion);
EbErrorType svt_aom_full_cost_light_pd0(ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                                        uint64_t* y_distortion, uint64_t lambda, uint64_t* y_coeff_bits);
void        svt_aom_full_cost(PictureControlSet* pcs, ModeDecisionContext* ctx, ModeDecisionCandidateBuffer* cand_bf,
                              uint64_t lambda, uint64_t y_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cb_distortion[DIST_TOTAL][DIST_CALC_TOTAL],
                              uint64_t cr_distortion[DIST_TOTAL][DIST_CALC_TOTAL], uint64_t* y_coeff_bits,
                              uint64_t* cb_coeff_bits, uint64_t* cr_coeff_bits);
uint64_t    svt_aom_tx_size_bits(PictureControlSet* pcs, uint8_t segment_id, MdRateEstimationContext* md_rate_est_ctx,
                                 MacroBlockD* xd, const MbModeInfo* mbmi, TxSize tx_size, TxMode tx_mode, BlockSize bsize,
                                 uint8_t skip, FRAME_CONTEXT* ec_ctx, uint8_t allow_update_cdf);

uint64_t svt_aom_get_tx_size_bits(ModeDecisionCandidateBuffer* candidateBuffer, ModeDecisionContext* ctx,
                                  PictureControlSet* pcs, uint8_t tx_depth, bool block_has_coeff);

MvJointType svt_av1_get_mv_joint(const Mv* mv);
int32_t     svt_av1_mv_bit_cost(const Mv* mv, const Mv* ref, const int32_t* mvjcost, const int32_t* const mvcost[2],
                                int32_t weight);
int32_t     svt_av1_mv_bit_cost_light(const Mv* mv, const Mv* ref);
int32_t svt_aom_get_switchable_rate(BlockModeInfo* block_mi, const FrameHeader* const frm_hdr, ModeDecisionContext* ctx,
                                    const bool enable_dual_filter);

// The MD version of this function omits the skip_mode check. If IFS is selected, skip_mode will be disabled.
static INLINE int32_t av1_is_interp_needed_md(BlockModeInfo* block_mi, PictureControlSet* pcs, BlockSize bsize) {
    /*Disable check on skip_mode_allowed (i.e. skip_mode).  If IFS is selected, skip_mode will be
     * disabled.*/
    if (block_mi->motion_mode == WARPED_CAUSAL) {
        return 0;
    }

    if (svt_aom_is_nontrans_global_motion(block_mi, bsize, pcs->ppcs)) {
        return 0;
    }

    return 1;
}

static INLINE uint8_t av1_drl_ctx(const CandidateMv* ref_mv_stack, int32_t ref_idx) {
    return ref_mv_stack[ref_idx].weight >= REF_CAT_LEVEL   ? ref_mv_stack[ref_idx + 1].weight >= REF_CAT_LEVEL ? 0 : 1
        : ref_mv_stack[ref_idx + 1].weight < REF_CAT_LEVEL ? 2
                                                           : 0;
}

// Transform end of block bit estimation
int get_eob_cost(int eob, const LvMapEobCost* txb_eob_costs, const LvMapCoeffCost* txb_costs, TxClass tx_class);
#ifdef __cplusplus
}
#endif
#endif //EbRateDistortionCost_h
