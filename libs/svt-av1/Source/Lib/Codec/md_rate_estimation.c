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

#include <stdlib.h>

#include "md_rate_estimation.h"
#include "common_utils.h"
#include "filter.h"
#include "entropy_coding.h"
#include "bitstream_unit.h"
#include "rd_cost.h"
#include "inter_prediction.h"

static INLINE int32_t get_interinter_wedge_bits(BlockSize bsize) {
    const int32_t wbits = svt_aom_get_wedge_params_bits(bsize);
    return (wbits > 0) ? wbits + 1 : 0;
}

/**************************************************************
* AV1GetCostSymbold
* Calculate the cost of a symbol with
* probability p15 / 2^15
***************************************************************/
static INLINE int32_t av1_cost_symbol(AomCdfProb p15) {
    // p15 can be out of range [1, CDF_PROB_TOP - 1]. Clamping it, so that the
    // following cost calculation works correctly. Otherwise, if p15 =
    // CDF_PROB_TOP, shift would be -1, and "p15 << shift" would be wrong.
    p15 = (AomCdfProb)clamp(p15, 1, CDF_PROB_TOP - 1);
    assert(0 < p15 && p15 < CDF_PROB_TOP);
    const int32_t shift = CDF_PROB_BITS - 1 - get_msb(p15);
    const int32_t prob  = get_prob(p15 << shift, CDF_PROB_TOP);
    assert(prob >= 128);
    return av1_prob_cost[prob - 128] + av1_cost_literal(shift);
}

/*************************************************************
* svt_aom_get_syntax_rate_from_cdf
**************************************************************/
void svt_aom_get_syntax_rate_from_cdf(int32_t* costs, const AomCdfProb* cdf, const int32_t* inv_map) {
    int32_t    i;
    AomCdfProb prev_cdf = 0;
    for (i = 0;; ++i) {
        AomCdfProb p15 = AOM_ICDF(cdf[i]) - prev_cdf;
        p15            = (p15 < EC_MIN_PROB) ? EC_MIN_PROB : p15;
        prev_cdf       = AOM_ICDF(cdf[i]);

        if (inv_map) {
            costs[inv_map[i]] = av1_cost_symbol(p15);
        } else {
            costs[i] = av1_cost_symbol(p15);
        }

        // Stop once we reach the end of the CDF
        if (cdf[i] == AOM_ICDF(CDF_PROB_TOP)) {
            break;
        }
    }
}

/*************************************************************
 * svt_aom_estimate_syntax_rate()
 * Estimate the rate for each syntax elements and for
 * all scenarios based on the frame CDF
 **************************************************************/
