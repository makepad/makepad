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

#ifndef EbModeDecisionProcess_h
#define EbModeDecisionProcess_h

#include "definitions.h"
#include "mode_decision.h"
#include "sys_resource_manager.h"
#include "pic_buffer_desc.h"
#include "reference_object.h"
#include "neighbor_arrays.h"
#include "object.h"
#include "enc_inter_prediction.h"

#ifdef __cplusplus
extern "C" {
#endif
/**************************************
 * Defines
 **************************************/
#define MODE_DECISION_CANDIDATE_MAX_COUNT_Y 1855
#define MODE_DECISION_CANDIDATE_MAX_COUNT (MODE_DECISION_CANDIDATE_MAX_COUNT_Y + 84)
#define MAX_MVP_CANIDATES 4
/**************************************
 * Macros
 **************************************/

#define GROUP_OF_4_8x8_BLOCKS(org_x, org_y) (((org_x >> 3) & 0x1) && ((org_y >> 3) & 0x1) ? true : false)
#define GROUP_OF_4_16x16_BLOCKS(org_x, org_y) \
    (((((org_x >> 3) & 0x2) == 0x2) && (((org_y >> 3) & 0x2) == 0x2)) ? true : false)
#define GROUP_OF_4_32x32_BLOCKS(org_x, org_y) \
    (((((org_x >> 3) & 0x4) == 0x4) && (((org_y >> 3) & 0x4) == 0x4)) ? true : false)

/**************************************
 * Coding Loop Context
 **************************************/
typedef struct MdEncPassCuData {
    uint64_t chroma_distortion;
} MdEncPassCuData;

typedef struct PALETTE_BUFFER {
    uint8_t best_palette_color_map[MAX_PALETTE_SQUARE];
    int     kmeans_data_buf[2 * MAX_PALETTE_SQUARE];
} PALETTE_BUFFER;

typedef struct RefResults {
    // to process this ref or not
    uint8_t do_ref;
} RefResults;

typedef struct PmeResults {
    // distortion
    uint32_t dist;
} PmeResults;

typedef enum InterCandGroup {
    // elementary-groups
    PA_ME_GROUP,
    UNI_3x3_GROUP,
    BI_3x3_GROUP,
    NRST_NEW_NEAR_GROUP,
    NRST_NEAR_GROUP,
    PRED_ME_GROUP,
    GLOBAL_GROUP,
    // complex-groups
    WARP_GROUP,
    OBMC_GROUP,
    INTER_INTRA_GROUP,
    INTER_COMP_GROUP, // dist, diff, wedge
    TOT_INTER_GROUP
} InterCandGroup;

typedef struct InterCompCtrls {
    // total compound types to test; 0: OFF, 1: AVG, 2: AVG/DIST, 3: AVG/DIST/DIFF/WEDGE, 4:
    // AVG/DIST/DIFF/WEDGE
    uint8_t tot_comp_types;
    // if true, test all compound types for me
    bool do_me;
    // if true, test all compound types for pme
    bool do_pme;
    // if true, test all compound types for nearest_nearest
    bool do_nearest_nearest;
    // if true, test all compound types for near_near
    bool do_near_near;
    // if true, test all compound types for nearest_near_new
    bool do_nearest_near_new;
    // if true, test all compound types for 3x3_bipred
    bool do_3x3_bi;
    // if true, test all compound types for global
    bool do_global;
    // multiplier to the pred0_to_pred1_sad; 0: no pred0_to_pred1_sad-based pruning, >= 1: towards
    // more inter-inter compound
    uint8_t pred0_to_pred1_mult;
    // Skip compound if any of the MV components are greater than max_mv_length
    uint16_t max_mv_length;
    // Skip MVP compound based on ref frame type and neighbour ref frame types
    bool skip_on_ref_info;
    // if true, use rate @ compound params derivation
    bool use_rate;
    // no distance for symteric refs
    bool no_sym_dist;
} InterCompCtrls;

typedef struct InterIntraCompCtrls {
    uint8_t enabled;
    // if 1 use curvefit model to estimate RD cost; if 0 use SSE
    uint8_t use_rd_model;
    // 0: no wedge, 1: inj. wedge II as separate candidate to MDS0, 2: only inj. wedge if better than non-wedge at II search
    uint8_t wedge_mode_sq;
    // 0: no wedge, 1: inj. wedge II as separate candidate to MDS0, 2: only inj. wedge if better than non-wedge at II search
    uint8_t wedge_mode_nsq;
} InterIntraCompCtrls;

typedef struct ObmcControls {
    uint8_t enabled;
    // Specifies max block size to refine
    uint8_t max_blk_size_to_refine;
    // Specifies max block size to test
    uint8_t max_blk_size;
    // Specifies the level the obmc refinement
    // 0: Full/Sub @ MDS0, 1: Full/Sub @ MDS1, 2: Only Sub MDS1, 3: Full/Sub @ MDS3, 4: Only Sub MDS3
    uint8_t refine_level;
    // if true, a face-off between simple-translation and obmc will take place at mds0
    uint8_t trans_face_off;
    // Specifies the search range @ the full-pel of OBMC
    uint8_t fpel_search_range;
    // Whether to search diagonal positions @ the full-pel of OBMC
    uint8_t fpel_search_diag;
} ObmcControls;

typedef struct TxtControls {
    uint8_t enabled;

    // group to use when inter and tx block < 16x16
    uint8_t txt_group_inter_lt_16x16;
    // group to use when inter and tx block >= 16x16
    uint8_t txt_group_inter_gt_eq_16x16;

    // group to use when intra and tx block < 16x16
    uint8_t txt_group_intra_lt_16x16;
    // group to use when intra and tx block >= 16x16
    uint8_t txt_group_intra_gt_eq_16x16;
    // Per unit distortion TH; if best TX distortion is below the TH, skip remaining TX types (0
    // OFF, higher = more aggressive)
    uint32_t early_exit_dist_th;
    // If best TX number of coeffs is less than the TH, skip remaining TX types (0 OFF, higher =
    // more aggressive)
    uint32_t early_exit_coeff_th;
    // If dev. of satd of current tx_type to best is greater than TH, exit early from testing
    // current tx_type. 0: off; larger value is safer
    uint16_t satd_early_exit_th_intra;
    // If dev. of satd of current tx_type to best is greater than TH, exit early from testing
    // current tx_type. 0: off; larger value is safer
    uint16_t satd_early_exit_th_inter;
    // If the rate cost of using a TX type is greater than the percentage threshold of the cost of the best TX type (actual cost, not just rate cost),
    // skip testing the TX type. txt_rate_cost_th is specified as a perentage * 10 (i.e. a value of 70 corresponds to skipping the TX type if the
    // txt rate cost is > 7% of the best TX type cost). 0 is off.  Lower values are more aggressive.
    uint16_t txt_rate_cost_th;
    // Whether to perform QP-based SATD-threshold pruning
    uint16_t satd_th_q_weight;
} TxtControls;

typedef struct TxsCycleRControls {
    // On/Off feature control
    uint8_t enabled;
    // Threshold to bypass intra TXS <the higher th the higher speed>
    uint16_t intra_th;
    // Threshold to bypass inter TXS <the higher th the higher speed>
    uint16_t inter_th;
} TxsCycleRControls;

typedef struct NearCountCtrls {
    uint8_t enabled;

    // max # of near to consider
    uint8_t near_count;
    // max # of near_near to consider
    uint8_t near_near_count;
} NearCountCtrls;

typedef struct Bipred3x3Controls {
    // Specifies whether to refine the ME-MV(s) of bipred @ mds0 or not (0: OFF, 1: ON)
    uint8_t enabled;
    // Specifies whether to search the diagonal position(s) or not (0: OFF, 1: ON)
    uint8_t search_diag;
    // Specifies whether to refine the MV(s) of only the best List (in terms of pred-error) or both (0: OFF, 1: ON)
    uint8_t use_best_list;
    // Specifies whether to skip the refinement when the List0-to-List1 pred-error deviation is higher than use_best_list or not
    uint8_t use_l0_l1_dev;

} Bipred3x3Controls;

typedef struct RefPruningControls {
    // 0: OFF; 1: use inter to inter distortion deviation to derive best_refs
    uint8_t enabled;
    // 0: OFF; 1: limit the injection to the best references based on distortion
    uint32_t max_dev_to_best[TOT_INTER_GROUP];
    uint8_t  use_tpl_info_offset;
    uint8_t  check_closest_multiplier;
    // 0: OFF; 1: limit the injection to the closest references based on distance (LAST/BWD)
    uint8_t closest_refs[TOT_INTER_GROUP];
} RefPruningControls;

typedef struct DepthRemovalCtrls {
    uint8_t enabled;
    // remove 32x32 blocks and below based on the sb_64x64 (me_distortion, variance)
    uint8_t disallow_below_64x64;
    // remove 16x16 blocks and below based on the sb_64x64 (me_distortion, variance)
    uint8_t disallow_below_32x32;
    // remove 8x8 blocks and below based on the sb_64x64 (me_distortion, variance)
    uint8_t disallow_below_16x16;
    // remove 8x8 blocks and below based on the sb_64x64 (me_distortion, variance)
    uint8_t disallow_4x4;
} DepthRemovalCtrls;

#define MAX_RANGE_CNT 8

#define PD0_DEPTH_NO_RESTRICTION 0 // No depth restriction
#define PD0_DEPTH_ADAPTIVE 1 // Adaptive depth control
#define PD0_DEPTH_PRED_PART_ONLY 2 // Pred-part only

typedef struct DepthRefinementCtrls {
    // Mode selection:
    // 0 - No depth restriction
    // 1 - Adaptive depth control
    // 2 - Pred-part only
    uint8_t mode;
    // Default parent-to-current cost deviation beyond which the previous depth will not be
    // added to PRED
    uint8_t s1_parent_to_current_th;
    uint8_t s2_parent_to_current_th;
    // Default sub-to-current cost deviation beyond which the next depth will not be added
    // to PRED
    uint8_t e1_sub_to_current_th;
    uint8_t e2_sub_to_current_th;
    // When enabled, only prune the parent depth when the cost is sufficiently high (i.e. the parent block is
    // sufficiently complex). The signal is specified as a multiplier to a threshold (the threshold is
    // an absolute cost).  A higher value is more conservative; 0 is off.
    // parent_max_cost_th_mult not relevant when parent is never skipped by parent_to_current_th
    uint16_t parent_max_cost_th_mult;
    // Whether whether to modulate the deviation thresholds and bounds based on the coefficient level
    uint8_t coeff_lvl_modulation;
    // Whether to decrement parent_to_current_th and sub_to_current_th based on the cost range of
    // the parent block or not
    uint8_t cost_band_based_modulation;
    // Max cost beyond which the decrement is ignored
    uint16_t max_cost_multiplier;
    // Number of band(s)
    uint8_t max_band_cnt;
    // Offset per band
    int64_t decrement_per_band[MAX_RANGE_CNT];
    // Skip parent depth if PARTITION_SPLIT rate of parent depth is much lower than parent cost. 0 is off; higher is more aggressive.
    uint32_t lower_depth_split_cost_th;
    // Skip child depth if PARTITION_SPLIT rate of current depth is x% higher than current cost. 0 is off; lower is more aggressive.
    uint32_t split_rate_th;
    // If true, limit the max/min block sizes for PD1 to the max/min selected by PD0 (when the max/min block sizes are different).
    uint8_t limit_max_min_to_pd0;
    // If true, check whether current and ref are selecting the largest block size, then force Pred
    uint8_t use_ref_info;
    // Whether to use QP to modulate the sub/parent-to-current threshold(s).
    uint8_t q_weight;
    // Handling mode for cases where PD0 information is unavailable
    // 0 - use default s_depth and e_depth
    // 1 - cap s_depth and e_depth to -1 and 1
    // 2 - set s_depth and e_depth to 0
    uint32_t pd0_unavail_mode_depth;
} DepthRefinementCtrls;

typedef struct SubresCtrls {
    // Residual sub-sampling step (0:OFF)
    uint8_t step;
    // Set step to 0 if the deviation between: (1) the pred-to-src SAD of even rows and (2) the
    // pred-to-src SAD of odd rows of the 1st 64x64 block @ mds3 of PD0 is higher than
    // odd_to_even_deviation_th when 0, the detection is OFF
    uint8_t odd_to_even_deviation_th;
} SubresCtrls;

typedef struct PfCtrls {
    EB_TRANS_COEFF_SHAPE pf_shape;
} PfCtrls;

typedef struct MdNsqMotionSearchCtrls {
    // 0: NSQ motion search @ MD OFF; 1: NSQ motion search @ MD ON
    uint8_t enabled;
    // 0: search using SAD; 1: search using Variance; 2: search using SSD
    DistortionType dist_type;
    // Full Pel search area width
    uint8_t full_pel_search_width;
    // Full Pel search area height
    uint8_t full_pel_search_height;
    // Enable pSad
    uint8_t enable_psad;
} MdNsqMotionSearchCtrls;

typedef struct MdSqMotionSearchCtrls {
    // 0: SQ motion search @ MD OFF; 1: SQ motion search @ MD ON
    uint8_t enabled;
    // 0: search using SAD; 1: search using Variance; 2: search using SSD
    DistortionType dist_type;
    // TH for pa_me distortion to determine whether to search (distortion per pixel)
    uint16_t pame_distortion_th;

    // 0: OFF; 1: ON
    uint8_t sprs_lev0_enabled;
    // Sparse search step
    uint8_t sprs_lev0_step;
    // Sparse search area width
    uint16_t sprs_lev0_w;
    // Sparse search area height
    uint16_t sprs_lev0_h;
    // Max Sparse search area width
    uint16_t max_sprs_lev0_w;
    // Max Sparse search area height
    uint16_t max_sprs_lev0_h;
    // search area multiplier (is a % -- 100 is no scaling)
    int16_t sprs_lev0_multiplier;

    // 0: OFF; 1: ON
    uint8_t sprs_lev1_enabled;
    // Sparse search step
    uint8_t sprs_lev1_step;
    // Sparse search area width
    uint16_t sprs_lev1_w;
    // Sparse search area height
    uint16_t sprs_lev1_h;
    // Max Sparse search area width
    uint16_t max_sprs_lev1_w;
    // Max Sparse search area height
    uint16_t max_sprs_lev1_h;
    // search area multiplier (is a % -- 100 is no scaling)
    int16_t sprs_lev1_multiplier;

    // 0: OFF; 1: ON
    uint8_t sprs_lev2_enabled;
    // Sparse search step
    uint8_t sprs_lev2_step;
    // Sparse search area width
    uint16_t sprs_lev2_w;
    // Sparse search area height
    uint16_t sprs_lev2_h;
    // Enable pSad
    uint8_t enable_psad;
} MdSqMotionSearchCtrls;

typedef struct MdPmeCtrls {
    // 0: PME search @ MD OFF; 1: PME search @ MD ON
    uint8_t enabled;
    // 0: search using SAD; 1: search using Variance; 2: search using SSD
    DistortionType dist_type;
    // Do not perform PME search for blocks that have a valid ME_MV unless the ME_MV has a different
    // direction than all MVP(s) and the ME_MV mag is higher than MV_TH=f(early_check_mv_th_multiplier)
    int early_check_mv_th_multiplier;
    // Full Pel search area width
    uint8_t full_pel_search_width;
    // Full Pel search area height
    uint8_t full_pel_search_height;
    // If pre_fp_pme_to_me_cost higher than pre_fp_pme_to_me_cost_th then PME_MV = ME_MV and exit
    // (decrease towards a faster level)
    int pre_fp_pme_to_me_cost_th;
    // If pre_fp_pme_to_me_mv smaller than pre_fp_pme_to_me_mv_th then PME_MV = ME_MV and exit
    // (increase towards a faster level)
    int pre_fp_pme_to_me_mv_th;
    // If post_fp_pme_to_me_cost higher than post_fp_pme_to_me_cost_th then PME_MV = ME_MV and exit
    // (decrease towards a faster level)
    int post_fp_pme_to_me_cost_th;
    // If post_fp_pme_to_me_mv smaller than post_fp_pme_to_me_mv_th then PME_MV = ME_MV and exit
    // (increase towards a faster level)
    int post_fp_pme_to_me_mv_th;
    // Enable pSad
    uint8_t enable_psad;
    // Whether to perform QP-based search-area pruning
    uint8_t sa_q_weight;
} MdPmeCtrls;

typedef struct MdSubPelSearchCtrls {
    // Specifies whether the Sub-Pel search will be performed or not (0: OFF, 1: ON)
    uint8_t enabled;
    // Specifies the interpolation filter tap (1: 2-tap filter, 2: 4-tap filter, 3: 8-tap filter)
    SUBPEL_SEARCH_TYPE subpel_search_type;
    // Specifies the refinement precision (or number of rounds) (0: 1/8-Pel (3 rounds), 1: 1/4-Pel
    // (2 rounds), 2: 1/2-Pel (1 round), 3: Full-Pel-no refinement (0 round))
    SUBPEL_FORCE_STOP max_precision;
    // Specifies whether pruning will be applied to 1/2-Pel position(s) or not (SUBPEL_TREE: No,
    // SUBPEL_TREE_PRUNED: YES)
    SUBPEL_SEARCH_METHODS subpel_search_method;
    // Specifies the maximum number of steps in logarithmic subpel search before giving up
    int subpel_iters_per_step;
    // Specifies the Full-Pel prediction-block-variance threshold under which the Sub-Pel search is
    // not performed; do not perform Sub-Pel if the variance of the Full-Pel prediction-block is low
    // (where interpolation will unlikely modify the Full-Pel samples)
    int pred_variance_th;
    // Specifies the Full-Pel prediction-block-error-threshold below which the Sub-Pel search is not
    // performed; do not perform Sub-Pel if the prediction-block-error is already low
    uint8_t abs_th_mult;
    // Specifies the prediction-block-error deviation threshold between round-(N-1) and round-(N-2)
    // under which the refinement is paused; pause the refinement if the prediction-block-error is
    // not getting better through the process (the check takes place at only the 2nd round (prior to
    // the 1/4-Pel refinement) and the 3rd round (prior to the 1/8-Pel refinement).
    int round_dev_th;
    // Specifies the refinement accuracy for diagonal position(s).
    uint8_t skip_diag_refinement;
    // Specifies whether the Sub-Pel search will be performed for around (0,0) or not (0: OFF, 1:
    // ON)
    uint8_t skip_zz_mv;
    uint8_t min_blk_sz; //blk size below which we skip subpel
    uint8_t mvp_th; // when > 0, use mvp info to skip hpel search. skip if ME is  worse than MVP.
    // Skip high precision (1/8-Pel) when the ME vs. MVP MV difference (x or y) is larger than the threshold.
    // MAX_SIGNED_VALUE is off; 0 is safest on TH, higher is more aggressive
    int hp_mv_th;
    int32_t
        bias_fp; // Bias towards fpel at the MD subpel-search: apply a penalty to the cost of fractional positions during the subpel-search each time we check against a full-pel MV
} MdSubPelSearchCtrls;

typedef struct NsqPsqTxsCtrls {
    uint8_t enabled;
    uint32_t
        hv_to_sq_th; // Skip NSQ if min(H0-nz,H1-nz) is higher than hv_to_sq_th * sq-nz, and min(V0-nz,V1-nz) is higher than hv_to_sq_th * sq-nz
    uint32_t
        h_to_v_th; // Skip H/V if min(H0-nz,H1-nz)/min(V0-nz,V1-nz) is higher than min(V0-nz,V1-nz)/min(H0-nz,H1-nz),
    // and min(H0-nz,H1-nz)/min(V0-nz,V1-nz) is higher than h_to_v_th * sq-nz
} NsqPsqTxsCtrls;

typedef struct RdoqCtrls {
    uint8_t enabled;
#if OPT_RDOQ_BIS
    // 0: do not apply cutoff; >=1: apply RDOQ only to the percentage of coefficients near EOB defined by cut_off_num / cut_off_denum, skipping the rest (low frequencies)
    uint16_t cut_off_num;
    uint16_t cut_off_denum;
#else
    // 0: do not use cut off div; >=1: limit rdoq to a fixed low-frequency cut-off (DC + first AC coefficients) and skip rdoq on all higher frequencies
    uint16_t cut_off_div;
    // 0: do not use eob_fast for luma inter; 1: use eob_fast for luma inter
    uint8_t eob_fast_y_inter;
    // 0: do not use eob_fast for luma intra; 1: use eob_fast for luma intra
    uint8_t eob_fast_y_intra;
    // 0: do not use eob_fast for chroma inter; 1: use eob_fast for chroma inter
    uint8_t eob_fast_uv_inter;
    // 0: do not use eob_fast for chroma intra; 1: use eob_fast for chroma intra
    uint8_t eob_fast_uv_intra;
    // 0: use default quant for luma; 1: use fp_quant for luma
    uint8_t fp_q_y;
    // 0: use default quant for chroma; 1: use fp_quant for chroma
    uint8_t fp_q_uv;
    // Do not perform rdoq if the tx satd > satd_factor
    uint8_t satd_factor;
    // Do not perform rdoq based on an early skip/non-skip cost
    uint8_t early_exit_th;
#endif
    // [MD Only] 0: RDOQ for both Luma & Chroma, 1: RDOQ for only Luma
    uint8_t skip_uv;
    // [MD Only] 0: RDOQ for All txt type(s), 1: RDOQ for only DCT_DCT
    uint8_t dct_dct_only;
    // eob_th beyond which RDOQ is shut
    uint8_t eob_th;
    uint8_t eob_fast_th;
} RdoqCtrls;

typedef struct NicScalingCtrls {
    // Scaling numerator for post-stage 0 NICS: <x>/16
    uint8_t stage1_scaling_num;
    // Scaling numerator for post-stage 1 NICS: <x>/16
    uint8_t stage2_scaling_num;
    // Scaling numerator for post-stage 2 NICS: <x>/16
    uint8_t stage3_scaling_num;
} NicScalingCtrls;

typedef struct NicPruningCtrls {
    // class pruning signal(s)
    // mdsx_class_th (for class removal); reduce cand if deviation to the best_cand is higher than mdsx_cand_th

    // All bands (except the last) are derived as follows:
    // For band_index=0 to band_index=(mdsx_band_cnt-2),
    //     band=[band_index*band_width, (band_index+1)*band_width]; band_width = mdsx_class_th/(band_cnt-1)
    //     multiplier= 1 / ((band_index+1)*2)
    // Last band is [mds1_class_th, +?] = kill (nic=0)

    // e.g. mds1_class_th=20 and mds1_band_cnt=3
    // band_index |0       |1         |2        |
    // band       |0 to 10 |10 to 20  |20 to +? |
    // action     |nic * 1 |nic * 1/2 |nic * 0  |

    // Post mds0
    uint64_t mds1_class_th;
    // Post mds1
    uint8_t  mds1_band_cnt;
    uint64_t mds2_class_th;
    // >=2 Post mds2
    uint8_t  mds2_band_cnt;
    uint64_t mds3_class_th;
    // cand pruning signal(s)
    // mdsx_cand_th (for single cand removal per class); remove cand if
    // deviation to the best_cand for @ the target class is higher than mdsx_cand_th
    // mdsx_cand_th = base_th + sq_offset_th + intra_class_offset_th Post mds0
    uint8_t mds3_band_cnt;
    // Post-MDS0 candidate pruning TH for intra classes
    uint64_t mds1_cand_base_th_intra;
    // Post-MDS0 candidate pruning TH for inter classes
    uint64_t mds1_cand_base_th_inter;

    // Use a more aggressive MDS0 candidate pruning TH based on how the candidate is ranked compared to others
    // (2nd best candidate will use a safer pruning TH than the 3rd best, and so on).
    // The new_th = mds1_cand_base_th / (mds1_cand_th_rank_factor * cand_count); where cand_count is the
    // number of candidates with better cost than the current candidate. 0 is off. Higher is more aggressive.
    uint16_t mds1_cand_th_rank_factor;

    // Post mds1
    uint64_t mds2_cand_base_th;
    // Use a more aggressive MDS1 candidate pruning TH based on how the candidate is ranked compared
    // to others (2nd best candidate will use a safer pruning TH than the 3rd best, and so on). The
    // new_th = mds2_cand_base_th / (mds2_cand_th_rank_factor * cand_count); where cand_count is the
    // number of candidates with better cost than the current candidate. 0 is off. Higher is more
    // aggressive.
    uint16_t mds2_cand_th_rank_factor;
    // Prune candidates based on the relative deviation compared to the previous candidate.
    // Deviations are still computed compared to the best candidate. If dev > previous_dev + th then
    // prune the candidate.
    uint16_t mds2_relative_dev_th;

    // Post mds2
    uint64_t mds3_cand_base_th;

    // enable skipping MDS1 in PD1 when there is only 1 cand post-mds0
    bool    enable_skipping_mds1;
    uint8_t merge_inter_cands_mult;
} NicPruningCtrls;

typedef struct NicCtrls {
    NicPruningCtrls pruning_ctrls;
    NicScalingCtrls scaling_ctrls;
    // to specify the number of md-stage(s)
    uint8_t md_staging_mode;
} NicCtrls;

typedef struct CandEliminationCtlrs {
    uint32_t enabled;
    // if inter distortion is below dc_only_th * block_area then test DC only for intra candidates
    // inter distortion can be from pme/subpel or MDS0 (for LPD1 only)
    // 0: off, higher is more aggressive
    uint16_t dc_only_th;
    // if inter distortion is below skip_dc_th * block_area then skip testing intra candidates
    // skip_dc_th active in LPD1 MDS0 only.
    // 0: off, higher is more aggressive
    uint16_t skip_dc_th;
} CandEliminationCtlrs;

typedef struct NsqGeomCtrls {
    // Enable or disable nsq signal. 0: disabled, 1: enabled
    bool enabled;
    // Disables all nsq blocks for below a specified size. e.g. 8 = 8x8, 16 = 16x16
    uint8_t min_nsq_block_size;
    // Disallow H4/V4 when off. 0: OFF, 1: ON
    uint8_t allow_HV4;
    // Disallow HA/HB/VA/VB NSQ blocks when off. 0: OFF, 1: ON
    uint8_t allow_HVA_HVB;
} NsqGeomCtrls;

typedef struct NsqSearchCtrls {
    // If enabled, allow multiple NSQ shapes to be searched at MD, and use the below search features (if on) to reduce the
    // compute overhead. If not enabled, NSQ shapes may still be allowed by nsq_geom_ctrls, but no search will be performed
    // (therefore, each depth  must specify one block to be tested at MD, whether SQ or NSQ).
    bool enabled;
    // Weighting (expressed as a percentage) applied to square shape costs for determining if a and
    // b shapes should be skipped. Namely: skip HA, HB, and H4 if h_cost > (weighted sq_cost) skip
    // VA, VB, and V4 if v_cost > (weighted sq_cost)
    uint32_t sq_weight;
    // Skip H/V if the cost of H/V is bigger than the cost of V/H by hv_weight
    // Only active when sq_weight is used (ie. sq_weight != (uint32_t)~0)
    uint32_t hv_weight;
    // max_part0_to_part1_dev is used to:
    // (1) skip the H_Path if the deviation between the Parent-SQ src-to-recon distortion of (1st quadrant + 2nd quadrant) and the Parent-SQ src-to-recon distortion of (3rd quadrant + 4th quadrant) is less than TH,
    // (2) skip the V_Path if the deviation between the Parent-SQ src-to-recon distortion of (1st quadrant + 3rd quadrant) and the Parent-SQ src-to-recon distortion of (2nd quadrant + 4th quadrant) is less than TH.
    uint32_t max_part0_to_part1_dev;
    // If the rate cost of splitting into NSQ shapes is greater than the percentage threshold of the cost of the SQ block, skip testing the NSQ shape.
    // split_cost_th is specified as a perentage * 10 (i.e. a value of 70 corresponds to skipping the NSQ shape if the split rate cost is > 7% of the SQ cost).
    // 0 is off.  Lower values are more aggressive.
    uint32_t nsq_split_cost_th;
    // If the rate cost of splitting the SQ into lower depths is smaller than the percentage threshold of the cost of the SQ block, skip testing the NSQ shapes.
    // depth_split_cost_th is specified as a perentage * 100 (i.e. a value of 700 corresponds to skipping the NSQ shapes if the split rate cost is < 7% of the SQ cost).
    // 0 is off.  Higher values are more aggressive.
    uint32_t lower_depth_split_cost_th;
    // Skip testing H or V if the signaling rate of H/V is significantly higher than the rate of V/H. Specified as a percentage TH. 0 is off, higher is more aggressive.
    uint32_t H_vs_V_split_rate_th;
    // For non-H/V partitions, skip testing the partition if its signaling rate cost is significantly higher than the signaling rate cost of the
    // best partition.  Specified as a percentage TH. 0 is off, higher is more aggressive.
    uint32_t non_HV_split_rate_th;
    // Offset applied to rate thresholds for 16x16 and smaller block sizes. Higher is more aggressive; 0 is off.
    uint32_t rate_th_offset_lte16;
    // If the distortion (or rate) component of the SQ cost is more than component_multiple_th times the rate (or distortion) component, skip the NSQ shapes
    // 0: off, higher is safer
    uint32_t component_multiple_th;
    // Predict the number of non-zero coeff per NSQ shape using a non-conformant txs-search
    uint8_t psq_txs_lvl;
    // Whether to use the default or aggressive settings for the sub-Pred_depth block(s) (i.e. not applicable when PRED only)
    uint8_t sub_depth_block_lvl;
} NsqSearchCtrls;

typedef struct DepthEarlyExitCtrls {
    // If the rate cost of splitting into lower depths is greater than the percentage threshold of the cost of the parent block, skip testing the lower depth.
    // split_cost_th is specified as a perentage * 10 (i.e. a value of 70 corresponds to skipping the lower depth if the split rate cost is > 7% of the parent cost).
    // 0 is off.  Lower values are more aggressive. Evaluated for quadrant 0.
    uint16_t split_cost_th;
    // Skip testing remaining blocks at the current depth if (curr_cost * 1000 > early_exit_th * parent_cost)
    // Tested before each quadrant greater than quadrant 0 (use split_cost_th to skip quadrant 0). Specified as a perentage * 10. 0 is off.  Lower values are
    // more aggressive.
    uint16_t early_exit_th;
} DepthEarlyExitCtrls;

typedef struct TxsControls {
    uint8_t enabled;
    // Skip current depth if previous depth has coeff count below the TH
    uint8_t prev_depth_coeff_exit_th;
    // Max number of depth(s) for INTRA classes in SQ blocks
    uint8_t intra_class_max_depth_sq;
    // Max number of depth(s) for INTRA classes in NSQ blocks
    uint8_t intra_class_max_depth_nsq;
    // Max number of depth(s) for INTER classes in SQ blocks
    uint8_t inter_class_max_depth_sq;
    // Max number of depth(s) for INTER classes in NSQ blocks
    uint8_t inter_class_max_depth_nsq;
    // Offset to be subtracted from default txt-group to derive the txt-group of depth-1
    int depth1_txt_group_offset;
    // Offset to be subtracted from default txt-group to derive the txt-group of depth-2
    int depth2_txt_group_offset;
    //skip depth if cost of processed sublocks of curent depth > th% of normalized
    //parent cost. th is the smaller the faster (sf)
    int32_t quadrant_th_sf;

} TxsControls;

typedef struct WmCtrls {
    uint8_t enabled;
    // allow/disallow MW for MVP candidates
    uint8_t use_wm_for_mvp;
    // Number of iterations to use in the refinement search; each iteration searches the cardinal
    // neighbours around the best-so-far position; 0 is no refinement
    uint8_t refinement_iterations;
    // Refinement search for diagonal positions
    uint8_t refine_diag;
    // Specifies the MD Stage where the wm refinement will take place. 0: Before MDS0.  1: At MDS1.  2: At MDS3.
    uint8_t refine_level;
    // Specifies minimum neighbour percentage for WM
    // Skip if alpha/ beta / gamma / delta is lower than threshold value
    uint16_t lower_band_th;
    // Skip if alpha/ beta / gamma / delta is higher than threshold value
    uint16_t upper_band_th;
    // Shut the approximation(s) if refinement @ mds1 or mds3
    bool shut_approx_if_not_mds0;
} WmCtrls;

typedef struct UvCtrls {
    uint8_t enabled;
    // Indicates the chroma search mode
    // CHROMA_MODE_0: Full chroma search @ MD
    // CHROMA_MODE_1: Fast chroma search @ MD - No independent chroma mode search
    // CHROMA_MODE_2: Chroma blind @ MD
    uint8_t uv_mode;
    // 0: Perform independent chroma search before candidate preparation
    // 1: Perform independent chroma search before last MD stage
    // 2: Perform independent chroma search before last MD stage and test only the modes corresponding to luma modes that made it to MDS3
    uint8_t ind_uv_last_mds;
    // Skip the independent chroma search if the only intra chroma mode at MDS3 is DC (applies only when ind. chroma search is done at MDS3)
    uint8_t skip_ind_uv_if_only_dc;
    // Scaling numerator for independent chroma NICS: <x>/16
    uint8_t uv_nic_scaling_num;
    // Threshold to skip the independent chroma search when the search is performed before the last MD stage.
    // Compare the best inter cost vs the best intra cost and skip the independent chroma search if intra cost
    // is less than TH% of the inter cost. 0 is off.
    uint32_t inter_vs_intra_cost_th;
} UvCtrls;

typedef struct InterpolationSearchCtrls {
    // Specifies the MD Stage where the interpolation filter search will take place (IFS_MDS0,
    // IFS_MDS1, IFS_MDS2, or IFS_MDS3 for respectively MD Stage 0, MD Stage 1, MD Stage 2, and MD
    // Stage 3)
    IfsLevel level;
    // Specifies whether sub-sampled input/prediction will be used at the distortion computation (0:
    // OFF, 1: ON, NA for block height 16 and lower)
    uint8_t subsampled_distortion;
    // Specifies whether a model wll be used for rate estimation or not (0: NO (assume rate is 0),
    // 1: estimate rate from distortion)
    uint8_t skip_sse_rd_model;
} InterpolationSearchCtrls;

typedef struct SpatialSSECtrls {
    // Specifies the MD Stage where the spatial SSE will start being used in the full loop (SSSE_MDS1, SSSE_MDS2, or
    // SSSE_MDS3 for respectively MD Stage 1, MD Stage 2, and MD Stage 3).  Spatial SSE will also be enabled
    // in all subsequent MD stages, beyond the stage in which it's first enabled.  For example, if set to SSSE_MDS2,
    // spatial SSE would be enabled in MDS2 and MDS3.
    SpatialSseLevel level;
} SpatialSSECtrls;

typedef struct RedundantCandCtrls {
    int score_th;
    int mag_th;
} RedundantCandCtrls;

typedef struct UseNeighbouringModeCtrls {
    uint8_t enabled;
} UseNeighbouringModeCtrls;

typedef struct BlockLocation {
    // luma block location in picture
    uint32_t input_origin_index;
    // chroma block location in picture
    uint32_t input_cb_origin_in_index;
} BlockLocation;

typedef struct Lpd0Ctrls {
    // Whether light-PD0 is set to be used for an SB (the detector may change this)
    Pd0Level pd0_level;
    // Whether to use a detector; if use_light_pd0 is set to 1, the detector will protect tough SBs
    bool use_lpd0_detector[LPD0_LEVELS];
    // Use info of ref frames - incl. colocated SBs - such as mode, coeffs, etc. in the detector.
    // [0,2] - 0 is off, 2 is most aggressive
    uint8_t use_ref_info[LPD0_LEVELS];
    // me_8x8_cost_variance_th beyond which the PD0 is used (instead of light-PD0)
    uint32_t me_8x8_cost_variance_th[LPD0_LEVELS];
    // ME_64x64_dist threshold used for edge SBs when PD0 is skipped
    uint32_t edge_dist_th[LPD0_LEVELS];
    // Shift applied to ME dist and var of top and left SBs when PD0 is skipped
    uint16_t neigh_me_dist_shift[LPD0_LEVELS];
} Lpd0Ctrls;

typedef struct Lpd1Ctrls {
    // Whether light-PD1 is set to be used for an SB (the detector may change this)
    Pd1Level pd1_level;
    // Whether to use a detector; if use_light_pd1 is set to 1, the detector will protect tough SBs
    bool use_lpd1_detector[LPD1_LEVELS];
    // Use info of ref frames - incl. colocated SBs - such as mode, coeffs, etc. in the detector.
    // [0,1] - 0 is off, 1 is on
    uint8_t use_ref_info[LPD1_LEVELS];
    // Distortion value used in cost TH for detector
    uint32_t cost_th_dist[LPD1_LEVELS];
    // Rate value used in cost TH for detector
    uint32_t cost_th_rate[LPD1_LEVELS];
    // Num non-zero coeffs used in detector
    uint32_t nz_coeff_th[LPD1_LEVELS];
    // Max MV length TH used in the detector: 0 - (0,0) MV only; (uint16_t)~0 means no MV check
    // (higher is more aggressive)
    uint16_t max_mv_length[LPD1_LEVELS];
    // me_8x8_cost_variance_th beyond which the PD1 is used (instead of light-PD1)
    uint32_t me_8x8_cost_variance_th[LPD1_LEVELS];
    // ME_64x64_dist threshold used for edge SBs when PD0 is skipped
    uint32_t skip_pd0_edge_dist_th[LPD1_LEVELS];
    // Shift applied to ME dist and var of top and left SBs when PD0 is skipped
    uint16_t skip_pd0_me_shift[LPD1_LEVELS];
} Lpd1Ctrls;

typedef struct Lpd1TxCtrls {
    // skip cost calc and chroma TX/compensation if there are zero luma coeffs
    uint8_t zero_y_coeff_exit;
    // Control aggressiveness of chroma detector (used to skip chroma TX when luma has 0 coeffs): 0:
    // OFF, 1: saftest, 2, 3: medium
    uint8_t chroma_detector_level;
    // Skip luma TX for NRST_NRST candidates if dist/QP is low and if top and left neighbours have
    // no coeffs and are NRST_NRST
    uint8_t skip_nrst_nrst_luma_tx;
    // if (skip_tx_th/QP < TH) skip TX at MDS3; 0: OFF, higher: more aggressive
    uint16_t skip_tx_th;
    // if (best_mds0_distortion/QP < TH) use shortcuts for candidate at MDS3; 0: OFF, higher: more
    // aggressive
    uint32_t use_mds3_shortcuts_th;
    // if true, use info from neighbouring blocks to use more aggressive THs/actions
    uint8_t use_neighbour_info;
    // Apply shortcuts to the chroma TX path if luma has few coeffs
    uint8_t use_uv_shortcuts_on_y_coeffs;
} Lpd1TxCtrls;

typedef struct CflCtrls {
    bool enabled;
    // Early exit to reduce the number of iterations to compute CFL parameters
    uint8_t itr_th;
    uint8_t cplx_th;
} CflCtrls;

typedef struct MdRateEstCtrls {
    // If true, update skip context and dc_sign context (updates are done in the same func, so
    // control together)
    bool update_skip_ctx_dc_sign_ctx;
    // If true, update skip coeff context
    bool update_skip_coeff_ctx;
    // 0: OFF (always use approx for coeff rate), 1: full; always compute coeff rate, 2: when
    // num_coeff is low, use approximation for coeff rate
    uint8_t coeff_rate_est_lvl;
    // Offset applied to qindex at quantization in LPD0
    int8_t lpd0_qp_offset;
    // estimate the rate of the first (eob/N) coeff(s) and last coeff only
    uint8_t pd0_fast_coeff_est_level;
} MdRateEstCtrls;

typedef struct IntraCtrls {
    uint8_t enable_intra;
    // the last intra prediciton mode generated starting from DC_PRED, min: DC_PRED, max: PAETH_PRED
    uint8_t intra_mode_end;
    // 0: angular off; 1: angular full; 2/3: limit num. angular candidates; 4: H + V only
    uint8_t angular_pred_level;
    uint8_t prune_using_best_mode;
#if OPT_PER_BLK_INTRA
    bool prune_using_edge_info;
#endif
    int8_t skip_angular_delta1_th;
    int8_t skip_angular_delta2_th;
    int8_t skip_angular_delta3_th;
} IntraCtrls;

typedef struct TxShortcutCtrls {
    // Skip TX at MDS3 if the prev MD stage gave 0 coeffs and MDS0 Distortion is less than the TH. 0 is off, lower is more aggressive
    uint32_t bypass_tx_th;
    // Apply pf based on the number of coeffs
    uint8_t apply_pf_on_coeffs;
    // Use a detector to protect chroma from aggressive actions based on luma info: 0: OFF, 1:
    // saftest, 2, 3: medium
    uint8_t chroma_detector_level;
    // if (best_mds0_distortion/QP < TH) use shortcuts for candidates at MDS3; 0: OFF, higher: more
    // aggressive
    uint32_t use_mds3_shortcuts_th;
} TxShortcutCtrls;

typedef struct Mds0Ctrls {
    // 0: disabled, > 0: switch between: (1) reset reference cost for each subsequent class, (2) continuously update reference cost, (uint8_t) ~0: continuously update reference cost
    uint8_t pruning_method_th;
    // % TH(s) used to compare candidate distortion to best cost; higher is safer (applies to reg. PD1 only)
    uint16_t per_class_dist_to_cost_th[CAND_CLASS_TOTAL];
    uint16_t dist_to_cost_th;
} Mds0Ctrls;

typedef struct CandReductionCtrls {
    RedundantCandCtrls redundant_cand_ctrls;
    NearCountCtrls     near_count_ctrls;
    // inject unipred MVP candidates only for the best ME list
    uint8_t                  lpd1_mvp_best_me_list;
    UseNeighbouringModeCtrls use_neighbouring_mode_ctrls;
    CandEliminationCtlrs     cand_elimination_ctrls;
    uint8_t                  reduce_unipred_candidates;
    uint8_t                  reduce_filter_intra;
} CandReductionCtrls;

typedef struct SkipSubDepthCtrls {
    uint8_t enabled;
    // Use the 4 quad(s) src-to-recon cost deviation to skip sub-depth(s)
    // Do not skip sub-depth(s) if the depth block size is higher than max_size
    uint8_t max_size;
    // Do not skip sub-depth(s) if std_deviation of the src-to-rec quad(s) is higher than std_deviation_th
    int quad_deviation_th;
    // Do not skip sub-depth(s) if coeff_perc is higher than coeff_perc
    uint8_t coeff_perc;

} SkipSubDepthCtrls;
#if !CLN_REMOVE_VAR_SUB_DEPTH
typedef struct VarSkipSubDepthCtrls {
    uint8_t  enabled;
    uint32_t coeff_th;
    uint8_t  min_size;
    uint8_t  max_size;
    uint32_t edge_th[4][3];
} VarSkipSubDepthCtrls;
#endif
typedef struct FilterIntraCtrls {
    bool enabled;
    // Set the max filter intra mode to test. The max filter intra level will also depend on ctx->intra_ctrls.intra_mode_end.
    // The max mode to be test will be min(max_filter_intra_mode, ctx->intra_ctrls.intra_mode_end).
    FilterIntraMode max_filter_intra_mode;
} FilterIntraCtrls;

typedef struct CompoundPredictionStore {
    //avoid doing Unipred prediction for redundant MV
    //example: NRST_NRST:  (0,0) (1,2)
    //         NEAR_NEAR:  (1,0) (1,2)
    //pred1 for NEAR_NEAR could be retrived from  NRST_NRST
    uint8_t  pred0_cnt; //actual size for available predictions
    uint8_t* pred0_buf[4]; //stores prediction for up to 4 different MVs (NEAREST + 3 NEAR)
    Mv       pred0_mv[4]; //MVs for availble predictions

    uint8_t  pred1_cnt;
    uint8_t* pred1_buf[4];
    Mv       pred1_mv[4];
} CompoundPredictionStore;

// struct that specifies which blocks should be tested during MD
typedef struct MdScan {
    // array containing all shapes to be tested for the current SQ block
    Part shapes[PART_S];
    // total number of shapes to test for the current SQ block
    uint8_t        tot_shapes;
    struct MdScan* split[4];
    int            index; // should be written once when struct is initialized, then never overwritten
    BlockSize      bsize; // should be written once when struct is initialized, then never overwritten
    // for indexing blk_geom and assigning blk_ptr
    uint32_t mds_idx; // should be written once when struct is initialized, then never overwritten
    bool     split_flag;
    bool     is_child; // does is it belong to the child depth(s); relative to PRED (the output of PD0)
} MdScan;

/* Stores partition structure of the current block. */
typedef struct PARTITION_TREE {
    // Pointers to the children if the current block is further split.
    struct PARTITION_TREE* sub_tree[4];
    // Pointers to the EcBlkStruct holding the block's data for entropy coding
    EcBlkStruct* blk_data[4];
    // The partition type used to split the current block.
    PartitionType partition;
    // Block size of the current depth.
    BlockSize bsize; // should be written once when struct is initialized, then never overwritten
    // The index of current node among its siblings
    int index; // should be written once when struct is initialized, then never overwritten
} PARTITION_TREE;

typedef struct RD_STATS {
    int64_t rd_cost;
    bool    valid;
} RD_STATS;

// TODO: replace BlkStrut with PICK_MODE_CONTEXT
typedef struct PC_TREE {
    BlockSize     bsize; // should be written once when struct is initialized, then never overwritten
    PartitionType partition;

    RD_STATS        rdc;
    BlkStruct*      block_data[PART_S][4 /*max blocks per shape*/]; // doesn't include split
    struct PC_TREE* split[4];
    int             index; // should be written once when struct is initialized, then never overwritten
    struct PC_TREE* parent; // this_pc_tree->parent->split[this_pc_tree->index] == this_pc_tree
    bool (*tested_blk)[4]; // tested_blk[PART_S][4]
    // Origin of the current depth (square shape)
    int mi_row;
    int mi_col;
    // Partition contexts for the current block, derived from the neighbouring blocks' partitions
    PartitionContextType left_part_ctx;
    PartitionContextType above_part_ctx;
} PC_TREE;

typedef struct ModeDecisionContext {
    EbDctor dctor;

    EbFifo*                       mode_decision_configuration_input_fifo_ptr;
    EbFifo*                       mode_decision_output_fifo_ptr;
    ModeDecisionCandidate*        fast_cand_array;
    ModeDecisionCandidateBuffer** cand_bf_ptr_array;
    ModeDecisionCandidateBuffer*  cand_bf_tx_depth_1;
    ModeDecisionCandidateBuffer*  cand_bf_tx_depth_2;
    MdRateEstimationContext*      md_rate_est_ctx;
    MdRateEstimationContext*      rate_est_table;
    BlkStruct*                    md_blk_arr_nsq;
    // used to set the array in PC_TREE by the same name. Implemented as a separate allocation
    // to easily zero out the whole array (for all blocks) without looping over entire pc_tree.
    bool (*tested_blk)[PART_S][4];
    // Number of allocated tested_blk, pc_tree, and mds entries
    int blocks_to_alloc;
    // Used to track which blocks should be tested in MD in each PD stage
    MdScan* mds;
    // Used to store results of MD
    PC_TREE*         pc_tree;
    bool             copied_neigh_arrays;
    MvReferenceFrame ref_frame_type_arr[MODE_CTX_REF_FRAMES];
    uint8_t          tot_ref_frame_types;

    NeighborArrayUnit* recon_neigh_y;
    NeighborArrayUnit* recon_neigh_cb;
    NeighborArrayUnit* recon_neigh_cr;
    NeighborArrayUnit* tx_search_luma_recon_na;
    NeighborArrayUnit* luma_recon_na_16bit;
    NeighborArrayUnit* cb_recon_na_16bit;
    NeighborArrayUnit* cr_recon_na_16bit;
    NeighborArrayUnit* tx_search_luma_recon_na_16bit;
    // Stored per 4x4. 8 bit: lower 6 bits (COEFF_CONTEXT_BITS), shows if there is at least one
    // Coef. Top 2 bit store the sign of DC as follow: 0->0,1->-1,2-> 1
    NeighborArrayUnit* luma_dc_sign_level_coeff_na;
    // Stored per 4x4. 8 bit: lower 6 bits (COEFF_CONTEXT_BITS), shows if there is at least one
    // Coef. Top 2 bit store the sign of DC as follow: 0->0,1->-1,2-> 1
    NeighborArrayUnit* full_loop_luma_dc_sign_level_coeff_na;
    // Stored per 4x4. 8 bit: lower 6 bits(COEFF_CONTEXT_BITS), shows if there is at least one Coef.
    // Top 2 bit store the sign of DC as follow: 0->0,1->-1,2-> 1
    NeighborArrayUnit* cr_dc_sign_level_coeff_na;
    // Stored per 4x4. 8 bit: lower 6 bits(COEFF_CONTEXT_BITS), shows if there is at least one Coef.
    // Top 2 bit store the sign of DC as follow: 0->0,1->-1,2-> 1
    NeighborArrayUnit*    cb_dc_sign_level_coeff_na;
    NeighborArrayUnit*    txfm_context_array;
    NeighborArrayUnit*    leaf_partition_na;
    struct EncDecContext* ed_ctx;

    uint64_t* fast_cost_array;
    uint64_t* full_cost_array;
    uint64_t* full_cost_ssim_array;
    // Lambda
    uint32_t fast_lambda_md[2];
    uint32_t full_lambda_md[2];
    // for the case of lambda modulation (blk_lambda_tuning), full_lambda_md/fast_lambda_md
    // corresponds to block lambda and full_sb_lambda_md is the full lambda per sb
    uint32_t full_sb_lambda_md[2];
    bool     blk_lambda_tuning;
    // Context Variables---------------------------------
    SuperBlock*      sb_ptr;
    BlkStruct*       blk_ptr;
    const BlockGeom* blk_geom;
    // MD palette search
    PALETTE_BUFFER* palette_buffer;
    PaletteInfo*    palette_cand_array;
    uint8_t*        palette_size_array_0;
    // simple geometry 64x64SB, Sq only, no 4xN
    uint8_t          sb64_sq_no4xn_geom;
    uint32_t*        best_candidate_index_array;
    uint16_t         blk_org_x;
    uint16_t         blk_org_y;
    uint32_t         sb_origin_x;
    uint32_t         sb_origin_y;
    uint32_t         round_origin_x;
    uint32_t         round_origin_y;
    bool             has_uv;
    Part             shape;
    uint8_t          hbd_md;
    uint8_t          encoder_bit_depth;
    uint8_t          qp_index;
    uint8_t          me_q_index;
    uint64_t         three_quad_energy;
    uint32_t         txb_1d_offset;
    bool             uv_intra_comp_only;
    bool             ind_uv_avail; // True if independent chroma search data is available
    UvPredictionMode best_uv_mode[UV_PAETH_PRED + 1];
    int8_t           best_uv_angle[UV_PAETH_PRED + 1];
    uint64_t         best_uv_cost[UV_PAETH_PRED + 1];
    // Context values for rate estimation
    uint8_t is_inter_ctx;
    uint8_t skip_mode_ctx;
    uint8_t skip_coeff_ctx;
    uint8_t intra_luma_left_ctx;
    uint8_t intra_luma_top_ctx;

    WarpSampleInfo wm_sample_info[REF_FRAMES];
    int16_t*       pred_buf_q3;
    // Track all MVs that are prepared for candidates prior to MDS0. Used to avoid MV duplication.
    Mv** injected_mvs;
    // Track the reference types for each MV
    MvReferenceFrame* injected_ref_types;
    uint16_t          injected_mv_count;
    uint32_t          me_block_offset;
    uint32_t          me_cand_offset;
    // Pointer to a scratch buffer used by CFL & IFS
    EbPictureBufferDesc* scratch_prediction_ptr;
    uint8_t              tx_depth;
    uint8_t              txb_itr;
    uint32_t             me_sb_addr;
    uint32_t             geom_offset_x;
    uint32_t             geom_offset_y;
    int16_t              luma_txb_skip_context;
    int16_t              luma_dc_sign_context;
    int16_t              cb_txb_skip_context;
    int16_t              cb_dc_sign_context;
    int16_t              cr_txb_skip_context;
    int16_t              cr_dc_sign_context;
    // Multi-modes signal(s)
    uint8_t           global_mv_injection;
    uint8_t           new_nearest_injection;
    uint8_t           new_nearest_near_comb_injection;
    WmCtrls           wm_ctrls;
    UvCtrls           uv_ctrls;
    uint8_t           unipred3x3_injection;
    Bipred3x3Controls bipred3x3_ctrls;
    uint8_t           redundant_blk;
    uint8_t*          cfl_temp_luma_recon;
    uint16_t*         cfl_temp_luma_recon16bit;
    bool              blk_skip_decision;
    Mv                sb_me_mv[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX];
    // Store ME MV of the square to use with NSQ shapes; 4x4 will also use the 8x8 ME MVs
    Mv       sq_sb_me_mv[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX];
    Mv       fp_me_mv[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    Mv       sub_me_mv[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    uint32_t post_subpel_me_mv_cost[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    Mv       best_pme_mv[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX];
    int8_t   valid_pme_mv[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX];
    // Store MVP during MD search - only results are forwarded to encdec
    CandidateMv ref_mv_stack[MODE_CTX_REF_FRAMES][MAX_REF_MV_STACK_SIZE];
    // Store inter_mode_ctx for each reference during MD search - only ctx for winning ref frame is forwarded to encdec
    int16_t              inter_mode_ctx[MODE_CTX_REF_FRAMES];
    EbPictureBufferDesc* input_sample16bit_buffer;
    // set to 1 once the packing of 10bit source is done for each SB
    uint8_t  hbd_pack_done;
    uint16_t tile_index;
    // Store buffers for inter-inter compound search
    CompoundPredictionStore cmp_store;

    uint8_t*  pred0;
    uint8_t*  pred1;
    int16_t*  residual1;
    int16_t*  diff10;
    MdStage   md_stage;
    uint32_t* cand_buff_indices[CAND_CLASS_TOTAL];
    uint8_t   bypass_md_stage_1;
    uint8_t   bypass_md_stage_2;
    uint32_t  md_stage_0_count[CAND_CLASS_TOTAL];
    uint32_t  md_stage_1_count[CAND_CLASS_TOTAL];
    uint32_t  md_stage_2_count[CAND_CLASS_TOTAL];
    uint32_t  md_stage_3_count[CAND_CLASS_TOTAL];
    uint32_t  md_stage_1_total_count;
    uint32_t  md_stage_2_total_count;
    uint32_t  md_stage_3_total_count;
    CandClass target_class;
    uint8_t   perform_mds1;
    uint8_t   use_tx_shortcuts_mds3;
    uint8_t   lpd1_allow_skipping_tx;
    // Signals controlling which features are used at each MD stage
    bool mds_do_ifs;
    bool mds_do_txs;
    bool mds_do_txt;
    bool mds_do_rdoq;
    bool mds_do_spatial_sse;
    bool mds_do_chroma;
    // Store intra prediction for inter-intra
    uint8_t** intrapred_buf;
    // Store OBMC pre-computed data
    uint8_t*       obmc_buff_0;
    uint8_t*       obmc_buff_1;
    int32_t*       wsrc_buf;
    int32_t*       mask_buf;
    uint8_t*       above_txfm_context;
    uint8_t*       left_txfm_context;
    IntraCtrls     intra_ctrls;
    MdRateEstCtrls rate_est_ctrls;
    // use coeff rate and slipt flag rate only (no MVP derivation)
    uint8_t shut_fast_rate;
    // Control fast_coeff_est_level per mds
    uint8_t mds_fast_coeff_est_level;
    // Control subres_step per mds
    uint8_t              mds_subres_step;
    FilterIntraCtrls     filter_intra_ctrls;
    uint8_t              md_allow_intrabc;
    uint8_t              md_palette_level;
    DepthRemovalCtrls    depth_removal_ctrls;
    DepthRefinementCtrls depth_refinement_ctrls;
    SkipSubDepthCtrls    skip_sub_depth_ctrls;
#if !CLN_REMOVE_VAR_SUB_DEPTH
    VarSkipSubDepthCtrls var_skip_sub_depth_ctrls;
#endif
    SubresCtrls subres_ctrls;
    uint8_t     is_subres_safe;
    PfCtrls     pf_ctrls;
    // Control signals for MD sparse search (used for increasing ME search for active clips)
    MdSqMotionSearchCtrls  md_sq_me_ctrls;
    MdNsqMotionSearchCtrls md_nsq_me_ctrls;
    MdPmeCtrls             md_pme_ctrls;
    MdSubPelSearchCtrls    md_subpel_me_ctrls;
    MdSubPelSearchCtrls    md_subpel_pme_ctrls;
    PmeResults             pme_res[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    ObmcControls           obmc_ctrls;
    InterCompCtrls         inter_comp_ctrls;
    InterIntraCompCtrls    inter_intra_comp_ctrls;
    RefResults             ref_filtering_res[TOT_INTER_GROUP][MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    RefPruningControls     ref_pruning_ctrls;
    // Signal to control initial and final pass PD setting(s)
    PdPass               pd_pass;
    CflCtrls             cfl_ctrls;
    TxsControls          txs_ctrls;
    TxtControls          txt_ctrls;
    CandReductionCtrls   cand_reduction_ctrls;
    NsqGeomCtrls         nsq_geom_ctrls;
    NsqSearchCtrls       nsq_search_ctrls;
    DepthEarlyExitCtrls  depth_early_exit_ctrls;
    RdoqCtrls            rdoq_ctrls;
    uint8_t              disallow_8x8;
    uint8_t              disallow_4x4;
    uint8_t              md_disallow_nsq_search;
    uint8_t              params_status; // specifies the status of MD parameters; 0: default, 1: modified
    NsqPsqTxsCtrls       nsq_psq_txs_ctrls;
    uint8_t              sb_size;
    EbPictureBufferDesc* recon_coeff_ptr[TX_TYPES];
    EbPictureBufferDesc* recon_ptr[TX_TYPES];
    EbPictureBufferDesc* quant_coeff_ptr[TX_TYPES];
    // buffer used to store transformed coeffs during TX/Q/IQ. TX'd coeffs are only needed
    // temporarily, so no need to save for each TX type.
    EbPictureBufferDesc* tx_coeffs;

    uint8_t              skip_intra;
    EbPictureBufferDesc* temp_residual;
    EbPictureBufferDesc* temp_recon_ptr;
    // Array for all nearest/near MVs for a block for single ref case
    Mv mvp_array[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH][MAX_MVP_CANIDATES];
    // Count of all nearest/near MVs for a block for single ref case
    int8_t   mvp_count[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    uint16_t best_fp_mvp_idx[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    uint32_t best_fp_mvp_dist[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    uint32_t fp_me_dist[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    // Start/end position for MD sparse search
    int16_t         sprs_lev0_start_x;
    int16_t         sprs_lev0_end_x;
    int16_t         sprs_lev0_start_y;
    int16_t         sprs_lev0_end_y;
    NicCtrls        nic_ctrls;
    Mv              ref_mv;
    uint16_t        sb_index;
#if OPT_PER_BLK_INTRA
    bool mds0_use_hadamard_sb;
    bool mds0_use_hadamard_blk;
#else
    bool mds0_use_hadamard;
#endif
    uint8_t         max_block_size;
    uint64_t        mds0_best_cost_per_class[CAND_CLASS_TOTAL];
    uint64_t        mds0_best_cost;
    uint8_t         mds0_best_class;
    uint32_t        mds0_best_idx;
    CandClass       mds0_best_class_it;
    uint64_t        mds0_best_class0_cost;
    uint32_t        mds1_best_idx;
    CandClass       mds1_best_class_it;
    Mds0Ctrls       mds0_ctrls;
    uint32_t        md_me_dist;
    uint32_t        md_pme_dist;
    uint8_t         inject_new_me;
    uint8_t         inject_new_pme;
    TxShortcutCtrls tx_shortcut_ctrls;
    // [TOTAL_REFS_PER_FRAME + 1]
    uint64_t estimate_ref_frames_num_bits[MODE_CTX_REF_FRAMES];
    // Maximum number of candidates MD can support
    uint32_t max_nics;
    // Maximum number of candidates MD can support
    uint32_t                 max_nics_uv;
    InterpolationSearchCtrls ifs_ctrls;
    // If enabled, will bypass EncDec and copy recon/quant coeffs from MD
    bool bypass_encdec;
    // Indicates whether only pred depth refinement is used in PD1 (set per SB)
    bool pred_depth_only;
    // If true, indicates that there is a fixed partition structure for the current PD pass. When
    // the partition strucutre is fixed, there is no SQ/NSQ (d1) decision and no inter-depth (d2) decision.
    // When the partition structure in PD1 is fixed and EncDec is bypassed, the recon pic and QP info
    // can be written directly to final buffers instead of temporary buffers to be copied in EncDec.
    bool fixed_partition;
    // Indicates whether only pred depth refinement is used in PD1 (set per frame) Per frame is
    // necessary because some shortcuts can only be taken if the whole frame uses pred depth only
    bool     pic_pred_depth_only;
    uint16_t coded_area_sb;
    uint16_t coded_area_sb_uv;
    // Use source samples instead of reconstructed samples for INTRA prediction of PD0 in I_SLICE
    // to avoid inverse transform and neighbor array updates for reconstructed samples
    bool      lpd0_use_src_samples;
    Lpd0Ctrls lpd0_ctrls;
    // 0 : Use regular PD0 1 : Use light PD0 path. Assumes one class, no NSQ, no 4x4, TXT off, TXS
    // off, PME off, etc. 2 : Use very light PD0 path: only mds0 (no transform path), no
    // compensation(s) @ mds0 (only umpired candidates, and read directly from reference buffer(s)
    // to compute distortion(s) without accessing the pred buffer(s)), SSD as distortion metric(kept
    // the use of full lambda @ in-depth-exit and @ inter-depth), only split flag for
    // rate(coefficient(s) rate is assumed to be 0), use the 2nd PD1-level classifier (as the
    // regular PD1 classifier uses the number of non-zero coefficient(s)). 3: Skip pd0 if block size
    // is equal to or greater than 32x32
    Lpd1Ctrls lpd1_ctrls;
    // Limits minimum LDP1 level that can the detector can act on. This is meant to set a minimum LPD1
    // level that will be used (unless the set level is more conservative than pd1_lvl_refinement.
    // 0: off
    // 1: LPD1 detector will not act if LPD1 level is <= LPD1_LVL_0
    // 2: LPD1 detector will not act if LPD1 level is <= LPD1_LVL_1. If LPD1 level is >= LPD1_LVL_1, the min
    //    LPD1 level will be LPD1_LVL_1. If LPD1 is <= LPD1_LVL_0, then the detector will not apply and
    //    the set level will be used without the detector.
    uint8_t         pd1_lvl_refinement;
    SpatialSSECtrls spatial_sse_ctrls;

    uint16_t init_max_block_cnt;
    // set to true if MDS3 needs to perform a full 10bit compensation in MDS3 (to make MDS3
    // conformant when using bypass_encdec)
    uint8_t need_hbd_comp_mds3;
    // use approximate rate for inter cost (set at pic-level b/c some pic-level initializations will
    // be removed)
    // 0: off, 1: on, 2: on (more aggressive)
    uint8_t approx_inter_rate;
    // Enable pSad
    uint8_t enable_psad;
    // Bias the inter-depth decision cost in MD towards the parent block. This will scale the cost of the
    // parent depth by parent_cost_bias/1000. Values <1000 favour the  parent, while values >1000 favour
    // the child depth. 1000 means no bias.
    uint32_t    parent_cost_bias;
    uint8_t     is_intra_bordered;
    uint8_t     updated_enable_pme;
    Lpd1TxCtrls lpd1_tx_ctrls;
    // Indicates which chroma components (if any) are complex, relative to luma. Chroma TX shortcuts
    // based on luma should not be used when chroma is complex.
    uint8_t chroma_complexity;
    uint8_t cfl_complexity;
    // Signal to skip INTER TX in LPD1; should only be used by M13 as this causes blocking
    // artifacts. 0: OFF, 1: Skip INTER TX if neighs have 0 coeffs, 2: skip all INTER TX
    uint8_t lpd1_skip_inter_tx_level;
    // Specifies the threshold to bypass transform in LPD1 based on full cost estimate.
    // 0: OFF (no bypassing)
    // The higher the number, the more aggressive the feature is
    uint8_t lpd1_bypass_tx_th;
    // chroma components to compensate at MDS3 of LPD1
    COMPONENT_TYPE lpd1_chroma_comp;
    uint8_t        corrupted_mv_check;
    uint8_t        pred_mode_depth_refine;
    // when MD is done on 8bit, scale palette colors to 10bit (valid when bypass is 1)
    uint8_t  scale_palette;
    uint64_t rec_dist_per_quadrant[4];
    // non-normative txs
    uint16_t min_nz_h;
    uint16_t min_nz_v;
    // used to signal when the N4 shortcut can be used for rtc, works in conjunction with use_tx_shortcuts_mds3 flag
    uint8_t rtc_use_N4_dct_dct_shortcut;
    // SSIM_LVL_0: off
    // SSIM_LVL_1: use ssim cost to find best candidate in product_full_mode_decision()
    // SSIM_LVL_2: addition to level 1, also use ssim cost to find best tx type in tx_type_search()
    SsimLevel tune_ssim_level;
    // OBMC control signals (flags related to OBMC prediction readiness and bit depth)
    bool obmc_weighted_pred_ready; // Flag indicating if weighted prediction is prepared
    bool obmc_neighbor_luma_pred_ready; // Flag indicating if luma neighbor prediction is prepared
    bool obmc_neighbor_chroma_pred_ready; // Flag indicating if luma neighbor prediction is prepared
    bool obmc_is_luma_neigh_10bit; // Flag indicating if neighbor uses 10-bit data
} ModeDecisionContext;

/**************************************
 * Extern Function Declarations
 **************************************/
extern EbErrorType svt_aom_mode_decision_context_ctor(ModeDecisionContext* ctx, SequenceControlSet* scs,
                                                      EbColorFormat color_format, uint8_t sb_size, EncMode enc_mode,
                                                      uint16_t max_block_cnt, uint32_t encoder_bit_depth,
                                                      EbFifo* mode_decision_configuration_input_fifo_ptr,
                                                      EbFifo* mode_decision_output_fifo_ptr,
                                                      uint8_t enable_hbd_mode_decision, uint8_t seq_qp_mod);

// Table that converts 0-63 Q-range values passed in outside to the Qindex
// range used internally.
extern const uint8_t quantizer_to_qindex[64];

#define FIXED_QP_OFFSET_COUNT 6
extern const int percents[2][FIXED_QP_OFFSET_COUNT];

extern const uint8_t uni_psy_bias[64];

extern void svt_aom_reset_mode_decision(SequenceControlSet* scs, ModeDecisionContext* ctx, PictureControlSet* pcs,
                                        uint16_t tile_row_idx, uint32_t segment_index);

extern void svt_aom_mode_decision_configure_sb(ModeDecisionContext* ctx, PictureControlSet* pcs, uint8_t sb_qp,
                                               uint8_t me_sb_qp);
#if FTR_VLPD0
void svt_aom_get_blk_var_map(int block_size, int org_x, int org_y, int* blk_idx, int sub_idx[4]);
#endif
#ifdef __cplusplus
}
#endif
#endif // EbModeDecisionProcess_h
