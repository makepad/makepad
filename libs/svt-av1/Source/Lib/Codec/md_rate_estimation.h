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

#ifndef EbMdRateEstimation_h
#define EbMdRateEstimation_h

#include "cabac_context_model.h"
#include "pcs.h"
#include "coding_unit.h"
#ifdef __cplusplus
extern "C" {
#endif
/**************************************
     * MD Rate Estimation Defines
     **************************************/
#define MV_COST_WEIGHT_SUB 120

// Set to (1 << 5) if the 32-ary codebooks are used for any bock size
#define MAX_WEDGE_TYPES (1 << 4)
// The factor to scale from cost in bits to cost in av1_prob_cost units.
#define AV1_PROB_COST_SHIFT 9
// Cost of coding an n bit literal, using 128 (i.e. 50%) probability for each bit.
#define av1_cost_literal(n) ((n) * (1 << AV1_PROB_COST_SHIFT))

typedef struct LvMapEobCost {
    int32_t eob_cost[2][11];
} LvMapEobCost;

typedef struct LvMapCoeffCost {
    int32_t txb_skip_cost[TXB_SKIP_CONTEXTS][2];
    int32_t base_eob_cost[SIG_COEF_CONTEXTS_EOB][3];
    int32_t base_cost[SIG_COEF_CONTEXTS][8];
    int32_t eob_extra_cost[EOB_COEF_CONTEXTS][2];
    int32_t dc_sign_cost[DC_SIGN_CONTEXTS][2];
    int32_t lps_cost[LEVEL_CONTEXTS][COEFF_BASE_RANGE + 1 + COEFF_BASE_RANGE + 1];
} LvMapCoeffCost;

/**************************************
     * MD Rate Estimation Structure
     **************************************/
typedef struct MdRateEstimationContext {
    // Partition
    int32_t partition_fac_bits[PARTITION_CONTEXTS][EXT_PARTITION_TYPES];
    int32_t partition_vert_alike_fac_bits[PARTITION_CONTEXTS][2];
    int32_t partition_horz_alike_fac_bits[PARTITION_CONTEXTS][2];
    int32_t partition_vert_alike_128x128_fac_bits[PARTITION_CONTEXTS][2];
    int32_t partition_horz_alike_128x128_fac_bits[PARTITION_CONTEXTS][2];

    // MV Mode
    int32_t skip_mode_fac_bits[SKIP_CONTEXTS][2];
    int32_t new_mv_mode_fac_bits[NEWMV_MODE_CONTEXTS][2];
    int32_t zero_mv_mode_fac_bits[GLOBALMV_MODE_CONTEXTS][2];
    int32_t ref_mv_mode_fac_bits[REFMV_MODE_CONTEXTS][2];
    int32_t drl_mode_fac_bits[DRL_MODE_CONTEXTS][2];
    int32_t switchable_interp_fac_bitss[SWITCHABLE_FILTER_CONTEXTS][SWITCHABLE_FILTERS];
    int32_t motion_mode_fac_bits[BLOCK_SIZES_ALL][MOTION_MODES];
    int32_t motion_mode_fac_bits1[BLOCK_SIZES_ALL][2];

    int32_t        nmv_vec_cost[MV_JOINTS];
    int32_t        nmv_costs[2][MV_VALS];
    int32_t        nmv_costs_hp[2][MV_VALS];
    const int32_t* nmvcoststack[2];
    int32_t        dv_cost[2][MV_VALS];
    int32_t        dv_joint_cost[MV_JOINTS];

    // Compouned Mode
    int32_t inter_compound_mode_fac_bits[INTER_MODE_CONTEXTS][INTER_COMPOUND_MODES];
    int32_t compound_type_fac_bits[BLOCK_SIZES_ALL][MASKED_COMPOUND_TYPES];
    int32_t single_ref_fac_bits[REF_CONTEXTS][SINGLE_REFS - 1][2];
    int32_t comp_ref_type_fac_bits[COMP_REF_TYPE_CONTEXTS][2];
    int32_t uni_comp_ref_fac_bits[UNI_COMP_REF_CONTEXTS][UNIDIR_COMP_REFS - 1][2];
    int32_t comp_ref_fac_bits[REF_CONTEXTS][FWD_REFS - 1][2];
    int32_t comp_bwd_ref_fac_bits[REF_CONTEXTS][BWD_REFS - 1][2];
    int32_t comp_idx_fac_bits[COMP_INDEX_CONTEXTS][2];
    int32_t comp_group_idx_fac_bits[COMP_GROUP_IDX_CONTEXTS][2];
    int32_t comp_inter_fac_bits[COMP_INTER_CONTEXTS][2];

    // Wedge Mode
    int32_t wedge_idx_fac_bits[BLOCK_SIZES_ALL][16];
    int32_t inter_intra_fac_bits[BlockSize_GROUPS][2];
    int32_t wedge_inter_intra_fac_bits[BLOCK_SIZES_ALL][2];
    int32_t inter_intra_mode_fac_bits[BlockSize_GROUPS][INTERINTRA_MODES];

    // Intra Mode
    int32_t intrabc_fac_bits[2];
    int32_t intra_inter_fac_bits[INTRA_INTER_CONTEXTS][2];
    int32_t filter_intra_fac_bits[BLOCK_SIZES_ALL][2];
    int32_t filter_intra_mode_fac_bits[FILTER_INTRA_MODES];
    int32_t y_mode_fac_bits[KF_MODE_CONTEXTS][KF_MODE_CONTEXTS][INTRA_MODES];
    int32_t mb_mode_fac_bits[BlockSize_GROUPS][INTRA_MODES];
    int32_t intra_uv_mode_fac_bits[CFL_ALLOWED_TYPES][INTRA_MODES][UV_INTRA_MODES];
    int32_t angle_delta_fac_bits[DIRECTIONAL_MODES][2 * MAX_ANGLE_DELTA + 1];
    int32_t cfl_alpha_fac_bits[CFL_JOINT_SIGNS][CFL_PRED_PLANES][CFL_ALPHABET_SIZE];

    // Palette Mode
    int32_t palette_ysize_fac_bits[PALATTE_BSIZE_CTXS][PALETTE_SIZES];
    int32_t palette_uv_size_fac_bits[PALATTE_BSIZE_CTXS][PALETTE_SIZES];
    int32_t palette_ycolor_fac_bitss[PALETTE_SIZES][PALETTE_COLOR_INDEX_CONTEXTS][PALETTE_COLORS];
    int32_t palette_uv_color_fac_bits[PALETTE_SIZES][PALETTE_COLOR_INDEX_CONTEXTS][PALETTE_COLORS];
    int32_t palette_ymode_fac_bits[PALATTE_BSIZE_CTXS][PALETTE_Y_MODE_CONTEXTS][2];
    int32_t palette_uv_mode_fac_bits[PALETTE_UV_MODE_CONTEXTS][2];

    // Restoration filter weights
    int32_t switchable_restore_fac_bits[RESTORE_SWITCHABLE_TYPES];
    int32_t wiener_restore_fac_bits[2];
    int32_t sgrproj_restore_fac_bits[2];

    // Tx and Coeff Rate Estimation
    LvMapCoeffCost coeff_fac_bits[TX_SIZES][PLANE_TYPES];
    LvMapEobCost   eob_frac_bits[7][2];
    int32_t        txfm_partition_fac_bits[TXFM_PARTITION_CONTEXTS][2];
    int32_t        skip_fac_bits[SKIP_CONTEXTS][2];
    int32_t        tx_size_fac_bits[MAX_TX_CATS][TX_SIZE_CONTEXTS][MAX_TX_DEPTH + 1];
    int32_t        intra_tx_type_fac_bits[EXT_TX_SETS_INTRA][EXT_TX_SIZES][INTRA_MODES][TX_TYPES];
    int32_t        inter_tx_type_fac_bits[EXT_TX_SETS_INTER][EXT_TX_SIZES][TX_TYPES];
    bool           initialized;
} MdRateEstimationContext;

/***************************************************************************
    * AV1 Probability table
    * // round(-log2(i/256.) * (1 << AV1_PROB_COST_SHIFT)); i = 128~255.
    ***************************************************************************/
static const uint16_t av1_prob_cost[128] = {
    512, 506, 501, 495, 489, 484, 478, 473, 467, 462, 456, 451, 446, 441, 435, 430, 425, 420, 415, 410, 405, 400,
    395, 390, 385, 380, 375, 371, 366, 361, 356, 352, 347, 343, 338, 333, 329, 324, 320, 316, 311, 307, 302, 298,
    294, 289, 285, 281, 277, 273, 268, 264, 260, 256, 252, 248, 244, 240, 236, 232, 228, 224, 220, 216, 212, 209,
    205, 201, 197, 194, 190, 186, 182, 179, 175, 171, 168, 164, 161, 157, 153, 150, 146, 143, 139, 136, 132, 129,
    125, 122, 119, 115, 112, 109, 105, 102, 99,  95,  92,  89,  86,  82,  79,  76,  73,  70,  66,  63,  60,  57,
    54,  51,  48,  45,  42,  38,  35,  32,  29,  26,  23,  20,  18,  15,  12,  9,   6,   3,
};
static const int use_inter_ext_tx_for_txsize[EXT_TX_SETS_INTER][EXT_TX_SIZES] = {
    {1, 1, 1, 1}, // unused
    {1, 1, 0, 0},
    {0, 0, 1, 0},
    {0, 1, 1, 1},
};
static const int32_t use_intra_ext_tx_for_txsize[EXT_TX_SETS_INTRA][EXT_TX_SIZES] = {
    {1, 1, 1, 1}, // unused
    {1, 1, 0, 0},
    {0, 0, 1, 0},
};

static const int32_t av1_ext_tx_set_idx_to_type[2][AOMMAX(EXT_TX_SETS_INTRA, EXT_TX_SETS_INTER)] = {
    {
        // Intra
        EXT_TX_SET_DCTONLY,
        EXT_TX_SET_DTT4_IDTX_1DDCT,
        EXT_TX_SET_DTT4_IDTX,
    },
    {
        // Inter
        EXT_TX_SET_DCTONLY,
        EXT_TX_SET_ALL16,
        EXT_TX_SET_DTT9_IDTX_1DDCT,
        EXT_TX_SET_DCT_IDTX,
    },
};

/***************************************************************************
    * svt_aom_get_syntax_rate_from_cdf
    ***************************************************************************/
extern void svt_aom_get_syntax_rate_from_cdf(int32_t* costs, const AomCdfProb* cdf, const int32_t* inv_map);
/**************************************************************************
    * Estimate the rate for each syntax elements and for
    * all scenarios based on the frame CDF
    ***************************************************************************/
extern void svt_aom_estimate_syntax_rate(MdRateEstimationContext* md_rate_est_ctx, bool is_i_slice,
                                         uint8_t pic_filter_intra_level, uint8_t allow_screen_content_tools,
                                         uint8_t enable_restoration, uint8_t allow_intrabc, FRAME_CONTEXT* fc);
/**************************************************************************
    * Estimate the rate of the quantized coefficient
    * based on the frame CDF
    ***************************************************************************/
extern void svt_aom_estimate_coefficients_rate(MdRateEstimationContext* md_rate_est_ctx, FRAME_CONTEXT* fc);
/**************************************************************************
    * svt_aom_estimate_mv_rate()
    * Estimate the rate of motion vectors
    * based on the frame CDF
    ***************************************************************************/
void svt_aom_estimate_mv_rate(struct PictureControlSet* pcs, MdRateEstimationContext* md_rate_est_ctx,
                              FRAME_CONTEXT* fc);

/*******************************************************************************
 * Updates all the syntax stats/CDF for the current block
 ******************************************************************************/
void svt_aom_update_stats(struct PictureControlSet* pcs, struct BlkStruct* blk_ptr, int mi_row, int mi_col);
/*******************************************************************************
 * Updates the partition stats/CDF for the current block
 ******************************************************************************/
void svt_aom_update_part_stats(struct PictureControlSet* pcs, const PartitionType partition, const BlockSize bsize,
                               const uint16_t tile_idx, const uint32_t sb_index, const int mi_row, const int mi_col);

/*
* Returns the me-based qindex (used for lambda modulation only; not at Q/Q-1)
*/
uint8_t svt_aom_get_me_qindex(struct PictureControlSet* pcs, struct SuperBlock* sb_ptr, uint8_t is_sb128);

#ifdef __cplusplus
}
#endif

#endif //EbMdRateEstimationTables_h