void svt_aom_estimate_syntax_rate(MdRateEstimationContext* md_rate_est_ctx, bool is_i_slice,
                                  uint8_t pic_filter_intra_level, uint8_t allow_screen_content_tools,
                                  uint8_t enable_restoration, uint8_t allow_intrabc, FRAME_CONTEXT* fc) {
    int32_t i, j;

    md_rate_est_ctx->initialized = 1;
    for (i = 0; i < PARTITION_CONTEXTS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->partition_fac_bits[i], fc->partition_cdf[i], NULL);

        AomCdfProb cdf[CDF_SIZE(2)];
        // The cdf will be updated differently for BLOCK_128X128 vs. all other blocks sizes.
        // therefore, we must compute the syntax rate for two cases: 128x128 blocks and all other
        // blocks.

        // Vert alike rate (128x128 and all other blocks)
        partition_gather_vert_alike(cdf, fc->partition_cdf[i], BLOCK_16X16);
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->partition_vert_alike_fac_bits[i], cdf, NULL);
        /* If the first entry of the cdf is 0, then svt_aom_get_syntax_rate_from_cdf will exit after deriving
        the rate of the first entry.  In that case, the rate of the second entry would not be initialized, and
        could cause a r2r. In this case, we set the second entry of the rate table to 6656 (cost of EC_MIN_PROB),
        which is the value which would be set if svt_aom_get_syntax_rate_from_cdf did not exit after the first 0 entry. */
        if (cdf[0] == 0) {
            md_rate_est_ctx->partition_vert_alike_fac_bits[i][1] = av1_cost_symbol(EC_MIN_PROB);
        }

        partition_gather_vert_alike(cdf, fc->partition_cdf[i], BLOCK_128X128);
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->partition_vert_alike_128x128_fac_bits[i], cdf, NULL);
        /* If the first entry of the cdf is 0, then svt_aom_get_syntax_rate_from_cdf will exit after deriving
        the rate of the first entry.  In that case, the rate of the second entry would not be initialized, and
        could cause a r2r. In this case, we set the second entry of the rate table to 6656 (cost of EC_MIN_PROB),
        which is the value which would be set if svt_aom_get_syntax_rate_from_cdf did not exit after the first 0 entry. */
        if (cdf[0] == 0) {
            md_rate_est_ctx->partition_vert_alike_128x128_fac_bits[i][1] = av1_cost_symbol(EC_MIN_PROB);
        }

        // Horz alike rate (128x128 and all other blocks)
        partition_gather_horz_alike(cdf, fc->partition_cdf[i], BLOCK_16X16);
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->partition_horz_alike_fac_bits[i], cdf, NULL);
        /* If the first entry of the cdf is 0, then svt_aom_get_syntax_rate_from_cdf will exit after deriving
        the rate of the first entry.  In that case, the rate of the second entry would not be initialized, and
        could cause a r2r. In this case, we set the second entry of the rate table to 6656 (cost of EC_MIN_PROB),
        which is the value which would be set if svt_aom_get_syntax_rate_from_cdf did not exit after the first 0 entry. */
        if (cdf[0] == 0) {
            md_rate_est_ctx->partition_horz_alike_fac_bits[i][1] = av1_cost_symbol(EC_MIN_PROB);
        }

        partition_gather_horz_alike(cdf, fc->partition_cdf[i], BLOCK_128X128);
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->partition_horz_alike_128x128_fac_bits[i], cdf, NULL);
        /* If the first entry of the cdf is 0, then svt_aom_get_syntax_rate_from_cdf will exit after deriving
        the rate of the first entry.  In that case, the rate of the second entry would not be initialized, and
        could cause a r2r. In this case, we set the second entry of the rate table to 6656 (cost of EC_MIN_PROB),
        which is the value which would be set if svt_aom_get_syntax_rate_from_cdf did not exit after the first 0 entry. */
        if (cdf[0] == 0) {
            md_rate_est_ctx->partition_horz_alike_128x128_fac_bits[i][1] = av1_cost_symbol(EC_MIN_PROB);
        }
    }

    for (i = 0; i < SKIP_CONTEXTS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->skip_mode_fac_bits[i], fc->skip_mode_cdfs[i], NULL);
    }

    for (i = 0; i < SKIP_CONTEXTS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->skip_fac_bits[i], fc->skip_cdfs[i], NULL);
    }
    for (i = 0; i < KF_MODE_CONTEXTS; ++i) {
        for (j = 0; j < KF_MODE_CONTEXTS; ++j) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->y_mode_fac_bits[i][j], fc->kf_y_cdf[i][j], NULL);
        }
    }

    for (i = 0; i < BlockSize_GROUPS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->mb_mode_fac_bits[i], fc->y_mode_cdf[i], NULL);
    }

    for (i = 0; i < CFL_ALLOWED_TYPES; ++i) {
        for (j = 0; j < INTRA_MODES; ++j) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->intra_uv_mode_fac_bits[i][j], fc->uv_mode_cdf[i][j], NULL);
        }
    }
    if (pic_filter_intra_level) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->filter_intra_mode_fac_bits, fc->filter_intra_mode_cdf, NULL);
        for (i = 0; i < BLOCK_SIZES_ALL; ++i) {
            if (svt_aom_filter_intra_allowed_bsize(i)) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->filter_intra_fac_bits[i], fc->filter_intra_cdfs[i], NULL);
            }
        }
    }
    for (i = 0; i < SWITCHABLE_FILTER_CONTEXTS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(
            md_rate_est_ctx->switchable_interp_fac_bitss[i], fc->switchable_interp_cdf[i], NULL);
    }
    if (allow_screen_content_tools) {
        for (i = 0; i < PALATTE_BSIZE_CTXS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->palette_ysize_fac_bits[i], fc->palette_y_size_cdf[i], NULL);
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->palette_uv_size_fac_bits[i], fc->palette_uv_size_cdf[i], NULL);
            for (j = 0; j < PALETTE_Y_MODE_CONTEXTS; ++j) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->palette_ymode_fac_bits[i][j], fc->palette_y_mode_cdf[i][j], NULL);
            }
        }

        for (i = 0; i < PALETTE_UV_MODE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->palette_uv_mode_fac_bits[i], fc->palette_uv_mode_cdf[i], NULL);
        }
        for (i = 0; i < PALETTE_SIZES; ++i) {
            for (j = 0; j < PALETTE_COLOR_INDEX_CONTEXTS; ++j) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->palette_ycolor_fac_bitss[i][j], fc->palette_y_color_index_cdf[i][j], NULL);
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->palette_uv_color_fac_bits[i][j], fc->palette_uv_color_index_cdf[i][j], NULL);
            }
        }
    }
    int32_t sign_fac_bits[CFL_JOINT_SIGNS];
    svt_aom_get_syntax_rate_from_cdf(sign_fac_bits, fc->cfl_sign_cdf, NULL);
    for (int32_t joint_sign = 0; joint_sign < CFL_JOINT_SIGNS; joint_sign++) {
        int32_t* fac_bits_u = md_rate_est_ctx->cfl_alpha_fac_bits[joint_sign][CFL_PRED_U];
        int32_t* fac_bits_v = md_rate_est_ctx->cfl_alpha_fac_bits[joint_sign][CFL_PRED_V];
        if (CFL_SIGN_U(joint_sign) == CFL_SIGN_ZERO) {
            memset(fac_bits_u, 0, CFL_ALPHABET_SIZE * sizeof(*fac_bits_u));
        } else {
            const AomCdfProb* cdf_u = fc->cfl_alpha_cdf[CFL_CONTEXT_U(joint_sign)];
            svt_aom_get_syntax_rate_from_cdf(fac_bits_u, cdf_u, NULL);
        }
        if (CFL_SIGN_V(joint_sign) == CFL_SIGN_ZERO) {
            memset(fac_bits_v, 0, CFL_ALPHABET_SIZE * sizeof(*fac_bits_v));
        } else {
            assert((CFL_CONTEXT_V(joint_sign) < CFL_ALPHA_CONTEXTS) && (CFL_CONTEXT_V(joint_sign) >= 0));
            const AomCdfProb* cdf_v = fc->cfl_alpha_cdf[CFL_CONTEXT_V(joint_sign)];
            svt_aom_get_syntax_rate_from_cdf(fac_bits_v, cdf_v, NULL);
        }
        for (int32_t u = 0; u < CFL_ALPHABET_SIZE; u++) {
            fac_bits_u[u] += sign_fac_bits[joint_sign];
        }
    }

    for (i = 0; i < MAX_TX_CATS; ++i) {
        for (j = 0; j < TX_SIZE_CONTEXTS; ++j) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->tx_size_fac_bits[i][j], fc->tx_size_cdf[i][j], NULL);
        }
    }

    for (i = 0; i < TXFM_PARTITION_CONTEXTS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->txfm_partition_fac_bits[i], fc->txfm_partition_cdf[i], NULL);
    }

    for (i = TX_4X4; i < EXT_TX_SIZES; ++i) {
        int32_t s;
        for (s = 1; s < EXT_TX_SETS_INTER; ++s) {
            if (use_inter_ext_tx_for_txsize[s][i]) {
                svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->inter_tx_type_fac_bits[s][i],
                                                 fc->inter_ext_tx_cdf[s][i],
                                                 av1_ext_tx_inv[av1_ext_tx_set_idx_to_type[1][s]]);
            }
        }
        for (s = 1; s < EXT_TX_SETS_INTRA; ++s) {
            if (use_intra_ext_tx_for_txsize[s][i]) {
                for (j = 0; j < INTRA_MODES; ++j) {
                    svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->intra_tx_type_fac_bits[s][i][j],
                                                     fc->intra_ext_tx_cdf[s][i][j],
                                                     av1_ext_tx_inv[av1_ext_tx_set_idx_to_type[0][s]]);
                }
            }
        }
    }
    for (i = 0; i < DIRECTIONAL_MODES; ++i) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->angle_delta_fac_bits[i], fc->angle_delta_cdf[i], NULL);
    }
    if (enable_restoration) {
        svt_aom_get_syntax_rate_from_cdf(
            md_rate_est_ctx->switchable_restore_fac_bits, fc->switchable_restore_cdf, NULL);
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->wiener_restore_fac_bits, fc->wiener_restore_cdf, NULL);
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->sgrproj_restore_fac_bits, fc->sgrproj_restore_cdf, NULL);
    }
    if (allow_intrabc) {
        svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->intrabc_fac_bits, fc->intrabc_cdf, NULL);
    }

    if (!is_i_slice) { // NM - Hardcoded to true
        for (i = 0; i < COMP_INTER_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->comp_inter_fac_bits[i], fc->comp_inter_cdf[i], NULL);
        }
        for (i = 0; i < REF_CONTEXTS; ++i) {
            for (j = 0; j < SINGLE_REFS - 1; ++j) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->single_ref_fac_bits[i][j], fc->single_ref_cdf[i][j], NULL);
            }
        }

        for (i = 0; i < COMP_REF_TYPE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->comp_ref_type_fac_bits[i], fc->comp_ref_type_cdf[i], NULL);
        }
        for (i = 0; i < UNI_COMP_REF_CONTEXTS; ++i) {
            for (j = 0; j < UNIDIR_COMP_REFS - 1; ++j) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->uni_comp_ref_fac_bits[i][j], fc->uni_comp_ref_cdf[i][j], NULL);
            }
        }

        for (i = 0; i < REF_CONTEXTS; ++i) {
            for (j = 0; j < FWD_REFS - 1; ++j) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->comp_ref_fac_bits[i][j], fc->comp_ref_cdf[i][j], NULL);
            }
        }

        for (i = 0; i < REF_CONTEXTS; ++i) {
            for (j = 0; j < BWD_REFS - 1; ++j) {
                svt_aom_get_syntax_rate_from_cdf(
                    md_rate_est_ctx->comp_bwd_ref_fac_bits[i][j], fc->comp_bwdref_cdf[i][j], NULL);
            }
        }

        for (i = 0; i < INTRA_INTER_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->intra_inter_fac_bits[i], fc->intra_inter_cdf[i], NULL);
        }
        for (i = 0; i < NEWMV_MODE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->new_mv_mode_fac_bits[i], fc->newmv_cdf[i], NULL);
        }
        for (i = 0; i < GLOBALMV_MODE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->zero_mv_mode_fac_bits[i], fc->zeromv_cdf[i], NULL);
        }
        for (i = 0; i < REFMV_MODE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->ref_mv_mode_fac_bits[i], fc->refmv_cdf[i], NULL);
        }
        for (i = 0; i < DRL_MODE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->drl_mode_fac_bits[i], fc->drl_cdf[i], NULL);
        }
        for (i = 0; i < INTER_MODE_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->inter_compound_mode_fac_bits[i], fc->inter_compound_mode_cdf[i], NULL);
        }
        for (i = 0; i < BLOCK_SIZES_ALL; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->compound_type_fac_bits[i], fc->compound_type_cdf[i], NULL);
        }
        for (i = 0; i < BLOCK_SIZES_ALL; ++i) {
            if (get_interinter_wedge_bits((BlockSize)i)) {
                svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->wedge_idx_fac_bits[i], fc->wedge_idx_cdf[i], NULL);
            }
        }
        for (i = 0; i < BlockSize_GROUPS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->inter_intra_fac_bits[i], fc->interintra_cdf[i], NULL);
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->inter_intra_mode_fac_bits[i], fc->interintra_mode_cdf[i], NULL);
        }
        for (i = 0; i < BLOCK_SIZES_ALL; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->wedge_inter_intra_fac_bits[i], fc->wedge_interintra_cdf[i], NULL);
        }
        for (i = BLOCK_8X8; i < BLOCK_SIZES_ALL; i++) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->motion_mode_fac_bits[i], fc->motion_mode_cdf[i], NULL);
        }
        for (i = BLOCK_8X8; i < BLOCK_SIZES_ALL; i++) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->motion_mode_fac_bits1[i], fc->obmc_cdf[i], NULL);
        }
        for (i = 0; i < COMP_INDEX_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(md_rate_est_ctx->comp_idx_fac_bits[i], fc->compound_index_cdf[i], NULL);
        }
        for (i = 0; i < COMP_GROUP_IDX_CONTEXTS; ++i) {
            svt_aom_get_syntax_rate_from_cdf(
                md_rate_est_ctx->comp_group_idx_fac_bits[i], fc->comp_group_idx_cdf[i], NULL);
        }
    }
}

static const uint8_t log_in_base_2[] = {
    0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 10};

static INLINE int32_t mv_class_base(MvClassType c) {
    return c ? CLASS0_SIZE << (c + 2) : 0;
}

MvClassType svt_av1_get_mv_class(int32_t z, int32_t* offset) {
    const MvClassType c = (z >= CLASS0_SIZE * 4096) ? MV_CLASS_10 : (MvClassType)log_in_base_2[z >> 3];
    if (offset) {
        *offset = z - mv_class_base(c);
    }
    return c;
}

static void build_nmv_component_cost_table(int32_t* mvcost, const NmvComponent* const mvcomp,
                                           MvSubpelPrecision precision) {
    int32_t i, v;
    int32_t sign_cost[2], class_cost[MV_CLASSES], class0_cost[CLASS0_SIZE];
    int32_t bits_cost[MV_OFFSET_BITS][2];
    int32_t class0_fp_cost[CLASS0_SIZE][MV_FP_SIZE], fp_cost[MV_FP_SIZE];
    int32_t class0_hp_cost[2], hp_cost[2];

    svt_aom_get_syntax_rate_from_cdf(sign_cost, mvcomp->sign_cdf, NULL);
    svt_aom_get_syntax_rate_from_cdf(class_cost, mvcomp->classes_cdf, NULL);
    svt_aom_get_syntax_rate_from_cdf(class0_cost, mvcomp->class0_cdf, NULL);
    for (i = 0; i < MV_OFFSET_BITS; ++i) {
        svt_aom_get_syntax_rate_from_cdf(bits_cost[i], mvcomp->bits_cdf[i], NULL);
    }
    for (i = 0; i < CLASS0_SIZE; ++i) {
        svt_aom_get_syntax_rate_from_cdf(class0_fp_cost[i], mvcomp->class0_fp_cdf[i], NULL);
    }
    svt_aom_get_syntax_rate_from_cdf(fp_cost, mvcomp->fp_cdf, NULL);

    if (precision > MV_SUBPEL_LOW_PRECISION) {
        svt_aom_get_syntax_rate_from_cdf(class0_hp_cost, mvcomp->class0_hp_cdf, NULL);
        svt_aom_get_syntax_rate_from_cdf(hp_cost, mvcomp->hp_cdf, NULL);
    }
    mvcost[0] = 0;
    for (v = 1; v <= MV_MAX; ++v) {
        int32_t z, c, o, d, e, f, cost = 0;
        z = v - 1;
        c = svt_av1_get_mv_class(z, &o);
        cost += class_cost[c];
        d = (o >> 3); /* int32_t mv data */
        f = (o >> 1) & 3; /* fractional pel mv data */
        e = (o & 1); /* high precision mv data */
        if (c == MV_CLASS_0) {
            cost += class0_cost[d];
        } else {
            const int32_t b = c + CLASS0_BITS - 1; /* number of bits */
            for (i = 0; i < b; ++i) {
                cost += bits_cost[i][((d >> i) & 1)];
            }
        }
        if (precision > MV_SUBPEL_NONE) {
            if (c == MV_CLASS_0) {
                cost += class0_fp_cost[d][f];
            } else {
                cost += fp_cost[f];
            }
            if (precision > MV_SUBPEL_LOW_PRECISION) {
                if (c == MV_CLASS_0) {
                    cost += class0_hp_cost[e];
                } else {
                    cost += hp_cost[e];
                }
            }
        }
        mvcost[v]  = cost + sign_cost[0];
        mvcost[-v] = cost + sign_cost[1];
    }
}

static void svt_av1_build_nmv_cost_table(int32_t* mvjoint, int32_t* mvcost[2], const NmvContext* ctx,
                                         MvSubpelPrecision precision) {
    svt_aom_get_syntax_rate_from_cdf(mvjoint, ctx->joints_cdf, NULL);
    build_nmv_component_cost_table(mvcost[0], &ctx->comps[0], precision);
    build_nmv_component_cost_table(mvcost[1], &ctx->comps[1], precision);
}

/**************************************************************************
 * svt_aom_estimate_mv_rate()
 * Estimate the rate of motion vectors
 * based on the frame CDF
 ***************************************************************************/
void svt_aom_estimate_mv_rate(PictureControlSet* pcs, MdRateEstimationContext* md_rate_est_ctx, FRAME_CONTEXT* fc) {
    if (pcs->approx_inter_rate) {
        memset(md_rate_est_ctx->nmv_vec_cost, 0, sizeof(int32_t) * MV_JOINTS);
        memset(md_rate_est_ctx->nmv_costs, 0, sizeof(int32_t) * MV_VALS * 2);
        md_rate_est_ctx->nmvcoststack[0] = &md_rate_est_ctx->nmv_costs[0][MV_MAX];
        md_rate_est_ctx->nmvcoststack[1] = &md_rate_est_ctx->nmv_costs[1][MV_MAX];
        return;
    }
    int32_t*     nmvcost[2];
    int32_t*     nmvcost_hp[2];
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;

    nmvcost[0]                            = &md_rate_est_ctx->nmv_costs[0][MV_MAX];
    nmvcost[1]                            = &md_rate_est_ctx->nmv_costs[1][MV_MAX];
    nmvcost_hp[0]                         = &md_rate_est_ctx->nmv_costs_hp[0][MV_MAX];
    nmvcost_hp[1]                         = &md_rate_est_ctx->nmv_costs_hp[1][MV_MAX];
    const uint8_t allow_high_precision_mv = frm_hdr->allow_high_precision_mv;
    svt_av1_build_nmv_cost_table(md_rate_est_ctx->nmv_vec_cost, // out
                                 allow_high_precision_mv ? nmvcost_hp : nmvcost, // out
                                 &fc->nmvc,
                                 allow_high_precision_mv);
    md_rate_est_ctx->nmvcoststack[0] = allow_high_precision_mv ? &md_rate_est_ctx->nmv_costs_hp[0][MV_MAX]
                                                               : &md_rate_est_ctx->nmv_costs[0][MV_MAX];
    md_rate_est_ctx->nmvcoststack[1] = allow_high_precision_mv ? &md_rate_est_ctx->nmv_costs_hp[1][MV_MAX]
                                                               : &md_rate_est_ctx->nmv_costs[1][MV_MAX];

    if (frm_hdr->allow_intrabc) {
        int32_t* dvcost[2] = {&md_rate_est_ctx->dv_cost[0][MV_MAX], &md_rate_est_ctx->dv_cost[1][MV_MAX]};
        svt_av1_build_nmv_cost_table(md_rate_est_ctx->dv_joint_cost, dvcost, &fc->ndvc, MV_SUBPEL_NONE);
    }
}

/**************************************************************************
 * svt_aom_estimate_coefficients_rate()
 * Estimate the rate of the quantized coefficient
 * based on the frame CDF
 ***************************************************************************/
void svt_aom_estimate_coefficients_rate(MdRateEstimationContext* md_rate_est_ctx, FRAME_CONTEXT* fc) {
    const int32_t num_planes = 3; // NM - Hardcoded to 3
    const int32_t nplanes    = AOMMIN(num_planes, PLANE_TYPES);

    for (int eob_multi_size = 0; eob_multi_size < 7; ++eob_multi_size) {
        for (int plane = 0; plane < nplanes; ++plane) {
            LvMapEobCost* pcost = &md_rate_est_ctx->eob_frac_bits[eob_multi_size][plane];
            for (int ctx = 0; ctx < 2; ++ctx) {
                AomCdfProb* pcdf;
                switch (eob_multi_size) {
                case 0:
                    pcdf = fc->eob_flag_cdf16[plane][ctx];
                    break;
                case 1:
                    pcdf = fc->eob_flag_cdf32[plane][ctx];
                    break;
                case 2:
                    pcdf = fc->eob_flag_cdf64[plane][ctx];
                    break;
                case 3:
                    pcdf = fc->eob_flag_cdf128[plane][ctx];
                    break;
                case 4:
                    pcdf = fc->eob_flag_cdf256[plane][ctx];
                    break;
                case 5:
                    pcdf = fc->eob_flag_cdf512[plane][ctx];
                    break;
                case 6:
                default:
                    pcdf = fc->eob_flag_cdf1024[plane][ctx];
                    break;
                }
                svt_aom_get_syntax_rate_from_cdf(pcost->eob_cost[ctx], pcdf, NULL);
            }
        }
    }
    for (int tx_size = 0; tx_size < TX_SIZES; ++tx_size) {
        for (int plane = 0; plane < nplanes; ++plane) {
            LvMapCoeffCost* pcost = &md_rate_est_ctx->coeff_fac_bits[tx_size][plane];

            for (int ctx = 0; ctx < TXB_SKIP_CONTEXTS; ++ctx) {
                svt_aom_get_syntax_rate_from_cdf(pcost->txb_skip_cost[ctx], fc->txb_skip_cdf[tx_size][ctx], NULL);
            }

            for (int ctx = 0; ctx < SIG_COEF_CONTEXTS_EOB; ++ctx) {
                svt_aom_get_syntax_rate_from_cdf(
                    pcost->base_eob_cost[ctx], fc->coeff_base_eob_cdf[tx_size][plane][ctx], NULL);
            }
            for (int ctx = 0; ctx < SIG_COEF_CONTEXTS; ++ctx) {
                svt_aom_get_syntax_rate_from_cdf(pcost->base_cost[ctx], fc->coeff_base_cdf[tx_size][plane][ctx], NULL);
            }
            for (int ctx = 0; ctx < SIG_COEF_CONTEXTS; ++ctx) {
                pcost->base_cost[ctx][4] = 0;
                pcost->base_cost[ctx][5] = pcost->base_cost[ctx][1] + av1_cost_literal(1) - pcost->base_cost[ctx][0];
                pcost->base_cost[ctx][6] = pcost->base_cost[ctx][2] - pcost->base_cost[ctx][1];
                pcost->base_cost[ctx][7] = pcost->base_cost[ctx][3] - pcost->base_cost[ctx][2];
            }
            for (int ctx = 0; ctx < EOB_COEF_CONTEXTS; ++ctx) {
                svt_aom_get_syntax_rate_from_cdf(
                    pcost->eob_extra_cost[ctx], fc->eob_extra_cdf[tx_size][plane][ctx], NULL);
            }

            for (int ctx = 0; ctx < DC_SIGN_CONTEXTS; ++ctx) {
                svt_aom_get_syntax_rate_from_cdf(pcost->dc_sign_cost[ctx], fc->dc_sign_cdf[plane][ctx], NULL);
            }

            for (int ctx = 0; ctx < LEVEL_CONTEXTS; ++ctx) {
                int32_t br_rate[BR_CDF_SIZE];
                int32_t prev_cost = 0;
                int32_t i, j;
                svt_aom_get_syntax_rate_from_cdf(
                    br_rate, fc->coeff_br_cdf[AOMMIN(tx_size, TX_32X32)][plane][ctx], NULL);
                // SVT_LOG("br_rate: ");
                // for(j = 0; j < BR_CDF_SIZE; j++)
                //  SVT_LOG("%4d ", br_rate[j]);
                // SVT_LOG("\n");
                for (i = 0; i < COEFF_BASE_RANGE; i += BR_CDF_SIZE - 1) {
                    for (j = 0; j < BR_CDF_SIZE - 1; j++) {
                        pcost->lps_cost[ctx][i + j] = prev_cost + br_rate[j];
                    }
                    prev_cost += br_rate[j];
                }
                pcost->lps_cost[ctx][i] = prev_cost;
                // SVT_LOG("lps_cost: %d %d %2d : ", tx_size, plane, ctx);
                // for (i = 0; i <= COEFF_BASE_RANGE; i++)
                //  SVT_LOG("%5d ", pcost->lps_cost[ctx][i]);
                // SVT_LOG("\n");
            }
            for (int ctx = 0; ctx < LEVEL_CONTEXTS; ++ctx) {
                pcost->lps_cost[ctx][0 + COEFF_BASE_RANGE + 1] = pcost->lps_cost[ctx][0];
                for (int i = 1; i <= COEFF_BASE_RANGE; ++i) {
                    pcost->lps_cost[ctx][i + COEFF_BASE_RANGE + 1] = pcost->lps_cost[ctx][i] -
                        pcost->lps_cost[ctx][i - 1];
                }
            }
        }
    }
}

static INLINE AomCdfProb* get_y_mode_cdf(FRAME_CONTEXT* tile_ctx, const MacroBlockD* xd) {
    uint8_t above_ctx, left_ctx;
    svt_aom_get_kf_y_mode_ctx(xd, &above_ctx, &left_ctx);
    return tile_ctx->kf_y_cdf[above_ctx][left_ctx];
}

AomCdfProb* svt_aom_get_reference_mode_cdf(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_comp_reference_type_cdf(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p2(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p2(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_bwdref_p(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_comp_bwdref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p1(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p2(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p3(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p4(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p5(const MacroBlockD* xd);
AomCdfProb* svt_aom_get_pred_cdf_single_ref_p6(const MacroBlockD* xd);
int         svt_aom_is_interintra_allowed_bsize(const BlockSize bsize);
int         svt_aom_is_interintra_allowed_mode(const PredictionMode mode);
int         svt_aom_is_interintra_allowed_ref(const MvReferenceFrame rf[2]);
int         svt_aom_is_interintra_allowed(const MbModeInfo* mbmi);
int         svt_aom_get_comp_group_idx_context_enc(const MacroBlockD* xd);

int svt_aom_allow_intrabc(const FrameHeader* frm_hdr, SliceType slice_type);

int svt_aom_allow_palette(int allow_screen_content_tools, BlockSize bsize);

int svt_aom_get_palette_bsize_ctx(BlockSize bsize);

int svt_aom_get_palette_mode_ctx(const MacroBlockD* xd);

int32_t svt_aom_partition_cdf_length(BlockSize bsize);

/*******************************************************************************
 * Updates all the filter type stats/CDF for the current block
 ******************************************************************************/
static AOM_INLINE void update_filter_type_cdf(MacroBlockD* xd, const MbModeInfo* const mbmi,
                                              const bool enable_dual_filter) {
    const int max_dir = enable_dual_filter ? 2 : 1;
    for (int dir = 0; dir < max_dir; ++dir) {
        const int ctx = svt_aom_get_pred_context_switchable_interp(
            mbmi->block_mi.ref_frame[0], mbmi->block_mi.ref_frame[1], xd, dir);
        InterpFilter filter = av1_extract_interp_filter(mbmi->block_mi.interp_filters, dir);
        update_cdf(xd->tile_ctx->switchable_interp_cdf[ctx], filter, SWITCHABLE_FILTERS);
    }
}

/*******************************************************************************
 * Updates all the mv component stats/CDF for the current block
 ******************************************************************************/
static void update_mv_component_stats(int comp, NmvComponent* mvcomp, MvSubpelPrecision precision) {
    assert(comp != 0);
    int       offset;
    const int sign     = comp < 0;
    const int mag      = sign ? -comp : comp;
    const int mv_class = svt_av1_get_mv_class(mag - 1, &offset);
    const int d        = offset >> 3; // int mv data
    const int fr       = (offset >> 1) & 3; // fractional mv data
    const int hp       = offset & 1; // high precision mv data

    // Sign
    update_cdf(mvcomp->sign_cdf, sign, 2);

    // Class
    update_cdf(mvcomp->classes_cdf, mv_class, MV_CLASSES);

    // Integer bits
    if (mv_class == MV_CLASS_0) {
        update_cdf(mvcomp->class0_cdf, d, CLASS0_SIZE);
    } else {
        const int n = mv_class + CLASS0_BITS - 1; // number of bits
        for (int i = 0; i < n; ++i) {
            update_cdf(mvcomp->bits_cdf[i], (d >> i) & 1, 2);
        }
    }
    // Fractional bits
    if (precision > MV_SUBPEL_NONE) {
        AomCdfProb* fp_cdf = mv_class == MV_CLASS_0 ? mvcomp->class0_fp_cdf[d] : mvcomp->fp_cdf;
        update_cdf(fp_cdf, fr, MV_FP_SIZE);
    }
    // High precision bit
    if (precision > MV_SUBPEL_LOW_PRECISION) {
        AomCdfProb* hp_cdf = mv_class == MV_CLASS_0 ? mvcomp->class0_hp_cdf : mvcomp->hp_cdf;
        update_cdf(hp_cdf, hp, 2);
    }
}

/*******************************************************************************
 * Updates all the mv stats/CDF for the current block
 ******************************************************************************/
static void av1_update_mv_stats(const Mv* mv, const Mv* ref, NmvContext* mvctx, MvSubpelPrecision precision) {
    const Mv          diff = {{mv->x - ref->x, mv->y - ref->y}};
    const MvJointType j    = svt_av1_get_mv_joint(&diff);

    update_cdf(mvctx->joints_cdf, j, MV_JOINTS);

    // The y-component (row component) of the MV is coded first
    if (mv_joint_vertical(j)) {
        update_mv_component_stats(diff.y, &mvctx->comps[0], precision);
    }

    if (mv_joint_horizontal(j)) {
        update_mv_component_stats(diff.x, &mvctx->comps[1], precision);
    }
}

/*******************************************************************************
 * Updates all the Inter mode stats/CDF for the current block
 ******************************************************************************/
static AOM_INLINE void update_inter_mode_stats(FRAME_CONTEXT* fc, PredictionMode mode, int16_t mode_context) {
    int16_t mode_ctx = mode_context & NEWMV_CTX_MASK;
    assert(mode_ctx < NEWMV_MODE_CONTEXTS);
    if (mode == NEWMV) {
        update_cdf(fc->newmv_cdf[mode_ctx], 0, 2);
        return;
    }

    update_cdf(fc->newmv_cdf[mode_ctx], 1, 2);
    mode_ctx = (mode_context >> GLOBALMV_OFFSET) & GLOBALMV_CTX_MASK;
    if (mode == GLOBALMV) {
        update_cdf(fc->zeromv_cdf[mode_ctx], 0, 2);
        return;
    }

    update_cdf(fc->zeromv_cdf[mode_ctx], 1, 2);
    mode_ctx = (mode_context >> REFMV_OFFSET) & REFMV_CTX_MASK;
    assert(mode_ctx < REFMV_MODE_CONTEXTS);
    update_cdf(fc->refmv_cdf[mode_ctx], mode != NEARESTMV, 2);
}

/*******************************************************************************
 * Updates all the palette stats/CDF for the current block
 ******************************************************************************/
static AOM_INLINE void update_palette_cdf(MacroBlockD* xd, const MbModeInfo* const mbmi, BlkStruct* blk_ptr,
                                          const int mi_row, const int mi_col) {
    FRAME_CONTEXT*  fc                = xd->tile_ctx;
    const BlockSize bsize             = mbmi->bsize;
    const int       palette_bsize_ctx = svt_aom_get_palette_bsize_ctx(bsize);

    if (mbmi->block_mi.mode == DC_PRED) {
        const int n                = blk_ptr->palette_size[0];
        const int palette_mode_ctx = svt_aom_get_palette_mode_ctx(xd);

        update_cdf(fc->palette_y_mode_cdf[palette_bsize_ctx][palette_mode_ctx], n > 0, 2);
        if (n > 0) {
            update_cdf(fc->palette_y_size_cdf[palette_bsize_ctx], n - PALETTE_MIN_SIZE, PALETTE_SIZES);
        }
    }
    uint32_t  intra_chroma_mode = blk_ptr->block_mi.uv_mode;
    const int uv_dc_pred        = intra_chroma_mode == UV_DC_PRED && is_chroma_reference(mi_row, mi_col, bsize, 1, 1);

    if (uv_dc_pred) {
        const int n                   = blk_ptr->palette_size[1];
        const int palette_uv_mode_ctx = (blk_ptr->palette_size[0] > 0);
        update_cdf(fc->palette_uv_mode_cdf[palette_uv_mode_ctx], n > 0, 2);
        if (n > 0) {
            update_cdf(fc->palette_uv_size_cdf[palette_bsize_ctx], n - PALETTE_MIN_SIZE, PALETTE_SIZES);
        }
    }
}

/*******************************************************************************
 * Updates all the Intra stats/CDF for the current block
 ******************************************************************************/
static AOM_INLINE void sum_intra_stats(PictureControlSet* pcs, BlkStruct* blk_ptr, const int intraonly,
                                       const int mi_row, const int mi_col) {
    MacroBlockD*            xd       = blk_ptr->av1xd;
    const MbModeInfo* const mbmi     = xd->mi[0];
    FRAME_CONTEXT*          fc       = xd->tile_ctx;
    const PredictionMode    y_mode   = mbmi->block_mi.mode;
    const BlockSize         bsize    = mbmi->bsize;
    const int               bwidth   = block_size_wide[bsize];
    const int               bheight  = block_size_high[bsize];
    assert(bsize < BLOCK_SIZES_ALL);
    assert(y_mode < 13);

    if (intraonly) {
        update_cdf(get_y_mode_cdf(fc, xd), y_mode, INTRA_MODES);
    } else {
        update_cdf(fc->y_mode_cdf[eb_size_group_lookup[bsize]], y_mode, INTRA_MODES);
    }
    if (blk_ptr->block_mi.use_intrabc == 0 &&
        svt_aom_filter_intra_allowed(
            pcs->ppcs->scs->seq_header.filter_intra_level, bsize, blk_ptr->palette_size[0], y_mode)) {
        const int use_filter_intra_mode = blk_ptr->block_mi.filter_intra_mode != FILTER_INTRA_MODES;
        update_cdf(fc->filter_intra_cdfs[bsize], use_filter_intra_mode, 2);
        if (use_filter_intra_mode) {
            update_cdf(fc->filter_intra_mode_cdf, blk_ptr->block_mi.filter_intra_mode, FILTER_INTRA_MODES);
        }
    }
    if (av1_is_directional_mode(y_mode) && av1_use_angle_delta(bsize)) {
        update_cdf(fc->angle_delta_cdf[y_mode - V_PRED],
                   blk_ptr->block_mi.angle_delta[PLANE_TYPE_Y] + MAX_ANGLE_DELTA,
                   2 * MAX_ANGLE_DELTA + 1);
    }
    uint8_t sub_sampling_x = 1; // NM - subsampling_x is harcoded to 1 for 420 chroma sampling.
    uint8_t sub_sampling_y = 1; // NM - subsampling_y is harcoded to 1 for 420 chroma sampling.
    if (!is_chroma_reference(mi_row, mi_col, bsize, sub_sampling_x, sub_sampling_y)) {
        return;
    }
    const UvPredictionMode uv_mode     = blk_ptr->block_mi.uv_mode;
    const int              cfl_allowed = bwidth <= 32 && bheight <= 32;
    update_cdf(fc->uv_mode_cdf[cfl_allowed][y_mode], uv_mode, UV_INTRA_MODES - !cfl_allowed);
    if (uv_mode == UV_CFL_PRED) {
        const int8_t  joint_sign = blk_ptr->block_mi.cfl_alpha_signs;
        const uint8_t idx        = blk_ptr->block_mi.cfl_alpha_idx;
        update_cdf(fc->cfl_sign_cdf, joint_sign, CFL_JOINT_SIGNS);
        if (CFL_SIGN_U(joint_sign) != CFL_SIGN_ZERO) {
            AomCdfProb* cdf_u = fc->cfl_alpha_cdf[CFL_CONTEXT_U(joint_sign)];
            update_cdf(cdf_u, CFL_IDX_U(idx), CFL_ALPHABET_SIZE);
        }
        if (CFL_SIGN_V(joint_sign) != CFL_SIGN_ZERO) {
            assert(CFL_SIGN_V(joint_sign) > 0);
            AomCdfProb* cdf_v = fc->cfl_alpha_cdf[CFL_CONTEXT_V(joint_sign)];
            update_cdf(cdf_v, CFL_IDX_V(idx), CFL_ALPHABET_SIZE);
        }
    }
    if (av1_is_directional_mode(get_uv_mode(uv_mode)) && av1_use_angle_delta(bsize)) {
        assert((uv_mode - UV_V_PRED) < DIRECTIONAL_MODES);
        assert((uv_mode - UV_V_PRED) >= 0);
        update_cdf(fc->angle_delta_cdf[uv_mode - UV_V_PRED],
                   blk_ptr->block_mi.angle_delta[PLANE_TYPE_UV] + MAX_ANGLE_DELTA,
                   2 * MAX_ANGLE_DELTA + 1);
    }
    if (svt_aom_allow_palette(pcs->ppcs->frm_hdr.allow_screen_content_tools, bsize)) {
        update_palette_cdf(xd, mbmi, blk_ptr, mi_row, mi_col);
    }
}

/*******************************************************************************
 * Updates all the syntax stats/CDF for the current block
 ******************************************************************************/
void svt_aom_update_stats(PictureControlSet* pcs, BlkStruct* blk_ptr, int mi_row, int mi_col) {
    FrameHeader*            frm_hdr = &pcs->ppcs->frm_hdr;
    MacroBlockD*            xd      = blk_ptr->av1xd;
    const MbModeInfo* const mbmi    = xd->mi[0];
    const BlockSize         bsize   = mbmi->bsize;
    assert(bsize < BLOCK_SIZES_ALL);
    FRAME_CONTEXT* fc             = xd->tile_ctx;
    const int      seg_ref_active = pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled &&
        pcs->ppcs->frm_hdr.segmentation_params.seg_id_pre_skip;

    if (pcs->ppcs->frm_hdr.skip_mode_params.skip_mode_flag && !seg_ref_active && is_comp_ref_allowed(bsize)) {
        const int skip_mode_ctx = av1_get_skip_mode_context(xd);
        update_cdf(fc->skip_mode_cdfs[skip_mode_ctx], mbmi->block_mi.skip_mode, 2);
    }

    if (!mbmi->block_mi.skip_mode && !seg_ref_active) {
        const int skip_ctx = av1_get_skip_context(xd);
        update_cdf(fc->skip_cdfs[skip_ctx], mbmi->block_mi.skip, 2);
    }
    if (!is_inter_block(&mbmi->block_mi)) {
        sum_intra_stats(pcs, blk_ptr, frame_is_intra_only(pcs->ppcs), mi_row, mi_col);
    }
    if (svt_aom_allow_intrabc(&pcs->ppcs->frm_hdr, pcs->ppcs->slice_type)) {
        update_cdf(fc->intrabc_cdf, is_intrabc_block(&mbmi->block_mi), 2);
    }

    if (frame_is_intra_only(pcs->ppcs) || mbmi->block_mi.skip_mode) {
        return;
    }
    const int inter_block = is_inter_block(&mbmi->block_mi);
    if (!seg_ref_active) {
        update_cdf(fc->intra_inter_cdf[svt_av1_get_intra_inter_context(xd)], inter_block, 2);
        // If the segment reference feature is enabled we have only a single
        // reference frame allowed for the segment so exclude it from
        // the reference frame counts used to work out probabilities.
        if (inter_block) {
            const MvReferenceFrame ref0 = mbmi->block_mi.ref_frame[0];
            const MvReferenceFrame ref1 = mbmi->block_mi.ref_frame[1];
            if (pcs->ppcs->frm_hdr.reference_mode == REFERENCE_MODE_SELECT) {
                if (is_comp_ref_allowed(bsize)) {
                    update_cdf(svt_aom_get_reference_mode_cdf(xd), has_second_ref(&mbmi->block_mi), 2);
                }
            }

            if (has_second_ref(&mbmi->block_mi)) {
                const CompReferenceType comp_ref_type = has_uni_comp_refs(&mbmi->block_mi) ? UNIDIR_COMP_REFERENCE
                                                                                           : BIDIR_COMP_REFERENCE;
                update_cdf(svt_aom_get_comp_reference_type_cdf(xd), comp_ref_type, COMP_REFERENCE_TYPES);

                if (comp_ref_type == UNIDIR_COMP_REFERENCE) {
                    const int bit = (ref0 == BWDREF_FRAME);
                    update_cdf(svt_aom_get_pred_cdf_uni_comp_ref_p(xd), bit, 2);
                    if (!bit) {
                        const int bit1 = (ref1 == LAST3_FRAME || ref1 == GOLDEN_FRAME);
                        update_cdf(svt_aom_get_pred_cdf_uni_comp_ref_p1(xd), bit1, 2);
                        if (bit1) {
                            update_cdf(svt_aom_get_pred_cdf_uni_comp_ref_p2(xd), ref1 == GOLDEN_FRAME, 2);
                        }
                    }
                } else {
                    const int bit = (ref0 == GOLDEN_FRAME || ref0 == LAST3_FRAME);
                    update_cdf(svt_aom_get_pred_cdf_comp_ref_p(xd), bit, 2);
                    if (!bit) {
                        update_cdf(svt_aom_get_pred_cdf_comp_ref_p1(xd), ref0 == LAST2_FRAME, 2);
                    } else {
                        update_cdf(svt_aom_get_pred_cdf_comp_ref_p2(xd), ref0 == GOLDEN_FRAME, 2);
                    }
                    update_cdf(svt_aom_get_pred_cdf_comp_bwdref_p(xd), ref1 == ALTREF_FRAME, 2);
                    if (ref1 != ALTREF_FRAME) {
                        update_cdf(svt_aom_get_pred_cdf_comp_bwdref_p1(xd), ref1 == ALTREF2_FRAME, 2);
                    }
                }
            } else {
                const int bit = (ref0 >= BWDREF_FRAME);
                update_cdf(svt_aom_get_pred_cdf_single_ref_p1(xd), bit, 2);
                if (bit) {
                    assert(ref0 <= ALTREF_FRAME);
                    update_cdf(svt_aom_get_pred_cdf_single_ref_p2(xd), ref0 == ALTREF_FRAME, 2);
                    if (ref0 != ALTREF_FRAME) {
                        update_cdf(svt_aom_get_pred_cdf_single_ref_p6(xd), ref0 == ALTREF2_FRAME, 2);
                    }
                } else {
                    const int bit1 = !(ref0 == LAST2_FRAME || ref0 == LAST_FRAME);
                    update_cdf(svt_aom_get_pred_cdf_single_ref_p3(xd), bit1, 2);
                    if (!bit1) {
                        update_cdf(svt_aom_get_pred_cdf_single_ref_p4(xd), ref0 != LAST_FRAME, 2);
                    } else {
                        update_cdf(svt_aom_get_pred_cdf_single_ref_p5(xd), ref0 != LAST3_FRAME, 2);
                    }
                }
            }
            if (pcs->ppcs->scs->seq_header.enable_interintra_compound && svt_aom_is_interintra_allowed(mbmi)) {
                const int bsize_group = eb_size_group_lookup[bsize];
                if (blk_ptr->block_mi.is_interintra_used) {
                    update_cdf(fc->interintra_cdf[bsize_group], 1, 2);
                    update_cdf(
                        fc->interintra_mode_cdf[bsize_group], blk_ptr->block_mi.interintra_mode, INTERINTRA_MODES);
                    if (svt_aom_is_interintra_wedge_used(bsize)) {
                        update_cdf(fc->wedge_interintra_cdf[bsize], blk_ptr->block_mi.use_wedge_interintra, 2);
                        if (blk_ptr->block_mi.use_wedge_interintra) {
                            update_cdf(fc->wedge_idx_cdf[bsize], blk_ptr->block_mi.interintra_wedge_index, 16);
                        }
                    }
                } else {
                    update_cdf(fc->interintra_cdf[bsize_group], 0, 2);
                }
            }
            const MotionMode motion_allowed = pcs->ppcs->frm_hdr.is_motion_mode_switchable
                ? svt_aom_motion_mode_allowed(pcs,
                                              blk_ptr->block_mi.num_proj_ref,
                                              blk_ptr->overlappable_neighbors,
                                              bsize,
                                              mbmi->block_mi.ref_frame[0],
                                              mbmi->block_mi.ref_frame[1],
                                              mbmi->block_mi.mode)
                : SIMPLE_TRANSLATION;
            if (mbmi->block_mi.ref_frame[1] != INTRA_FRAME) {
                if (motion_allowed == WARPED_CAUSAL) {
                    update_cdf(fc->motion_mode_cdf[bsize], blk_ptr->block_mi.motion_mode, MOTION_MODES);
                } else if (motion_allowed == OBMC_CAUSAL) {
                    update_cdf(fc->obmc_cdf[bsize], blk_ptr->block_mi.motion_mode == OBMC_CAUSAL, 2);
                }
            }

            if (has_second_ref(&mbmi->block_mi)) {
                const int masked_compound_used = is_any_masked_compound_used(bsize) &&
                    pcs->ppcs->scs->seq_header.enable_masked_compound;
                if (masked_compound_used) {
                    const int comp_group_idx_ctx = svt_aom_get_comp_group_idx_context_enc(xd);
                    update_cdf(fc->comp_group_idx_cdf[comp_group_idx_ctx], mbmi->block_mi.comp_group_idx, 2);
                }
                if (mbmi->block_mi.comp_group_idx == 0) {
                    const int comp_index_ctx = svt_aom_get_comp_index_context_enc(
                        pcs->ppcs,
                        pcs->ppcs->cur_order_hint, // cur_frame_index,
                        pcs->ppcs->ref_order_hint[mbmi->block_mi.ref_frame[0] - 1], // bck_frame_index,
                        pcs->ppcs->ref_order_hint[mbmi->block_mi.ref_frame[1] - 1], // fwd_frame_index,
                        blk_ptr->av1xd);
                    update_cdf(fc->compound_index_cdf[comp_index_ctx], mbmi->block_mi.compound_idx, 2);
                } else {
                    assert(masked_compound_used);
                    if (is_interinter_compound_used(COMPOUND_WEDGE, bsize)) {
                        update_cdf(fc->compound_type_cdf[bsize],
                                   blk_ptr->block_mi.interinter_comp.type - COMPOUND_WEDGE,
                                   MASKED_COMPOUND_TYPES);
                    }
                    if (blk_ptr->block_mi.interinter_comp.type == COMPOUND_WEDGE) {
                        if (is_interinter_compound_used(COMPOUND_WEDGE, bsize)) {
                            update_cdf(fc->wedge_idx_cdf[bsize], blk_ptr->block_mi.interinter_comp.wedge_index, 16);
                        }
                    }
                }
            }
        }
    }
    if (inter_block && frm_hdr->interpolation_filter == SWITCHABLE && blk_ptr->block_mi.motion_mode != WARPED_CAUSAL &&
        !svt_aom_is_nontrans_global_motion(&mbmi->block_mi, bsize, pcs->ppcs)) {
        update_filter_type_cdf(xd, mbmi, pcs->scs->seq_header.enable_dual_filter);
    }
    if (inter_block && !seg_ref_active) {
        const PredictionMode mode     = mbmi->block_mi.mode;
        MvReferenceFrame     rf[2]    = {blk_ptr->block_mi.ref_frame[0], blk_ptr->block_mi.ref_frame[1]};
        const int16_t        mode_ctx = svt_aom_mode_context_analyzer(blk_ptr->inter_mode_ctx, rf);
        if (has_second_ref(&mbmi->block_mi)) {
            update_cdf(fc->inter_compound_mode_cdf[mode_ctx], INTER_COMPOUND_OFFSET(mode), INTER_COMPOUND_MODES);
        } else {
            update_inter_mode_stats(fc, mode, mode_ctx);
        }

        const int new_mv = mbmi->block_mi.mode == NEWMV || mbmi->block_mi.mode == NEW_NEWMV;
        if (new_mv) {
            const uint8_t ref_frame_type = av1_ref_frame_type(mbmi->block_mi.ref_frame);
            for (int idx = 0; idx < 2; ++idx) {
                if (xd->ref_mv_count[ref_frame_type] > idx + 1) {
                    const uint8_t drl_ctx = blk_ptr->drl_ctx[idx];
                    update_cdf(fc->drl_cdf[drl_ctx], blk_ptr->drl_index != idx, 2);
                    if (blk_ptr->drl_index == idx) {
                        break;
                    }
                }
            }
        }

        if (have_nearmv_in_inter_mode(mbmi->block_mi.mode)) {
            const uint8_t ref_frame_type = av1_ref_frame_type(mbmi->block_mi.ref_frame);
            for (int idx = 1; idx < 3; ++idx) {
                if (xd->ref_mv_count[ref_frame_type] > idx + 1) {
                    const uint8_t drl_ctx = blk_ptr->drl_ctx_near[idx - 1];
                    update_cdf(fc->drl_cdf[drl_ctx], blk_ptr->drl_index != idx - 1, 2);
                    if (blk_ptr->drl_index == idx - 1) {
                        break;
                    }
                }
            }
        }
        if (svt_aom_have_newmv_in_inter_mode(mbmi->block_mi.mode)) {
            const int allow_hp = pcs->ppcs->frm_hdr.force_integer_mv ? MV_SUBPEL_NONE
                                                                     : pcs->ppcs->frm_hdr.allow_high_precision_mv;
            if (new_mv) {
                Mv ref_mv;
                for (int ref = 0; ref < 1 + has_second_ref(&mbmi->block_mi); ++ref) {
                    ref_mv = blk_ptr->predmv[ref];
                    av1_update_mv_stats(&mbmi->block_mi.mv[ref], &ref_mv, &fc->nmvc, allow_hp);
                }
            } else if (mbmi->block_mi.mode == NEAREST_NEWMV || mbmi->block_mi.mode == NEAR_NEWMV) {
                Mv ref_mv = blk_ptr->predmv[1];
                Mv mv     = blk_ptr->block_mi.mv[1];
                av1_update_mv_stats(&mv, &ref_mv, &fc->nmvc, allow_hp);
            } else if (mbmi->block_mi.mode == NEW_NEARESTMV || mbmi->block_mi.mode == NEW_NEARMV) {
                Mv ref_mv = blk_ptr->predmv[0];
                Mv mv     = blk_ptr->block_mi.mv[0];
                av1_update_mv_stats(&mv, &ref_mv, &fc->nmvc, allow_hp);
            }
        }
    }
}

/*******************************************************************************
 * Updates the partition stats/CDF for the current block
 ******************************************************************************/
void svt_aom_update_part_stats(PictureControlSet* pcs, const PartitionType partition, const BlockSize bsize,
                               const uint16_t tile_idx, const uint32_t sb_index, const int mi_row, const int mi_col) {
    const Av1Common* const cm                = pcs->ppcs->av1_cm;
    FRAME_CONTEXT*         fc                = &pcs->ec_ctx_array[sb_index];
    const int              is_partition_root = bsize >= BLOCK_8X8;
    assert(bsize < BLOCK_SIZES_ALL);
    assert(mi_size_wide_log2[bsize] == mi_size_high_log2[bsize]);
    if (mi_row >= cm->mi_rows || mi_col >= cm->mi_cols || !is_partition_root) {
        return;
    }
    const int hbs      = mi_size_wide[bsize] >> 1;
    const int has_rows = (mi_row + hbs) < cm->mi_rows;
    const int has_cols = (mi_col + hbs) < cm->mi_cols;

    NeighborArrayUnit* partition_context_na                  = pcs->ep_partition_context_na[tile_idx];
    const uint32_t     partition_context_left_neighbor_index = get_neighbor_array_unit_left_index(partition_context_na,
                                                                                              (mi_row << MI_SIZE_LOG2));
    const uint32_t     partition_context_top_neighbor_index  = get_neighbor_array_unit_top_index(partition_context_na,
                                                                                            (mi_col << MI_SIZE_LOG2));

    const PartitionContextType above_ctx =
        (((PartitionContext*)partition_context_na->top_array)[partition_context_top_neighbor_index].above ==
         (char)INVALID_NEIGHBOR_DATA)
        ? 0
        : ((PartitionContext*)partition_context_na->top_array)[partition_context_top_neighbor_index].above;
    const PartitionContextType left_ctx =
        (((PartitionContext*)partition_context_na->left_array)[partition_context_left_neighbor_index].left ==
         (char)INVALID_NEIGHBOR_DATA)
        ? 0
        : ((PartitionContext*)partition_context_na->left_array)[partition_context_left_neighbor_index].left;
    const int32_t bsl   = mi_size_wide_log2[bsize] - mi_size_wide_log2[BLOCK_8X8];
    const int32_t above = (above_ctx >> bsl) & 1, left = (left_ctx >> bsl) & 1;
    assert(bsl >= 0);

    const int ctx = (left * 2 + above) + bsl * PARTITION_PLOFFSET;

    if (has_rows && has_cols) {
        update_cdf(fc->partition_cdf[ctx], partition, svt_aom_partition_cdf_length(bsize));
    }
}

uint8_t svt_aom_get_me_qindex(PictureControlSet* pcs, SuperBlock* sb_ptr, uint8_t is_sb128) {
    if (!is_sb128) {
        return pcs->b64_me_qindex[sb_ptr->index];
    }

    uint32_t pic_width_in_b64  = (pcs->ppcs->aligned_width + 64 - 1) / 64;
    uint32_t pic_height_in_b64 = (pcs->ppcs->aligned_height + 64 - 1) / 64;

    uint32_t x_b64_index = sb_ptr->org_x / 64;
    uint32_t y_b64_index = sb_ptr->org_y / 64;
    uint32_t b64_index   = x_b64_index + y_b64_index * pic_width_in_b64;

    uint8_t valid_b64_cnt = 1;

    uint16_t sum_me_qindex = pcs->b64_me_qindex[b64_index];

    if ((x_b64_index + 1) < pic_width_in_b64) {
        sum_me_qindex += pcs->b64_me_qindex[b64_index + 1];
        valid_b64_cnt++;
    }
    if ((y_b64_index + 1) < pic_height_in_b64) {
        sum_me_qindex += pcs->b64_me_qindex[b64_index + pic_width_in_b64];
        valid_b64_cnt++;
    }
    if (valid_b64_cnt == 3) {
        sum_me_qindex += pcs->b64_me_qindex[b64_index + 1 + pic_width_in_b64];
        valid_b64_cnt++;
    }

    return sum_me_qindex / valid_b64_cnt;
}
