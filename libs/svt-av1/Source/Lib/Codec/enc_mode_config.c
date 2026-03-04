#include "enc_mode_config.h"
#include <stdlib.h>

#include "rd_cost.h"
#include "aom_dsp_rtcd.h"
#include "mode_decision.h"
#include "coding_loop.h"

#define LOW_8x8_DIST_VAR_TH 25000
#define HIGH_8x8_DIST_VAR_TH 50000
#define MAX_LDP0_LVL 8 // Max supported ldp0 levels
static uint8_t pf_gi[16] = {0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60};
#if OPT_PER_BLK_INTRA
#define MAX_INTRA_LEVEL 10
static const uint8_t angular_pred_level[MAX_INTRA_LEVEL] = {0, 1, 2, 2, 3, 4, 4, 4, 4, 0};
#else
#define MAX_INTRA_LEVEL 9
static const uint8_t angular_pred_level[MAX_INTRA_LEVEL] = {0, 1, 2, 2, 3, 4, 4, 4, 0};
#endif
void svt_aom_get_qp_based_th_scaling_factors(bool enable_qp_based_th_scaling, uint32_t* ret_q_weight,
                                             uint32_t* ret_q_weight_denom, uint32_t qp) {
    if (enable_qp_based_th_scaling) {
        // limit scaling for low QPs to 10/63 to avoid extreme actions. 10 chosen arbitrarily.
        uint32_t q_weight       = MAX(10, qp);
        uint32_t q_weight_denom = MAX_QP_VALUE;
        // The QP boundary where we change from exponential to linear scaling was chosen so that the scaling will
        // be more aggressive for lower QP and so there would not be a large jump between the two methods.
        // Scaling(QP=45) = 45/63 = 0.714
        // Scaling(QP=46) = 1.05 - exp(-(46-35)/10) = 0.717
        // Therefore, the scaling for QP 45 is more aggressive than QP 46, and there is not a large jump in behaviour
        // when switching between the two methods.
        if (qp >= 46) {
            double ex           = -(MAX(40, (double)qp) - 35) / 10;
            double q_weight_int = exp(ex);
            q_weight_int        = 1.05 - q_weight_int;
            q_weight_int *= 10000;

            q_weight       = (uint32_t)q_weight_int;
            q_weight_denom = 10000;
        }
        *ret_q_weight       = q_weight;
        *ret_q_weight_denom = q_weight_denom;
    } else {
        *ret_q_weight       = 1;
        *ret_q_weight_denom = 1;
        return;
    }
}

/*
* Get the equivalent ME distortion/variance for a 128x128 SB, based on the ME data stored in 64x64 granularity.
*
* The equivalent values are stored in me_64x64_dist, me_32x32_dist, me_16x16_dist, me_8x8_dist, me_8x8_cost_var.
* The distortion metrics will be averaged (sum of 64x64 dists/num 64x64 blocks in the SB), while the variance
* will be the maximum of the 64x64 variances.
*/
static void get_sb128_me_data(PictureControlSet* pcs, ModeDecisionContext* ctx, uint32_t* me_64x64_dist,
                              uint32_t* me_32x32_dist, uint32_t* me_16x16_dist, uint32_t* me_8x8_dist,
                              uint32_t* me_8x8_cost_var) {
    uint32_t me_sb_size          = pcs->scs->b64_size;
    uint32_t me_pic_width_in_sb  = (pcs->ppcs->aligned_width + me_sb_size - 1) / me_sb_size;
    uint32_t me_pic_height_in_sb = (pcs->ppcs->aligned_height + me_sb_size - 1) / me_sb_size;
    uint32_t me_sb_x             = (ctx->sb_origin_x / me_sb_size);
    uint32_t me_sb_y             = (ctx->sb_origin_y / me_sb_size);
    uint32_t me_sb_addr          = me_sb_x + me_sb_y * me_pic_width_in_sb;

    uint32_t dist_64              = pcs->ppcs->me_64x64_distortion[me_sb_addr];
    uint32_t dist_32              = pcs->ppcs->me_32x32_distortion[me_sb_addr];
    uint32_t dist_16              = pcs->ppcs->me_16x16_distortion[me_sb_addr];
    uint32_t dist_8               = pcs->ppcs->me_8x8_distortion[me_sb_addr];
    uint32_t me_8x8_cost_variance = pcs->ppcs->me_8x8_cost_variance[me_sb_addr];
    int      count                = 1;
    if (me_sb_x + 1 < me_pic_width_in_sb) {
        dist_64 += pcs->ppcs->me_64x64_distortion[me_sb_addr + 1];
        dist_32 += pcs->ppcs->me_32x32_distortion[me_sb_addr + 1];
        dist_16 += pcs->ppcs->me_16x16_distortion[me_sb_addr + 1];
        dist_8 += pcs->ppcs->me_8x8_distortion[me_sb_addr + 1];
        me_8x8_cost_variance = MAX(me_8x8_cost_variance, pcs->ppcs->me_8x8_cost_variance[me_sb_addr + 1]);
        count++;
    }
    if (me_sb_y + 1 < me_pic_height_in_sb) {
        dist_64 += pcs->ppcs->me_64x64_distortion[me_sb_addr + me_pic_width_in_sb];
        dist_32 += pcs->ppcs->me_32x32_distortion[me_sb_addr + me_pic_width_in_sb];
        dist_16 += pcs->ppcs->me_16x16_distortion[me_sb_addr + me_pic_width_in_sb];
        dist_8 += pcs->ppcs->me_8x8_distortion[me_sb_addr + me_pic_width_in_sb];
        me_8x8_cost_variance = MAX(me_8x8_cost_variance,
                                   pcs->ppcs->me_8x8_cost_variance[me_sb_addr + me_pic_width_in_sb]);
        count++;
    }
    if (me_sb_x + 1 < me_pic_width_in_sb && me_sb_y + 1 < me_pic_height_in_sb) {
        dist_64 += pcs->ppcs->me_64x64_distortion[me_sb_addr + me_pic_width_in_sb + 1];
        dist_32 += pcs->ppcs->me_32x32_distortion[me_sb_addr + me_pic_width_in_sb + 1];
        dist_16 += pcs->ppcs->me_16x16_distortion[me_sb_addr + me_pic_width_in_sb + 1];
        dist_8 += pcs->ppcs->me_8x8_distortion[me_sb_addr + me_pic_width_in_sb + 1];
        me_8x8_cost_variance = MAX(me_8x8_cost_variance,
                                   pcs->ppcs->me_8x8_cost_variance[me_sb_addr + me_pic_width_in_sb + 1]);
        count++;
    }
    dist_64 /= count;
    dist_32 /= count;
    dist_16 /= count;
    dist_8 /= count;

    *me_64x64_dist   = dist_64;
    *me_32x32_dist   = dist_32;
    *me_16x16_dist   = dist_16;
    *me_8x8_dist     = dist_8;
    *me_8x8_cost_var = me_8x8_cost_variance;
}

/*
* Estimate the variance of the 128x128 by averaging the variances of the sub-64x64 blocks
*/
static void get_sb128_variance(PictureControlSet* pcs, ModeDecisionContext* ctx, uint16_t* variance) {
    uint32_t b64_size       = pcs->scs->b64_size;
    uint32_t b64_pic_width  = (pcs->ppcs->aligned_width + b64_size - 1) / b64_size;
    uint32_t b64_pic_height = (pcs->ppcs->aligned_height + b64_size - 1) / b64_size;
    uint32_t b64_x          = (ctx->sb_origin_x / b64_size);
    uint32_t b64_y          = (ctx->sb_origin_y / b64_size);
    uint32_t b64_index      = b64_x + b64_y * b64_pic_width;
    uint16_t var_64         = pcs->ppcs->variance[b64_index][ME_TIER_ZERO_PU_64x64];
    uint8_t  count          = 1;
    if (b64_x + 1 < b64_pic_width) {
        var_64 += pcs->ppcs->variance[b64_index + 1][ME_TIER_ZERO_PU_64x64];
        count++;
    }
    if (b64_y + 1 < b64_pic_height) {
        var_64 += pcs->ppcs->variance[b64_index + b64_pic_width][ME_TIER_ZERO_PU_64x64];
        count++;
    }
    if (b64_x + 1 < b64_pic_width && b64_y + 1 < b64_pic_height) {
        var_64 += pcs->ppcs->variance[b64_index + b64_pic_width + 1][ME_TIER_ZERO_PU_64x64];
        count++;
    }
    var_64 /= count;
    *variance = var_64;
}

// use this function to set the enable_me_8x8 level
uint8_t svt_aom_get_enable_me_8x8(EncMode enc_mode, EbInputResolution input_resolution, const bool rtc_tune,
                                  const bool flat_rtc_tune) {
    uint8_t enable_me_8x8 = 0;
    if (enc_mode <= ENC_M5) {
        enable_me_8x8 = 1;
    } else if ((!rtc_tune && enc_mode <= ENC_M8) || (rtc_tune && !flat_rtc_tune && enc_mode <= ENC_M10) ||
               (rtc_tune && flat_rtc_tune && enc_mode <= ENC_M9)) {
        if (input_resolution <= INPUT_SIZE_720p_RANGE) {
            enable_me_8x8 = 1;
        } else {
            enable_me_8x8 = 0;
        }
    } else {
        enable_me_8x8 = 0;
    }

    return enable_me_8x8;
}

uint8_t svt_aom_get_enable_me_16x16(EncMode enc_mode) {
    UNUSED(enc_mode);
    uint8_t enable_me_16x16 = 1;
    return enable_me_16x16;
}

uint8_t svt_aom_get_gm_core_level(EncMode enc_mode, bool super_res_off) {
    uint8_t gm_level = 0;
    if (super_res_off) {
        if (enc_mode <= ENC_MR) {
            gm_level = 2;
        } else if (enc_mode <= ENC_M5) {
            gm_level = 4;
        } else {
            gm_level = 0;
        }
    }
    return gm_level;
}

uint8_t svt_aom_derive_gm_level(PictureParentControlSet* pcs, bool super_res_off) {
    uint8_t       gm_level  = 0;
    const EncMode enc_mode  = pcs->enc_mode;
    const uint8_t is_islice = pcs->slice_type == I_SLICE;

    // disable global motion when reference scaling enabled,
    // even if current pic is not scaled, because its reference
    // pics might be scaled in different size
    // super-res is ok for its reference pics are always upscaled
    // to original size
    if (!is_islice) {
        gm_level = svt_aom_get_gm_core_level(enc_mode, super_res_off);
    }
    return gm_level;
}

/************************************************
 * Set ME/HME Params from Config
 ************************************************/
/************************************************
 * Set HME Search area parameters
 ************************************************/
static void set_hme_search_params(PictureParentControlSet* pcs, MeContext* me_ctx, EbInputResolution input_resolution) {
    const bool rtc_tune = pcs->scs->static_config.rtc;
    // Set number of HME level 0 search regions to use
    me_ctx->num_hme_sa_w = 2;
    me_ctx->num_hme_sa_h = 2;
    // Set HME level 0 min and max search areas
    if (pcs->enc_mode <= ENC_M1) {
        if (input_resolution < INPUT_SIZE_4K_RANGE) {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){32, 32};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
        } else {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){240, 240};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){480, 480};
        }
    } else if (pcs->enc_mode <= ENC_M3) {
        me_ctx->hme_l0_sa.sa_min = (SearchArea){32, 32};
        me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
    } else if (pcs->enc_mode <= ENC_M5) {
        me_ctx->hme_l0_sa.sa_min = (SearchArea){32, 32};
        me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
    } else if (!rtc_tune && pcs->enc_mode <= ENC_M7) {
        if (pcs->sc_class1 || input_resolution >= INPUT_SIZE_4K_RANGE) {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){32, 32};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
        } else {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){16, 16};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
        }
    } else if (pcs->enc_mode <= ENC_M8) {
        if (pcs->sc_class1) {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){32, 32};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
        } else {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){16, 16};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
        }
    } else {
        if (pcs->sc_class1) {
            me_ctx->hme_l0_sa.sa_min = (SearchArea){32, 32};
            me_ctx->hme_l0_sa.sa_max = (SearchArea){192, 192};
        } else {
            if (input_resolution < INPUT_SIZE_4K_RANGE) {
                me_ctx->hme_l0_sa.sa_min = (SearchArea){8, 8};
                me_ctx->hme_l0_sa.sa_max = (SearchArea){96, 96};
            } else {
                me_ctx->hme_l0_sa.sa_min = (SearchArea){16, 16};
                me_ctx->hme_l0_sa.sa_max = (SearchArea){96, 96};
            }
        }
    }
    // Modulate the HME search-area using qp
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.hme_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->scs->static_config.qp);
    me_ctx->hme_l0_sa.sa_min.width  = MAX(8,
                                         DIVIDE_AND_ROUND(me_ctx->hme_l0_sa.sa_min.width * q_weight, q_weight_denom));
    me_ctx->hme_l0_sa.sa_min.height = MAX(8,
                                          DIVIDE_AND_ROUND(me_ctx->hme_l0_sa.sa_min.height * q_weight, q_weight_denom));
    me_ctx->hme_l0_sa.sa_max.width  = MAX(96,
                                         DIVIDE_AND_ROUND(me_ctx->hme_l0_sa.sa_max.width * q_weight, q_weight_denom));
    me_ctx->hme_l0_sa.sa_max.height = MAX(96,
                                          DIVIDE_AND_ROUND(me_ctx->hme_l0_sa.sa_max.height * q_weight, q_weight_denom));
    // Set the HME Level 1 and Level 2 refinement areas
    if (pcs->enc_mode <= ENC_MR) {
        me_ctx->hme_l1_sa = (SearchArea){16, 16};
        me_ctx->hme_l2_sa = (SearchArea){16, 16};
    } else {
        me_ctx->hme_l1_sa = (SearchArea){8, 3};
        me_ctx->hme_l2_sa = (SearchArea){8, 3};
    }
}

/************************************************
 * Set ME Search area parameters
 ************************************************/
static void set_me_search_params(SequenceControlSet* scs, PictureParentControlSet* pcs, MeContext* me_ctx,
                                 EbInputResolution input_resolution) {
    const EncMode enc_mode  = pcs->enc_mode;
    const uint8_t sc_class1 = pcs->sc_class1;
    const bool    rtc_tune  = scs->static_config.rtc;
    // Set the min and max ME search area
    if (rtc_tune) {
        if (sc_class1) {
            if (enc_mode <= ENC_M7) {
                me_ctx->me_sa.sa_min = (SearchArea){48, 48};
                me_ctx->me_sa.sa_max = (SearchArea){96, 96};
            } else if (enc_mode <= ENC_M9) {
                if (input_resolution < INPUT_SIZE_1080p_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){24, 16};
                    me_ctx->me_sa.sa_max = (SearchArea){32, 16};
                } else {
                    me_ctx->me_sa.sa_min = (SearchArea){24, 24};
                    me_ctx->me_sa.sa_max = (SearchArea){24, 24};
                }
            } else {
                if (input_resolution < INPUT_SIZE_1080p_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){24, 16};
                    me_ctx->me_sa.sa_max = (SearchArea){32, 16};
                } else {
                    me_ctx->me_sa.sa_min = (SearchArea){16, 9};
                    me_ctx->me_sa.sa_max = (SearchArea){16, 9};
                }
            }
        } else {
            if ((!scs->use_flat_ipp && enc_mode <= ENC_M9) || (scs->use_flat_ipp && enc_mode <= ENC_M10)) {
                if (input_resolution < INPUT_SIZE_1080p_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){24, 16};
                    me_ctx->me_sa.sa_max = (SearchArea){32, 16};
                } else {
                    me_ctx->me_sa.sa_min = (SearchArea){16, 9};
                    me_ctx->me_sa.sa_max = (SearchArea){16, 9};
                }
            } else if (!scs->use_flat_ipp && enc_mode <= ENC_M10) {
                if (input_resolution < INPUT_SIZE_1080p_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){12, 4};
                    me_ctx->me_sa.sa_max = (SearchArea){16, 9};
                } else {
                    me_ctx->me_sa.sa_min = (SearchArea){16, 9};
                    me_ctx->me_sa.sa_max = (SearchArea){16, 9};
                }
            } else if ((scs->use_flat_ipp && enc_mode <= ENC_M11) || (!scs->use_flat_ipp && enc_mode <= ENC_M10)) {
                if (input_resolution < INPUT_SIZE_720p_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){12, 4};
                    me_ctx->me_sa.sa_max = (SearchArea){16, 9};
                } else if (input_resolution < INPUT_SIZE_1080p_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){12, 1};
                    me_ctx->me_sa.sa_max = (SearchArea){16, 7};
                } else if (input_resolution < INPUT_SIZE_4K_RANGE) {
                    me_ctx->me_sa.sa_min = (SearchArea){8, 1};
                    me_ctx->me_sa.sa_max = (SearchArea){8, 7};
                } else {
                    me_ctx->me_sa.sa_min = (SearchArea){8, 1};
                    me_ctx->me_sa.sa_max = (SearchArea){8, 1};
                }
            } else {
                me_ctx->me_sa.sa_min = (SearchArea){8, 1};
                me_ctx->me_sa.sa_max = (SearchArea){8, 1};
            }
        }
    } else if (sc_class1) {
        if (enc_mode <= ENC_MR) {
            me_ctx->me_sa.sa_min = (SearchArea){60, 60};
            me_ctx->me_sa.sa_max = (SearchArea){240, 240};
        } else if (enc_mode <= ENC_M0) {
            me_ctx->me_sa.sa_min = (SearchArea){48, 48};
            me_ctx->me_sa.sa_max = (SearchArea){192, 192};
        } else if (enc_mode <= ENC_M1) {
            me_ctx->me_sa.sa_min = (SearchArea){24, 24};
            me_ctx->me_sa.sa_max = (SearchArea){128, 128};
        } else if (enc_mode <= ENC_M4) {
            me_ctx->me_sa.sa_min = (SearchArea){12, 12};
            me_ctx->me_sa.sa_max = (SearchArea){96, 96};
        } else if (enc_mode <= ENC_M7) {
            me_ctx->me_sa.sa_min = (SearchArea){12, 12};
            me_ctx->me_sa.sa_max = (SearchArea){64, 64};
        } else {
            me_ctx->me_sa.sa_min = (SearchArea){12, 12};
            me_ctx->me_sa.sa_max = (SearchArea){16, 16};
        }
    } else if (enc_mode <= ENC_MR) {
        me_ctx->me_sa.sa_min = (SearchArea){96, 96};
        me_ctx->me_sa.sa_max = (SearchArea){256, 256};
    } else if (enc_mode <= ENC_M1) {
        me_ctx->me_sa.sa_min = (SearchArea){84, 84};
        me_ctx->me_sa.sa_max = (SearchArea){224, 224};
    } else if (enc_mode <= ENC_M3) {
        me_ctx->me_sa.sa_min = (SearchArea){24, 24};
        me_ctx->me_sa.sa_max = (SearchArea){88, 88};
    } else if (enc_mode <= ENC_M5) {
        me_ctx->me_sa.sa_min = (SearchArea){24, 12};
        me_ctx->me_sa.sa_max = (SearchArea){48, 32};
    } else if (enc_mode <= ENC_M7) {
        me_ctx->me_sa.sa_min = (SearchArea){24, 9};
        me_ctx->me_sa.sa_max = (SearchArea){24, 12};
    } else if (enc_mode <= ENC_M8) {
        me_ctx->me_sa.sa_min = (SearchArea){16, 6};
        me_ctx->me_sa.sa_max = (SearchArea){24, 12};
    } else if (enc_mode <= ENC_M11) {
        if (input_resolution < INPUT_SIZE_720p_RANGE) {
            me_ctx->me_sa.sa_min = (SearchArea){12, 4};
            me_ctx->me_sa.sa_max = (SearchArea){16, 9};
        } else if (input_resolution < INPUT_SIZE_1080p_RANGE) {
            me_ctx->me_sa.sa_min = (SearchArea){12, 1};
            me_ctx->me_sa.sa_max = (SearchArea){16, 7};
        } else if (input_resolution < INPUT_SIZE_4K_RANGE) {
            me_ctx->me_sa.sa_min = (SearchArea){8, 1};
            me_ctx->me_sa.sa_max = (SearchArea){8, 7};
        } else {
            me_ctx->me_sa.sa_min = (SearchArea){8, 1};
            me_ctx->me_sa.sa_max = (SearchArea){8, 1};
        }
    } else {
        me_ctx->me_sa.sa_min = (SearchArea){8, 1};
        me_ctx->me_sa.sa_max = (SearchArea){8, 1};
    }
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(
        scs->qp_based_th_scaling_ctrls.me_qp_based_th_scaling, &q_weight, &q_weight_denom, scs->static_config.qp);
    me_ctx->me_sa.sa_min.width  = MAX(8, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_min.width * q_weight, q_weight_denom));
    me_ctx->me_sa.sa_min.height = MAX(3, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_min.height * q_weight, q_weight_denom));
    me_ctx->me_sa.sa_max.width  = MAX(8, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_max.width * q_weight, q_weight_denom));
    me_ctx->me_sa.sa_max.height = MAX(3, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_max.height * q_weight, q_weight_denom));
}

static void svt_aom_set_me_hme_ref_prune_ctrls(MeContext* me_ctx, uint8_t prune_level) {
    MeHmeRefPruneCtrls* me_hme_prune_ctrls = &me_ctx->me_hme_prune_ctrls;

    switch (prune_level) {
    case 0:
        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 0;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = (uint16_t)~0;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = (uint16_t)~0;

        me_hme_prune_ctrls->zz_sad_th    = 0;
        me_hme_prune_ctrls->zz_sad_pct   = 0;
        me_hme_prune_ctrls->phme_sad_th  = 0;
        me_hme_prune_ctrls->phme_sad_pct = 0;
        break;
    case 1:
        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 1;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = 80;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = (uint16_t)~0;

        me_hme_prune_ctrls->zz_sad_th    = 0;
        me_hme_prune_ctrls->zz_sad_pct   = 0;
        me_hme_prune_ctrls->phme_sad_th  = 0;
        me_hme_prune_ctrls->phme_sad_pct = 0;
        break;
    case 2:

        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 1;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = 50;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = (uint16_t)~0;

        me_hme_prune_ctrls->zz_sad_th    = 0;
        me_hme_prune_ctrls->zz_sad_pct   = 0;
        me_hme_prune_ctrls->phme_sad_th  = 0;
        me_hme_prune_ctrls->phme_sad_pct = 0;
        break;

    case 3:
        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 1;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = 30;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = (uint16_t)~0;

        me_hme_prune_ctrls->zz_sad_th    = 0;
        me_hme_prune_ctrls->zz_sad_pct   = 0;
        me_hme_prune_ctrls->phme_sad_th  = 0;
        me_hme_prune_ctrls->phme_sad_pct = 0;
        break;
    case 4:
        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 1;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = 15;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = 60;

        me_hme_prune_ctrls->zz_sad_th    = 0;
        me_hme_prune_ctrls->zz_sad_pct   = 0;
        me_hme_prune_ctrls->phme_sad_th  = 0;
        me_hme_prune_ctrls->phme_sad_pct = 0;
        break;

    case 5:
        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 1;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = 5;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = 60;

        me_hme_prune_ctrls->zz_sad_th    = 0;
        me_hme_prune_ctrls->zz_sad_pct   = 0;
        me_hme_prune_ctrls->phme_sad_th  = 0;
        me_hme_prune_ctrls->phme_sad_pct = 0;
        break;
    case 6:
        me_hme_prune_ctrls->enable_me_hme_ref_pruning               = 1;
        me_hme_prune_ctrls->prune_ref_if_hme_sad_dev_bigger_than_th = 5;
        me_hme_prune_ctrls->prune_ref_if_me_sad_dev_bigger_than_th  = 60;

        me_hme_prune_ctrls->zz_sad_th    = 20 * 64 * 64;
        me_hme_prune_ctrls->zz_sad_pct   = 5;
        me_hme_prune_ctrls->phme_sad_th  = 10 * 64 * 64;
        me_hme_prune_ctrls->phme_sad_pct = 5;
        break;
    default:
        assert(0);
        break;
    }
}

static void svt_aom_set_mv_based_sa_ctrls(MeContext* me_ctx, uint8_t mv_sa_adj_level) {
    MvBasedSearchAdj* mv_sa_adj_ctrls = &me_ctx->mv_based_sa_adj;

    switch (mv_sa_adj_level) {
    case 0:
        mv_sa_adj_ctrls->enabled = 0;
        break;
    case 1:
        mv_sa_adj_ctrls->enabled          = 1;
        mv_sa_adj_ctrls->nearest_ref_only = 0;
        mv_sa_adj_ctrls->mv_size_th       = 25;
        mv_sa_adj_ctrls->sa_multiplier    = 2;
        break;
    case 2:
        mv_sa_adj_ctrls->enabled          = 1;
        mv_sa_adj_ctrls->nearest_ref_only = 1;
        mv_sa_adj_ctrls->mv_size_th       = 25;
        mv_sa_adj_ctrls->sa_multiplier    = 2;
        break;
    default:
        assert(0);
        break;
    }
}

static void svt_aom_set_me_sr_adjustment_ctrls(MeContext* me_ctx, uint8_t sr_adjustment_level) {
    MeSrCtrls* me_sr_adjustment_ctrls = &me_ctx->me_sr_adjustment_ctrls;

    switch (sr_adjustment_level) {
    case 0:
        me_sr_adjustment_ctrls->enable_me_sr_adjustment = 0;
        break;
    case 1:
        me_sr_adjustment_ctrls->enable_me_sr_adjustment              = 1;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_mv_length_th   = 4;
        me_sr_adjustment_ctrls->stationary_hme_sad_abs_th            = 12000;
        me_sr_adjustment_ctrls->stationary_me_sr_divisor             = 8;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th = 6000;
        me_sr_adjustment_ctrls->me_sr_divisor_for_low_hme_sad        = 8;
        me_sr_adjustment_ctrls->distance_based_hme_resizing          = 0;
        break;
    case 2:
        me_sr_adjustment_ctrls->enable_me_sr_adjustment              = 1;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_mv_length_th   = 4;
        me_sr_adjustment_ctrls->stationary_hme_sad_abs_th            = 12000;
        me_sr_adjustment_ctrls->stationary_me_sr_divisor             = 8;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th = 6000;
        me_sr_adjustment_ctrls->me_sr_divisor_for_low_hme_sad        = 8;
        me_sr_adjustment_ctrls->distance_based_hme_resizing          = 1;
        break;
    case 3:
        me_sr_adjustment_ctrls->enable_me_sr_adjustment              = 1;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_mv_length_th   = 4;
        me_sr_adjustment_ctrls->stationary_hme_sad_abs_th            = 12000;
        me_sr_adjustment_ctrls->stationary_me_sr_divisor             = 8;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th = 12000;
        me_sr_adjustment_ctrls->me_sr_divisor_for_low_hme_sad        = 8;
        me_sr_adjustment_ctrls->distance_based_hme_resizing          = 1;
        break;
    case 4:
        me_sr_adjustment_ctrls->enable_me_sr_adjustment              = 2;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_mv_length_th   = 16;
        me_sr_adjustment_ctrls->stationary_hme_sad_abs_th            = 20000;
        me_sr_adjustment_ctrls->stationary_me_sr_divisor             = 8;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th = 20000;
        me_sr_adjustment_ctrls->me_sr_divisor_for_low_hme_sad        = 8;
        me_sr_adjustment_ctrls->distance_based_hme_resizing          = 1;
        break;

    case 5:

        me_sr_adjustment_ctrls->enable_me_sr_adjustment              = 2;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_mv_length_th   = 20;
        me_sr_adjustment_ctrls->stationary_hme_sad_abs_th            = 24000;
        me_sr_adjustment_ctrls->stationary_me_sr_divisor             = 8;
        me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th = 24000;
        me_sr_adjustment_ctrls->me_sr_divisor_for_low_hme_sad        = 8;
        me_sr_adjustment_ctrls->distance_based_hme_resizing          = 1;

        break;

    default:
        assert(0);
        break;
    }
    if (me_ctx->enable_hme_level2_flag == 0) {
        if (me_ctx->enable_hme_level1_flag == 1) {
            me_sr_adjustment_ctrls->stationary_hme_sad_abs_th = me_sr_adjustment_ctrls->stationary_hme_sad_abs_th / 4;
            me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th =
                me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th / 4;
        } else {
            me_sr_adjustment_ctrls->stationary_hme_sad_abs_th = me_sr_adjustment_ctrls->stationary_hme_sad_abs_th / 16;
            me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th =
                me_sr_adjustment_ctrls->reduce_me_sr_based_on_hme_sad_abs_th / 16;
        }
    }
}

static void svt_aom_set_me_8x8_var_ctrls(MeContext* me_ctx, uint8_t level) {
    Me8x8VarCtrls* me_8x8_var_ctrls = &me_ctx->me_8x8_var_ctrls;

    switch (level) {
    case 0:
        me_8x8_var_ctrls->enabled = 0;
        break;
    case 1:
        me_8x8_var_ctrls->enabled        = 1;
        me_8x8_var_ctrls->me_sr_div4_th  = 0;
        me_8x8_var_ctrls->me_sr_div2_th  = 0;
        me_8x8_var_ctrls->me_sr_mult2_th = 900000;
        break;
    case 2:
        me_8x8_var_ctrls->enabled        = 1;
        me_8x8_var_ctrls->me_sr_div4_th  = 80000;
        me_8x8_var_ctrls->me_sr_div2_th  = 150000;
        me_8x8_var_ctrls->me_sr_mult2_th = (uint32_t)~0;
        break;
    default:
        assert(0);
    }
}

/*configure PreHme control*/
static void svt_aom_set_prehme_ctrls(MeContext* me_ctx, uint8_t level) {
    PreHmeCtrls* ctrl = &me_ctx->prehme_ctrl;

    switch (level) {
    case 0:
        ctrl->enable = 0;
        break;
    case 1:
        ctrl->enable = 1;
        // vertical shape search region
        ctrl->prehme_sa_cfg[0].sa_min = (SearchArea){8, 144};
        ctrl->prehme_sa_cfg[0].sa_max = (SearchArea){8, 496};
        // horizontal shape search region
        ctrl->prehme_sa_cfg[1].sa_min = (SearchArea){144, 3};
        ctrl->prehme_sa_cfg[1].sa_max = (SearchArea){496, 3};
        ctrl->skip_search_line        = 0;
        ctrl->l1_early_exit           = 0;
        break;
    case 2:
        ctrl->enable = 1;
        // vertical shape search region
        ctrl->prehme_sa_cfg[0].sa_min = (SearchArea){8, 100};
        ctrl->prehme_sa_cfg[0].sa_max = (SearchArea){8, 400};
        // horizontal shape search region
        ctrl->prehme_sa_cfg[1].sa_min = (SearchArea){96, 3};
        ctrl->prehme_sa_cfg[1].sa_max = (SearchArea){384, 3};
        ctrl->skip_search_line        = 0;
        ctrl->l1_early_exit           = 0;
        break;
    case 3:
        ctrl->enable = 1;
        // vertical shape search region
        ctrl->prehme_sa_cfg[0].sa_min = (SearchArea){8, 100};
        ctrl->prehme_sa_cfg[0].sa_max = (SearchArea){8, 350};
        // horizontal shape search region
        ctrl->prehme_sa_cfg[1].sa_min = (SearchArea){32, 7};
        ctrl->prehme_sa_cfg[1].sa_max = (SearchArea){200, 7};
        ctrl->skip_search_line        = 1;
        ctrl->l1_early_exit           = 0;
        break;
    case 4:
        ctrl->enable = 1;
        // vertical shape search region
        ctrl->prehme_sa_cfg[0].sa_min = (SearchArea){8, 100};
        ctrl->prehme_sa_cfg[0].sa_max = (SearchArea){8, 350};
        // horizontal shape search region
        ctrl->prehme_sa_cfg[1].sa_min = (SearchArea){32, 7};
        ctrl->prehme_sa_cfg[1].sa_max = (SearchArea){128, 7};
        ctrl->skip_search_line        = 1;
        ctrl->l1_early_exit           = 1;
        break;
    default:
        assert(0);
        break;
    }
}

/************************************************
 * Set ME/HME Params for Altref Temporal Filtering
 ************************************************/
static void tf_set_me_hme_params_oq(MeContext* me_ctx, PictureParentControlSet* pcs) {
    switch (pcs->tf_ctrls.hme_me_level) {
    case 0:
        me_ctx->num_hme_sa_w                = 2;
        me_ctx->num_hme_sa_h                = 2;
        me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){30, 30};
        me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){60, 60};
        me_ctx->hme_l1_sa                   = (SearchArea){16, 16};
        me_ctx->hme_l2_sa                   = (SearchArea){16, 16};
        me_ctx->me_sa.sa_min                = (SearchArea){60, 60};
        me_ctx->me_sa.sa_max                = (SearchArea){120, 120};
        break;

    case 1:
        me_ctx->num_hme_sa_w                = 2;
        me_ctx->num_hme_sa_h                = 2;
        me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){16, 16};
        me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){32, 32};
        me_ctx->hme_l1_sa                   = (SearchArea){16, 16};
        me_ctx->hme_l2_sa                   = (SearchArea){16, 16};
        me_ctx->me_sa.sa_min                = (SearchArea){16, 16};
        me_ctx->me_sa.sa_max                = (SearchArea){32, 32};
        break;

    case 2:
        me_ctx->num_hme_sa_w = 2;
        me_ctx->num_hme_sa_h = 2;
        if (pcs->scs->input_resolution <= INPUT_SIZE_360p_RANGE) {
            me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){8, 8};
            me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){8, 8};
            me_ctx->hme_l1_sa                   = (SearchArea){8, 8};
        } else if (pcs->scs->input_resolution <= INPUT_SIZE_480p_RANGE) {
            me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){8, 8};
            me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){16, 16};
            me_ctx->hme_l1_sa                   = (SearchArea){8, 8};
        } else {
            me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){16, 16};
            me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){32, 32};
            me_ctx->hme_l1_sa                   = (SearchArea){16, 16};
        }
        me_ctx->hme_l2_sa    = (SearchArea){16, 16};
        me_ctx->me_sa.sa_min = (SearchArea){8, 8};
        me_ctx->me_sa.sa_max = (SearchArea){8, 8};
        break;
    case 3:
        me_ctx->num_hme_sa_w                = 2;
        me_ctx->num_hme_sa_h                = 2;
        me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){8, 8};
        me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){8, 8};
        me_ctx->hme_l1_sa                   = (SearchArea){8, 8};
        me_ctx->hme_l2_sa                   = (SearchArea){8, 8};
        me_ctx->me_sa.sa_min                = (SearchArea){8, 8};
        me_ctx->me_sa.sa_max                = (SearchArea){8, 8};
        break;
    case 4:
        me_ctx->num_hme_sa_w                = 2;
        me_ctx->num_hme_sa_h                = 2;
        me_ctx->hme_l0_sa_default_tf.sa_min = (SearchArea){4, 4};
        me_ctx->hme_l0_sa_default_tf.sa_max = (SearchArea){4, 4};
        me_ctx->hme_l1_sa                   = (SearchArea){8, 8};
        me_ctx->hme_l2_sa                   = (SearchArea){8, 8};
        me_ctx->me_sa.sa_min                = (SearchArea){8, 8};
        me_ctx->me_sa.sa_max                = (SearchArea){8, 8};
        break;

    default:
        assert(0);
        break;
    }

    // Modulate the ME search-area using qp
    if (pcs->tf_ctrls.qp_opt) {
        uint32_t q_weight, q_weight_denom;
        svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.tf_me_qp_based_th_scaling,
                                                &q_weight,
                                                &q_weight_denom,
                                                pcs->scs->static_config.qp);
        me_ctx->me_sa.sa_min.width  = MAX(8, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_min.width * q_weight, q_weight_denom));
        me_ctx->me_sa.sa_min.height = MAX(8, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_min.height * q_weight, q_weight_denom));
        me_ctx->me_sa.sa_max.width  = MAX(8, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_max.width * q_weight, q_weight_denom));
        me_ctx->me_sa.sa_max.height = MAX(8, DIVIDE_AND_ROUND(me_ctx->me_sa.sa_max.height * q_weight, q_weight_denom));
    }
};

/******************************************************
* Derive ME Settings for OQ
  Input   : encoder mode and tune
  Output  : ME Kernel signal(s)
******************************************************/
void svt_aom_sig_deriv_me(SequenceControlSet* scs, PictureParentControlSet* pcs, MeContext* me_ctx) {
    EncMode           enc_mode         = pcs->enc_mode;
    const uint8_t     sc_class1        = pcs->sc_class1;
    const uint8_t     sc_class4        = pcs->sc_class4;
    EbInputResolution input_resolution = scs->input_resolution;
    const bool        rtc_tune         = scs->static_config.rtc;
    const bool        is_base          = pcs->temporal_layer_index == 0;
    const bool        flat_rtc         = rtc_tune && scs->use_flat_ipp;
    // Set ME search area
    set_me_search_params(scs, pcs, me_ctx, input_resolution);

    // Set HME search area
    set_hme_search_params(pcs, me_ctx, input_resolution);

    // Set HME flags
    me_ctx->enable_hme_flag        = pcs->enable_hme_flag;
    me_ctx->enable_hme_level0_flag = pcs->enable_hme_level0_flag;
    me_ctx->enable_hme_level1_flag = pcs->enable_hme_level1_flag;
    me_ctx->enable_hme_level2_flag = pcs->enable_hme_level2_flag;
    // HME Search Method
    me_ctx->hme_search_method = SUB_SAD_SEARCH;
    me_ctx->me_search_method  = SUB_SAD_SEARCH;

    if (rtc_tune && sc_class1) {
        me_ctx->reduce_hme_l0_sr_th_min = 8;
        me_ctx->reduce_hme_l0_sr_th_max = 100;
    } else {
        me_ctx->reduce_hme_l0_sr_th_min = 0;
        me_ctx->reduce_hme_l0_sr_th_max = 0;
    }
    // Set pre-hme level (0-2)
    uint8_t prehme_level = 0;
    if (sc_class1) {
        prehme_level = rtc_tune ? 1 : 2;
    } else if (flat_rtc) {
        if (enc_mode <= ENC_M8) {
            prehme_level = 2;
        } else if (enc_mode <= ENC_M11) {
            prehme_level = 4;
        } else {
            prehme_level = 0;
        }
    } else if (rtc_tune) {
        if (enc_mode <= ENC_M8) {
            prehme_level = 2;
        } else if (enc_mode <= ENC_M10) {
            prehme_level = 4;
        } else {
            prehme_level = 0;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            prehme_level = 2;
        } else if (enc_mode <= ENC_M11) {
            prehme_level = 4;
        } else {
            prehme_level = 0;
        }
    }
    if (pcs->enable_hme_level1_flag == 0) {
        prehme_level = 0;
    }

    svt_aom_set_prehme_ctrls(me_ctx, prehme_level);

    // Set hme/me based reference pruning level (0-4)

    uint8_t me_ref_prune_level = 0;

    if (sc_class1) {
        if (enc_mode <= ENC_M2) {
            me_ref_prune_level = 1;
        } else if (enc_mode <= ENC_M7) {
            me_ref_prune_level = 3;
        } else {
            me_ref_prune_level = 6;
        }
    } else {
        if (enc_mode <= ENC_MR) {
            me_ref_prune_level = 0;
        } else if (enc_mode <= ENC_M0) {
            me_ref_prune_level = is_base ? 1 : 4;
        } else if (enc_mode <= ENC_M5) {
            me_ref_prune_level = is_base ? 1 : 5;
        } else if ((!rtc_tune && enc_mode <= ENC_M8) || (!scs->use_flat_ipp && rtc_tune && enc_mode <= ENC_M7) ||
                   (scs->use_flat_ipp && rtc_tune && enc_mode <= ENC_M6)) {
            me_ref_prune_level = is_base ? 1 : 6;
        } else {
            me_ref_prune_level = 6;
        }
    }

    svt_aom_set_me_hme_ref_prune_ctrls(me_ctx, me_ref_prune_level);

    // Set hme-based me sr adjustment level
    uint8_t me_sr_adj_lvl = 0;
    if (sc_class1) {
        if (enc_mode <= ENC_M8) {
            me_sr_adj_lvl = 4;
        } else {
            me_sr_adj_lvl = 5;
        }
    } else if (enc_mode <= ENC_M0) {
        me_sr_adj_lvl = 0;
    } else {
        me_sr_adj_lvl = 3;
    }
    svt_aom_set_me_sr_adjustment_ctrls(me_ctx, me_sr_adj_lvl);

    uint8_t mv_sa_adj_level = 0;
    if (enc_mode <= ENC_M0) {
        mv_sa_adj_level = 2;
    } else {
        mv_sa_adj_level = 0;
    }
    svt_aom_set_mv_based_sa_ctrls(me_ctx, mv_sa_adj_level);

    uint8_t me_8x8_var_lvl = 2;
    svt_aom_set_me_8x8_var_ctrls(me_ctx, me_8x8_var_lvl);
    if (enc_mode <= ENC_M1) {
        me_ctx->prune_me_candidates_th = 0;
    } else {
        me_ctx->prune_me_candidates_th = 65;
    }

    // Applies to sc-class1 & sc-class4 scenes
    if (sc_class1) {
        if (enc_mode <= ENC_M2) {
            me_ctx->sc_class_me_boost = 1;
        } else if (enc_mode <= ENC_M6) {
            me_ctx->sc_class_me_boost = 2;
        } else if (enc_mode <= ENC_M7) {
            me_ctx->sc_class_me_boost = 3;
        } else {
            me_ctx->sc_class_me_boost = 0;
        }
    } else if (sc_class4) {
        if (enc_mode <= ENC_M6) {
            me_ctx->sc_class_me_boost = 1;
        } else {
            me_ctx->sc_class_me_boost = 0;
        }
    } else {
        me_ctx->sc_class_me_boost = 0;
    }
    // Set signal at picture level b/c may check signal in MD
    me_ctx->use_best_unipred_cand_only = pcs->use_best_me_unipred_cand_only;
    if (rtc_tune && sc_class1) {
        me_ctx->me_early_exit_th = BLOCK_SIZE_64 * BLOCK_SIZE_64;
    } else {
        if (enc_mode <= ENC_M2) {
            me_ctx->me_early_exit_th = 0;
        } else if ((!rtc_tune && enc_mode <= ENC_M10) || (rtc_tune && !scs->use_flat_ipp && enc_mode <= ENC_M9) ||
                   (rtc_tune && scs->use_flat_ipp && enc_mode <= ENC_M10)) {
            me_ctx->me_early_exit_th = BLOCK_SIZE_64 * BLOCK_SIZE_64 * 8;
        } else {
            me_ctx->me_early_exit_th = BLOCK_SIZE_64 * BLOCK_SIZE_64 * 12;
        }
    }

    me_ctx->me_safe_limit_zz_th = scs->mrp_ctrls.safe_limit_nref == 1 ? scs->mrp_ctrls.safe_limit_zz_th : 0;

    me_ctx->prev_me_stage_based_exit_th = 0;
    if (rtc_tune && sc_class1) {
        me_ctx->prev_me_stage_based_exit_th = BLOCK_SIZE_64 * BLOCK_SIZE_64 * 4;
    } else {
        me_ctx->prev_me_stage_based_exit_th = 0;
    }
};

/******************************************************
* Derive ME Settings for OQ for Altref Temporal Filtering
  Input   : encoder mode and tune
  Output  : ME Kernel signal(s)
******************************************************/
void svt_aom_sig_deriv_me_tf(PictureParentControlSet* pcs, MeContext* me_ctx) {
    // Set ME/HME search regions
    tf_set_me_hme_params_oq(me_ctx, pcs);
    // Set HME flags
    me_ctx->enable_hme_flag        = pcs->tf_enable_hme_flag;
    me_ctx->enable_hme_level0_flag = pcs->tf_enable_hme_level0_flag;
    me_ctx->enable_hme_level1_flag = pcs->tf_enable_hme_level1_flag;
    me_ctx->enable_hme_level2_flag = pcs->tf_enable_hme_level2_flag;
    if (pcs->tf_ctrls.hme_me_level <= 2) {
        // HME Search Method
        me_ctx->hme_search_method = FULL_SAD_SEARCH;
        // ME Search Method
        me_ctx->me_search_method = FULL_SAD_SEARCH;
    } else {
        // HME Search Method
        me_ctx->hme_search_method = SUB_SAD_SEARCH;
        // ME Search Method
        me_ctx->me_search_method = SUB_SAD_SEARCH;
    }

    uint8_t prehme_level = 0;
    svt_aom_set_prehme_ctrls(me_ctx, prehme_level);

    // Set hme/me based reference pruning level (0-4)
    // Ref pruning is disallowed for TF in motion_estimate_sb()
    svt_aom_set_me_hme_ref_prune_ctrls(me_ctx, 0);

    // Set hme-based me sr adjustment level
    // ME SR adjustment is disallowed for TF in motion_estimate_sb()
    svt_aom_set_me_sr_adjustment_ctrls(me_ctx, 0);

    svt_aom_set_mv_based_sa_ctrls(me_ctx, 0);

    svt_aom_set_me_8x8_var_ctrls(me_ctx, 0);

    me_ctx->sc_class_me_boost           = 0;
    me_ctx->me_early_exit_th            = pcs->tf_ctrls.hme_me_level <= 1 ? 0 : BLOCK_SIZE_64 * BLOCK_SIZE_64 * 4;
    me_ctx->me_safe_limit_zz_th         = 0;
    me_ctx->reduce_hme_l0_sr_th_min     = 0;
    me_ctx->reduce_hme_l0_sr_th_max     = 0;
    me_ctx->prev_me_stage_based_exit_th = pcs->tf_ctrls.hme_me_level <= 1 ? 0 : BLOCK_SIZE_64 * BLOCK_SIZE_64 * 4;
};

static void set_cdef_search_controls(PictureParentControlSet* pcs, uint8_t cdef_search_level) {
    CdefSearchControls* cdef_ctrls           = &pcs->cdef_search_ctrls;
    const bool          is_base              = pcs->temporal_layer_index == 0;
    const bool          is_not_highest_layer = !pcs->is_highest_layer;
    int                 i, j, sf_idx, second_pass_fs_num;
    switch (cdef_search_level) {
        // OFF
    case 0:
        cdef_ctrls->enabled               = 0;
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
        break;
    case 1:
        // pf_set {0,1,..,15}
        // sf_set {0,1,2,3}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 16;
        second_pass_fs_num                     = 3;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[1];
        cdef_ctrls->default_first_pass_fs[2]   = pf_gi[2];
        cdef_ctrls->default_first_pass_fs[3]   = pf_gi[3];
        cdef_ctrls->default_first_pass_fs[4]   = pf_gi[4];
        cdef_ctrls->default_first_pass_fs[5]   = pf_gi[5];
        cdef_ctrls->default_first_pass_fs[6]   = pf_gi[6];
        cdef_ctrls->default_first_pass_fs[7]   = pf_gi[7];
        cdef_ctrls->default_first_pass_fs[8]   = pf_gi[8];
        cdef_ctrls->default_first_pass_fs[9]   = pf_gi[9];
        cdef_ctrls->default_first_pass_fs[10]  = pf_gi[10];
        cdef_ctrls->default_first_pass_fs[11]  = pf_gi[11];
        cdef_ctrls->default_first_pass_fs[12]  = pf_gi[12];
        cdef_ctrls->default_first_pass_fs[13]  = pf_gi[13];
        cdef_ctrls->default_first_pass_fs[14]  = pf_gi[14];
        cdef_ctrls->default_first_pass_fs[15]  = pf_gi[15];
        sf_idx                                 = 0;
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            int pf_idx = cdef_ctrls->default_first_pass_fs[i];
            for (j = 1; j < 4; j++) {
                cdef_ctrls->default_second_pass_fs[sf_idx] = pf_idx + j;
                sf_idx++;
            }
        }
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            cdef_ctrls->default_first_pass_fs_uv[i] = cdef_ctrls->default_first_pass_fs[i];
        }
        for (i = 0; i < cdef_ctrls->default_second_pass_fs_num; i++) {
            cdef_ctrls->default_second_pass_fs_uv[i] = cdef_ctrls->default_second_pass_fs[i];
        }
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->search_best_ref_fs    = 0;
        cdef_ctrls->subsampling_factor    = 1;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 2:
        // pf_set {0,1,2,4,5,6,8,9,10,12,13,14}
        // sf_set {0,1,..,3}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 12;
        second_pass_fs_num                     = 3;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[1];
        cdef_ctrls->default_first_pass_fs[2]   = pf_gi[2];
        cdef_ctrls->default_first_pass_fs[3]   = pf_gi[4];
        cdef_ctrls->default_first_pass_fs[4]   = pf_gi[5];
        cdef_ctrls->default_first_pass_fs[5]   = pf_gi[6];
        cdef_ctrls->default_first_pass_fs[6]   = pf_gi[8];
        cdef_ctrls->default_first_pass_fs[7]   = pf_gi[9];
        cdef_ctrls->default_first_pass_fs[8]   = pf_gi[10];
        cdef_ctrls->default_first_pass_fs[9]   = pf_gi[12];
        cdef_ctrls->default_first_pass_fs[10]  = pf_gi[13];
        cdef_ctrls->default_first_pass_fs[11]  = pf_gi[14];
        sf_idx                                 = 0;
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            int pf_idx = cdef_ctrls->default_first_pass_fs[i];
            for (j = 1; j < 4; j++) {
                cdef_ctrls->default_second_pass_fs[sf_idx] = pf_idx + j;
                sf_idx++;
            }
        }
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            cdef_ctrls->default_first_pass_fs_uv[i] = cdef_ctrls->default_first_pass_fs[i];
        }
        for (i = 0; i < cdef_ctrls->default_second_pass_fs_num; i++) {
            cdef_ctrls->default_second_pass_fs_uv[i] = -1; // cdef_ctrls->default_second_pass_fs[i];
        }
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->search_best_ref_fs    = 0;
        cdef_ctrls->subsampling_factor    = 1;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 3:
        // pf_set {0,4,8,12,15}
        // sf_set {0,1,..,3}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 5;
        second_pass_fs_num                     = 3;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[4];
        cdef_ctrls->default_first_pass_fs[2]   = pf_gi[8];
        cdef_ctrls->default_first_pass_fs[3]   = pf_gi[12];
        cdef_ctrls->default_first_pass_fs[4]   = pf_gi[15];
        sf_idx                                 = 0;
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            int pf_idx = cdef_ctrls->default_first_pass_fs[i];
            for (j = 1; j < 4; j++) {
                cdef_ctrls->default_second_pass_fs[sf_idx] = pf_idx + j;
                sf_idx++;
            }
        }
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            cdef_ctrls->default_first_pass_fs_uv[i] = cdef_ctrls->default_first_pass_fs[i];
        }
        for (i = 0; i < cdef_ctrls->default_second_pass_fs_num; i++) {
            cdef_ctrls->default_second_pass_fs_uv[i] = -1; // cdef_ctrls->default_second_pass_fs[i];
        }
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->search_best_ref_fs    = 0;
        cdef_ctrls->subsampling_factor    = 1;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 4:
        // pf_set {0,7,15}
        // sf_set {0,1,..,3}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 3;
        second_pass_fs_num                     = 3;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[7];
        cdef_ctrls->default_first_pass_fs[2]   = pf_gi[15];
        sf_idx                                 = 0;
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            int pf_idx = cdef_ctrls->default_first_pass_fs[i];
            for (j = 1; j < 4; j++) {
                cdef_ctrls->default_second_pass_fs[sf_idx] = pf_idx + j;
                sf_idx++;
            }
        }
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            cdef_ctrls->default_first_pass_fs_uv[i] = cdef_ctrls->default_first_pass_fs[i];
        }
        for (i = 0; i < cdef_ctrls->default_second_pass_fs_num; i++) {
            cdef_ctrls->default_second_pass_fs_uv[i] = -1; // cdef_ctrls->default_second_pass_fs[i];
        }
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->search_best_ref_fs    = 0;
        cdef_ctrls->subsampling_factor    = 1;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 5:
        // pf_set {0,7,15}
        // sf_set {0,2}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 3;
        second_pass_fs_num                     = 1;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[7];
        cdef_ctrls->default_first_pass_fs[2]   = pf_gi[15];

        cdef_ctrls->default_second_pass_fs[0] = pf_gi[0] + 2;
        cdef_ctrls->default_second_pass_fs[1] = pf_gi[7] + 2;
        cdef_ctrls->default_second_pass_fs[2] = pf_gi[15] + 2;
        for (i = 0; i < cdef_ctrls->first_pass_fs_num; i++) {
            cdef_ctrls->default_first_pass_fs_uv[i] = cdef_ctrls->default_first_pass_fs[i];
        }
        for (i = 0; i < cdef_ctrls->default_second_pass_fs_num; i++) {
            cdef_ctrls->default_second_pass_fs_uv[i] = -1; // cdef_ctrls->default_second_pass_fs[i];
        }
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->search_best_ref_fs    = is_not_highest_layer ? 0 : 1;
        cdef_ctrls->subsampling_factor    = 1;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 6:
        // pf_set {0,15}
        // sf_set {0,2}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 2;
        second_pass_fs_num                     = 1;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[15];

        cdef_ctrls->default_second_pass_fs[0] = pf_gi[0] + 2;
        cdef_ctrls->default_second_pass_fs[1] = pf_gi[15] + 2;

        cdef_ctrls->default_first_pass_fs_uv[0]  = cdef_ctrls->default_first_pass_fs[0];
        cdef_ctrls->default_first_pass_fs_uv[1]  = cdef_ctrls->default_first_pass_fs[1];
        cdef_ctrls->default_first_pass_fs_uv[2]  = -1; // when using search_best_ref_fs, set at least 3 filters
        cdef_ctrls->default_second_pass_fs_uv[0] = -1;
        cdef_ctrls->default_second_pass_fs_uv[1] = -1;

        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->search_best_ref_fs    = is_not_highest_layer ? 0 : 1;
        cdef_ctrls->subsampling_factor    = 4;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 7:
        // pf_set {0,15}
        // sf_set {0,2}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 2;
        second_pass_fs_num                     = 1;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[15];

        cdef_ctrls->default_second_pass_fs[0]    = pf_gi[0] + 2;
        cdef_ctrls->default_second_pass_fs[1]    = pf_gi[15] + 2;
        cdef_ctrls->default_first_pass_fs_uv[0]  = cdef_ctrls->default_first_pass_fs[0];
        cdef_ctrls->default_first_pass_fs_uv[1]  = cdef_ctrls->default_first_pass_fs[1];
        cdef_ctrls->default_first_pass_fs_uv[2]  = -1; // if using search_best_ref_fs, set at least 3 filters
        cdef_ctrls->default_second_pass_fs_uv[0] = -1; // cdef_ctrls->default_second_pass_fs[0];
        cdef_ctrls->default_second_pass_fs_uv[1] = -1; // cdef_ctrls->default_second_pass_fs[1];

        cdef_ctrls->use_reference_cdef_fs = is_not_highest_layer ? 0 : 1;
        cdef_ctrls->search_best_ref_fs    = is_base ? 0 : 1;
        cdef_ctrls->subsampling_factor    = 4;
        cdef_ctrls->use_skip_detector     = is_base ? 0 : 1;
        cdef_ctrls->uv_from_y             = false;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;
    case 8:
        // pf_set {0,15}
        // sf_set {0,2}
        cdef_ctrls->enabled                    = 1;
        cdef_ctrls->first_pass_fs_num          = 2;
        second_pass_fs_num                     = 1;
        cdef_ctrls->default_second_pass_fs_num = cdef_ctrls->first_pass_fs_num * second_pass_fs_num;
        cdef_ctrls->default_first_pass_fs[0]   = pf_gi[0];
        cdef_ctrls->default_first_pass_fs[1]   = pf_gi[15];

        cdef_ctrls->default_second_pass_fs[0]    = pf_gi[0] + 2;
        cdef_ctrls->default_second_pass_fs[1]    = pf_gi[15] + 2;
        cdef_ctrls->default_first_pass_fs_uv[0]  = -1;
        cdef_ctrls->default_first_pass_fs_uv[1]  = -1;
        cdef_ctrls->default_first_pass_fs_uv[2]  = -1; // if using search_best_ref_fs, set at least 3 filters
        cdef_ctrls->default_second_pass_fs_uv[0] = -1; // cdef_ctrls->default_second_pass_fs[0];
        cdef_ctrls->default_second_pass_fs_uv[1] = -1; // cdef_ctrls->default_second_pass_fs[1];

        cdef_ctrls->use_reference_cdef_fs = is_base ? 0 : 1;
        cdef_ctrls->search_best_ref_fs    = is_base ? 0 : 1;
        cdef_ctrls->subsampling_factor    = 4;
        cdef_ctrls->use_skip_detector     = 0;
        cdef_ctrls->uv_from_y             = true;
#if OPT_Q_CDEF
        cdef_ctrls->use_qp_strength = false;
#endif
        break;

#if OPT_Q_CDEF
    case 9:
        cdef_ctrls->enabled               = 1;
        cdef_ctrls->use_reference_cdef_fs = 0;
        cdef_ctrls->use_qp_strength       = true;
        break;
#endif
    default:
        assert(0);
        break;
    }
    // If chroma filters will be copied from luma, set chroma filters to -1 to avoid testing
#if OPT_Q_CDEF
    if (cdef_ctrls->uv_from_y && !cdef_ctrls->use_qp_strength) {
#else
    if (cdef_ctrls->uv_from_y) {
#endif
        int fs_idx;
        for (fs_idx = 0; fs_idx < cdef_ctrls->first_pass_fs_num; fs_idx++) {
            cdef_ctrls->default_first_pass_fs_uv[fs_idx] = -1;
        }
        for (fs_idx = 0; fs_idx < cdef_ctrls->default_second_pass_fs_num; fs_idx++) {
            cdef_ctrls->default_second_pass_fs_uv[fs_idx] = -1;
        }
    }
}

static void set_cdef_recon_controls(PictureParentControlSet* pcs, uint8_t cdef_recon_level) {
    CdefReconControls* cdef_ctrls = &pcs->cdef_recon_ctrls;
    switch (cdef_recon_level) {
    case 0: // OFF
        cdef_ctrls->zero_fs_cost_bias        = 0;
        cdef_ctrls->zero_filter_strength_lvl = 0;
        cdef_ctrls->prev_cdef_dist_th        = 0;
        break;
    case 1:
        cdef_ctrls->zero_fs_cost_bias        = 61;
        cdef_ctrls->zero_filter_strength_lvl = 2;
        cdef_ctrls->prev_cdef_dist_th        = 10;
        break;
    case 2:
        cdef_ctrls->zero_fs_cost_bias        = 61;
        cdef_ctrls->zero_filter_strength_lvl = 3;
        cdef_ctrls->prev_cdef_dist_th        = 10;
        break;
    case 3: // old level 4
        cdef_ctrls->zero_fs_cost_bias        = 60;
        cdef_ctrls->zero_filter_strength_lvl = 3;
        cdef_ctrls->prev_cdef_dist_th        = 10;
        break;
    case 4: // old level 5
        cdef_ctrls->zero_fs_cost_bias        = 58;
        cdef_ctrls->zero_filter_strength_lvl = 3;
        cdef_ctrls->prev_cdef_dist_th        = 10;
        break;
    default:
        assert(0);
        break;
    }
}

static void svt_aom_set_wn_filter_ctrls(Av1Common* cm, uint8_t wn_filter_lvl) {
    WnFilterCtrls* ctrls = &cm->wn_filter_ctrls;

    switch (wn_filter_lvl) {
    case 0:
        ctrls->enabled = 0;
        break;
    case 1:
        ctrls->enabled                 = 1;
        ctrls->use_chroma              = 1;
        ctrls->filter_tap_lvl          = 1;
        ctrls->use_refinement          = 1;
        ctrls->max_one_refinement_step = 0;
        ctrls->use_prev_frame_coeffs   = 0;
        break;
    case 2:
        ctrls->enabled                 = 1;
        ctrls->use_chroma              = 1;
        ctrls->filter_tap_lvl          = 1;
        ctrls->use_refinement          = 1;
        ctrls->max_one_refinement_step = 1;
        ctrls->use_prev_frame_coeffs   = 0;
        break;
    case 3:
        ctrls->enabled                 = 1;
        ctrls->use_chroma              = 1;
        ctrls->filter_tap_lvl          = 2;
        ctrls->use_refinement          = 1;
        ctrls->max_one_refinement_step = 1;
        ctrls->use_prev_frame_coeffs   = 0;
        break;
    case 4:
        ctrls->enabled                 = 1;
        ctrls->use_chroma              = 1;
        ctrls->filter_tap_lvl          = 2;
        ctrls->use_refinement          = 0;
        ctrls->max_one_refinement_step = 1;
        ctrls->use_prev_frame_coeffs   = 0;
        break;
    case 5:
        ctrls->enabled                 = 1;
        ctrls->use_chroma              = 0;
        ctrls->filter_tap_lvl          = 2;
        ctrls->use_refinement          = 0;
        ctrls->max_one_refinement_step = 1;
        ctrls->use_prev_frame_coeffs   = 0;
        break;
    case 6:
        ctrls->enabled                 = 1;
        ctrls->use_chroma              = 0;
        ctrls->filter_tap_lvl          = 2;
        ctrls->use_refinement          = 0;
        ctrls->max_one_refinement_step = 1;
        ctrls->use_prev_frame_coeffs   = 1;
        break;
    default:
        assert(0);
        break;
    }
}

static void svt_aom_set_sg_filter_ctrls(Av1Common* cm, uint8_t sg_filter_lvl) {
    SgFilterCtrls* ctrls = &cm->sg_filter_ctrls;

    switch (sg_filter_lvl) {
    case 0:
        ctrls->enabled = 0;
        break;
    case 1:
        ctrls->enabled     = 1;
        ctrls->use_chroma  = 1;
        ctrls->start_ep[0] = 0;
        ctrls->end_ep[0]   = 16;
        ctrls->ep_inc[0]   = 1;
        ctrls->start_ep[1] = 0;
        ctrls->end_ep[1]   = 16;
        ctrls->ep_inc[1]   = 1;
        ctrls->refine[0]   = 1;
        ctrls->refine[1]   = 1;
        break;
    case 2:
        ctrls->enabled     = 1;
        ctrls->use_chroma  = 1;
        ctrls->start_ep[0] = 0;
        ctrls->end_ep[0]   = 16;
        ctrls->ep_inc[0]   = 1;
        ctrls->start_ep[1] = 4;
        ctrls->end_ep[1]   = 5;
        ctrls->ep_inc[1]   = 1;
        ctrls->refine[0]   = 1;
        ctrls->refine[1]   = 0;
        break;
    case 3:
        ctrls->enabled     = 1;
        ctrls->use_chroma  = 1;
        ctrls->start_ep[0] = 0;
        ctrls->end_ep[0]   = 16;
        ctrls->ep_inc[0]   = 8;
        ctrls->start_ep[1] = 4;
        ctrls->end_ep[1]   = 5;
        ctrls->ep_inc[1]   = 1;
        ctrls->refine[0]   = 1;
        ctrls->refine[1]   = 0;
        break;
    case 4:
        ctrls->enabled     = 1;
        ctrls->use_chroma  = 0;
        ctrls->start_ep[0] = 0;
        ctrls->end_ep[0]   = 16;
        ctrls->ep_inc[0]   = 8;
        ctrls->start_ep[1] = 4;
        ctrls->end_ep[1]   = 5;
        ctrls->ep_inc[1]   = 1;
        ctrls->refine[0]   = 1;
        ctrls->refine[1]   = 0;
        break;
    default:
        assert(0);
        break;
    }
}

// Returns the level for Wiener restoration filter
#if TUNE_STILL_IMAGE
static uint8_t svt_aom_get_wn_filter_level_default(EncMode enc_mode, uint8_t input_resolution, bool is_not_last_layer) {
    uint8_t wn_filter_lvl = 0;

    if (enc_mode <= ENC_M8) {
        wn_filter_lvl = is_not_last_layer ? 5 : 0;
    } else {
        wn_filter_lvl = 0;
    }
    // Disable wiener restoration for resolutions 8K and above, unless allintra coding is used (due to memory constraints)
    if (input_resolution >= INPUT_SIZE_8K_RANGE) {
        wn_filter_lvl = 0;
    }

    return wn_filter_lvl;
}

static uint8_t svt_aom_get_wn_filter_level_rtc(EncMode enc_mode, uint8_t input_resolution, bool is_not_last_layer) {
    uint8_t wn_filter_lvl = 0;

    if (enc_mode <= ENC_M6) {
        wn_filter_lvl = is_not_last_layer ? 5 : 0;
    } else {
        wn_filter_lvl = 0;
    }
    // Disable wiener restoration for resolutions 8K and above, unless allintra coding is used (due to memory constraints)
    if (input_resolution >= INPUT_SIZE_8K_RANGE) {
        wn_filter_lvl = 0;
    }

    return wn_filter_lvl;
}

static uint8_t svt_aom_get_wn_filter_level_allintra(EncMode enc_mode) {
    uint8_t wn_filter_lvl;
    if (enc_mode <= ENC_M3) {
        wn_filter_lvl = 3;
    } else if (enc_mode <= ENC_M6) {
        wn_filter_lvl = 4;
    } else if (enc_mode <= ENC_M7) {
        wn_filter_lvl = 5;
    } else {
        wn_filter_lvl = 0;
    }

    return wn_filter_lvl;
}
#else
static uint8_t svt_aom_get_wn_filter_level(EncMode enc_mode, uint8_t input_resolution, bool is_not_last_layer,
                                           bool allintra, bool rtc_tune) {
    uint8_t wn_filter_lvl = 0;
    if (allintra) {
        if (enc_mode <= ENC_M2) {
            wn_filter_lvl = 3;
        } else if (enc_mode <= ENC_M7) {
            wn_filter_lvl = 4;
        } else if (enc_mode <= ENC_M10) {
            wn_filter_lvl = 5;
        } else {
            wn_filter_lvl = 0;
        }
    } else if ((enc_mode <= ENC_M8 && !rtc_tune) || (enc_mode <= ENC_M6 && rtc_tune)) {
        wn_filter_lvl = is_not_last_layer ? 5 : 0;
    } else {
        wn_filter_lvl = 0;
    }
    // Disable wiener restoration for resolutions 8K and above, unless allintra coding is used (due to memory constraints)
    if (!allintra && input_resolution >= INPUT_SIZE_8K_RANGE) {
        wn_filter_lvl = 0;
    }

    return wn_filter_lvl;
}
#endif

// Returns the level for self-guided restoration filter
#if TUNE_STILL_IMAGE
static uint8_t svt_aom_get_sg_filter_level_default(EncMode enc_mode, uint8_t input_resolution, uint8_t fast_decode) {
    uint8_t sg_filter_lvl;
    if (enc_mode <= ENC_MR) {
        sg_filter_lvl = 1;
    } else if (enc_mode <= ENC_M3) {
        sg_filter_lvl = 3;
    } else {
        sg_filter_lvl = 0;
    }

    // Disable self-guided restoration for resolutions 8K and above, unless allintra coding is used (due to memory constraints)
    if ((input_resolution >= INPUT_SIZE_8K_RANGE) || (fast_decode && !(input_resolution <= INPUT_SIZE_360p_RANGE))) {
        sg_filter_lvl = 0;
    }

    return sg_filter_lvl;
}

static uint8_t svt_aom_get_sg_filter_level_rtc(EncMode enc_mode, uint8_t input_resolution, uint8_t fast_decode) {
    uint8_t sg_filter_lvl;
    if (enc_mode <= ENC_MR) {
        sg_filter_lvl = 1;
    } else if (enc_mode <= ENC_M3) {
        sg_filter_lvl = 3;
    } else {
        sg_filter_lvl = 0;
    }

    // Disable self-guided restoration for resolutions 8K and above, unless allintra coding is used (due to memory constraints)
    if ((input_resolution >= INPUT_SIZE_8K_RANGE) || (fast_decode && !(input_resolution <= INPUT_SIZE_360p_RANGE))) {
        sg_filter_lvl = 0;
    }

    return sg_filter_lvl;
}

static uint8_t svt_aom_get_sg_filter_level_allintra() {
    uint8_t sg_filter_lvl;
    sg_filter_lvl = 0;

    return sg_filter_lvl;
}
#else
static uint8_t svt_aom_get_sg_filter_level(EncMode enc_mode, uint8_t input_resolution, uint8_t fast_decode,
                                           bool allintra) {
    uint8_t sg_filter_lvl = 0;
    if (allintra) {
        if (enc_mode <= ENC_MR) {
            sg_filter_lvl = 1;
        } else {
            sg_filter_lvl = 0;
        }
    } else if (enc_mode <= ENC_MR) {
        sg_filter_lvl = 1;
    } else if (enc_mode <= ENC_M3) {
        sg_filter_lvl = 3;
    } else {
        sg_filter_lvl = 0;
    }

    // Disable self-guided restoration for resolutions 8K and above, unless allintra coding is used (due to memory constraints)
    if ((!allintra && input_resolution >= INPUT_SIZE_8K_RANGE) ||
        (fast_decode && !(input_resolution <= INPUT_SIZE_360p_RANGE))) {
        sg_filter_lvl = 0;
    }

    return sg_filter_lvl;
}
#endif
static void dlf_level_modulation(PictureControlSet* pcs, uint8_t* default_dlf_level, uint8_t modulation_mode) {
    uint8_t dlf_level = *default_dlf_level;

    if (modulation_mode == 1 || modulation_mode == 2) {
        if (pcs->ref_skip_percentage < 25) {
            dlf_level = dlf_level == 0 ? 6 : dlf_level > 5 ? MAX(5, dlf_level - 2) : dlf_level;
        } else if (pcs->ref_skip_percentage < 50) {
            dlf_level = dlf_level == 0 ? 7 : dlf_level > 5 ? dlf_level - 1 : dlf_level;
        }
    }

    if (modulation_mode == 2 || modulation_mode == 3) {
        if (dlf_level > 4) {
            if (pcs->ref_skip_percentage > 95) {
                dlf_level = dlf_level >= 6 ? 0 : dlf_level + 2;
            } else if (pcs->ref_skip_percentage > 75) {
                dlf_level = dlf_level == 7 ? 0 : dlf_level + 1;
            }
        }
    }

    *default_dlf_level = dlf_level;
}
#if TUNE_STILL_IMAGE
static uint8_t get_dlf_level_default(PictureControlSet* pcs, EncMode enc_mode, uint8_t is_not_last_layer,
                                     uint8_t fast_decode, EbInputResolution resolution, int is_base) {
    const uint8_t sc_class1       = pcs->ppcs->sc_class1;
    uint8_t       dlf_level       = 0;
    uint8_t       modulation_mode = 0; // 0: off, 1: only towards bd-rate, 2: both sides; , 3: only towards speed

    if (fast_decode <= 1 || resolution <= INPUT_SIZE_360p_RANGE) { // fast-decode 0 && fast-decode 1
        if (enc_mode <= ENC_M0) {
            dlf_level = 1;
        } else if ((!sc_class1 && enc_mode <= ENC_M3) || (sc_class1 && enc_mode <= ENC_M4)) {
            dlf_level = 2;
        } else if (enc_mode <= ENC_M6) {
            dlf_level = is_not_last_layer ? 3 : 6;
        } else if (enc_mode <= ENC_M7) {
            dlf_level       = is_not_last_layer ? 3 : 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M9) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M11) {
            if (pcs->coeff_lvl == HIGH_LVL) {
                dlf_level = is_base ? 6 : 0;
            } else {
                dlf_level = is_base ? 6 : is_not_last_layer ? 7 : 0;
            }
            modulation_mode = 3;
        } else {
            dlf_level       = 0;
            modulation_mode = 3;
        }
    } else { // fast-decode 2
        if (enc_mode <= ENC_M6) {
            dlf_level = 4;
        } else if (enc_mode <= ENC_M7) {
            dlf_level       = 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M10) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else {
            dlf_level       = is_not_last_layer ? 7 : 0;
            modulation_mode = 3;
        }
    }

    if (!is_base) {
        dlf_level_modulation(pcs, &dlf_level, modulation_mode);
    }

    return dlf_level;
}

static uint8_t get_dlf_level_rtc(PictureControlSet* pcs, EncMode enc_mode, uint8_t is_not_last_layer, int is_base) {
    uint8_t dlf_level       = 0;
    uint8_t modulation_mode = 0; // 0: off, 1: only towards bd-rate, 2: both sides; , 3: only towards speed

    if (pcs->scs->use_flat_ipp) {
        if (enc_mode <= ENC_M9) {
            dlf_level       = 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M10) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else {
            dlf_level       = 0;
            modulation_mode = 3;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            dlf_level       = 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M9) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else {
            dlf_level       = 0;
            modulation_mode = 3;
        }
    }

    if (!is_base) {
        dlf_level_modulation(pcs, &dlf_level, modulation_mode);
    }

    return dlf_level;
}

static uint8_t get_dlf_level_allintra(EncMode enc_mode, uint8_t fast_decode, EbInputResolution resolution) {
    uint8_t dlf_level = 0;
    if (fast_decode <= 1 || resolution <= INPUT_SIZE_360p_RANGE) {
        if (enc_mode <= ENC_M3) {
            dlf_level = 1;
        } else if (enc_mode <= ENC_M5) {
            dlf_level = 2;
        } else {
            dlf_level = 5;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            dlf_level = 0;
        } else {
            dlf_level = 5;
        }
    }

    return dlf_level;
}
#else
static uint8_t get_dlf_level(PictureControlSet* pcs, EncMode enc_mode, uint8_t is_not_last_layer, uint8_t fast_decode,
                             EbInputResolution resolution, bool allintra, int is_base) {
    const uint8_t sc_class1       = pcs->ppcs->sc_class1;
    uint8_t       dlf_level       = 0;
    uint8_t       modulation_mode = 0; // 0: off, 1: only towards bd-rate, 2: both sides; , 3: only towards speed
    const bool    rtc_tune        = pcs->scs->static_config.rtc;
    const bool    flat_rtc        = rtc_tune && pcs->scs->use_flat_ipp;

    if (allintra) {
        if (pcs->scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (fast_decode <= 1 || resolution <= INPUT_SIZE_360p_RANGE) {
                if (enc_mode <= ENC_M3) {
                    dlf_level = 1;
                } else if (enc_mode <= ENC_M6) {
                    dlf_level = 2;
                } else {
                    dlf_level       = 5;
                    modulation_mode = 3;
                }
            } else {
                if (enc_mode <= ENC_M9) {
                    dlf_level       = 0;
                    modulation_mode = 3;
                } else {
                    dlf_level       = 5;
                    modulation_mode = 3;
                }
            }
        } else {
            if (enc_mode <= ENC_M0) {
                dlf_level = 1;
            } else {
                dlf_level       = 5;
                modulation_mode = 3;
            }
        }
    } else if (flat_rtc) { // RTC FLAT
        if (enc_mode <= ENC_M9) {
            dlf_level       = 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M10) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else {
            dlf_level       = 0;
            modulation_mode = 3;
        }
    } else if (rtc_tune) { // RTC Non-FLAT
        if (enc_mode <= ENC_M7) {
            dlf_level       = 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M9) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else {
            dlf_level       = 0;
            modulation_mode = 3;
        }
    } else if (fast_decode <= 1 || resolution <= INPUT_SIZE_360p_RANGE) { // fast-decode 0 && fast-decode 1
        if (enc_mode <= ENC_M0) {
            dlf_level = 1;
        } else if ((!sc_class1 && enc_mode <= ENC_M3) || (sc_class1 && enc_mode <= ENC_M4)) {
            dlf_level = 2;
        } else if (enc_mode <= ENC_M6) {
            dlf_level = is_not_last_layer ? 3 : 6;
        } else if (enc_mode <= ENC_M7) {
            dlf_level       = is_not_last_layer ? 3 : 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M9) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M11) {
            if (pcs->coeff_lvl == HIGH_LVL) {
                dlf_level = is_base ? 6 : 0;
            } else {
                dlf_level = is_base ? 6 : is_not_last_layer ? 7 : 0;
            }
            modulation_mode = 3;
        } else {
            dlf_level       = 0;
            modulation_mode = 3;
        }
    } else { // fast-decode 2
        if (enc_mode <= ENC_M6) {
            dlf_level = 4;
        } else if (enc_mode <= ENC_M7) {
            dlf_level       = 6;
            modulation_mode = 3;
        } else if (enc_mode <= ENC_M10) {
            dlf_level       = is_not_last_layer ? 6 : 0;
            modulation_mode = 3;
        } else {
            dlf_level       = is_not_last_layer ? 7 : 0;
            modulation_mode = 3;
        }
    }

    if (!is_base) {
        dlf_level_modulation(pcs, &dlf_level, modulation_mode);
    }

    return dlf_level;
}
#endif
static void svt_aom_set_dlf_controls(PictureParentControlSet* pcs, uint8_t dlf_level) {
    DlfCtrls* ctrls = &pcs->dlf_ctrls;

    switch (dlf_level) {
    case 0:
        ctrls->enabled                  = 0;
        ctrls->sb_based_dlf             = 0;
        ctrls->dlf_avg                  = 0;
        ctrls->use_ref_avg_y            = 0;
        ctrls->use_ref_avg_uv           = 0;
        ctrls->early_exit_convergence   = 0;
        ctrls->zero_filter_strength_lvl = 0;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    case 1:
        ctrls->enabled                  = 1;
        ctrls->sb_based_dlf             = 0;
        ctrls->dlf_avg                  = 0;
        ctrls->use_ref_avg_y            = 0;
        ctrls->use_ref_avg_uv           = 0;
        ctrls->early_exit_convergence   = 0;
        ctrls->zero_filter_strength_lvl = 0;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    case 2:
        ctrls->enabled                  = 1;
        ctrls->sb_based_dlf             = 0;
        ctrls->dlf_avg                  = 1;
        ctrls->use_ref_avg_y            = 0;
        ctrls->use_ref_avg_uv           = 1;
        ctrls->early_exit_convergence   = 1;
        ctrls->zero_filter_strength_lvl = 0;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    case 3:
        ctrls->enabled                  = 1;
        ctrls->sb_based_dlf             = 0;
        ctrls->dlf_avg                  = 1;
        ctrls->use_ref_avg_y            = 1;
        ctrls->use_ref_avg_uv           = 1;
        ctrls->early_exit_convergence   = 1;
        ctrls->zero_filter_strength_lvl = 0;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    case 4:
        ctrls->enabled                  = 1;
        ctrls->sb_based_dlf             = 0;
        ctrls->dlf_avg                  = 1;
        ctrls->use_ref_avg_y            = 1;
        ctrls->use_ref_avg_uv           = 1;
        ctrls->early_exit_convergence   = 1;
        ctrls->zero_filter_strength_lvl = 2;
        ctrls->prev_dlf_dist_th         = 10;
        break;
    case 5:
        ctrls->enabled      = 1;
        ctrls->sb_based_dlf = 1;

        ctrls->dlf_avg                  = 0;
        ctrls->use_ref_avg_y            = 0;
        ctrls->use_ref_avg_uv           = 0;
        ctrls->early_exit_convergence   = 0;
        ctrls->zero_filter_strength_lvl = 1;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    case 6:
        ctrls->enabled                  = 1;
        ctrls->sb_based_dlf             = 1;
        ctrls->dlf_avg                  = 0;
        ctrls->use_ref_avg_y            = 0;
        ctrls->use_ref_avg_uv           = 0;
        ctrls->early_exit_convergence   = 0;
        ctrls->zero_filter_strength_lvl = 2;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    case 7:
        ctrls->enabled                  = 1;
        ctrls->sb_based_dlf             = 1;
        ctrls->dlf_avg                  = 0;
        ctrls->use_ref_avg_y            = 0;
        ctrls->use_ref_avg_uv           = 0;
        ctrls->early_exit_convergence   = 0;
        ctrls->zero_filter_strength_lvl = 3;
        ctrls->prev_dlf_dist_th         = 0;
        break;
    default:
        assert(0);
        break;
    }
}

/*
    set controls for intra block copy
*/
static void set_intrabc_level(PictureParentControlSet* pcs, SequenceControlSet* scs, uint8_t ibc_level) {
    IntraBCCtrls* intraBC_ctrls = &pcs->intraBC_ctrls;
#if TUNE_STILL_IMAGE
    bool disallow_4x4;
    if (pcs->scs->allintra) {
        disallow_4x4 = svt_aom_get_disallow_4x4_allintra(pcs->enc_mode);
    } else if (scs->static_config.rtc) {
        disallow_4x4 = svt_aom_get_disallow_4x4_rtc(pcs->enc_mode);
    } else {
        disallow_4x4 = svt_aom_get_disallow_4x4_default(pcs->enc_mode);
    }
#else
    uint8_t allow_4x4_blocks = !svt_aom_get_disallow_4x4_default(pcs->enc_mode);
#endif
    switch (ibc_level) {
    case 0:
        intraBC_ctrls->enabled = 0;
        break;
#if CLN_INTRABC_LEVEL_DEF
    case 1:
        intraBC_ctrls->enabled       = 1;
        intraBC_ctrls->ibc_shift     = 0;
        intraBC_ctrls->ibc_direction = 0;
#if TUNE_STILL_IMAGE
        intraBC_ctrls->hash_4x4_blocks = !disallow_4x4;
#else
        intraBC_ctrls->hash_4x4_blocks = allow_4x4_blocks;
#endif
        intraBC_ctrls->max_block_size_hash = scs->super_block_size;
        break;
    case 2:
        intraBC_ctrls->enabled       = 1;
        intraBC_ctrls->ibc_shift     = 1;
        intraBC_ctrls->ibc_direction = 0;
#if TUNE_STILL_IMAGE
        intraBC_ctrls->hash_4x4_blocks = !disallow_4x4;
#else
        intraBC_ctrls->hash_4x4_blocks = allow_4x4_blocks;
#endif
        intraBC_ctrls->max_block_size_hash = scs->super_block_size;
        break;
    case 3:
        intraBC_ctrls->enabled       = 1;
        intraBC_ctrls->ibc_shift     = 1;
        intraBC_ctrls->ibc_direction = 0;
#if TUNE_STILL_IMAGE
        intraBC_ctrls->hash_4x4_blocks = !disallow_4x4;
#else
        intraBC_ctrls->hash_4x4_blocks = allow_4x4_blocks;
#endif
        intraBC_ctrls->max_block_size_hash = block_size_wide[BLOCK_16X16];
        break;
    case 4:
        intraBC_ctrls->enabled       = 1;
        intraBC_ctrls->ibc_shift     = 1;
        intraBC_ctrls->ibc_direction = 1;
#if TUNE_STILL_IMAGE
        intraBC_ctrls->hash_4x4_blocks = !disallow_4x4;
#else
        intraBC_ctrls->hash_4x4_blocks = allow_4x4_blocks;
#endif
        intraBC_ctrls->max_block_size_hash = block_size_wide[BLOCK_16X16];
        break;
    case 5:
        intraBC_ctrls->enabled       = 1;
        intraBC_ctrls->ibc_shift     = 1;
        intraBC_ctrls->ibc_direction = 1;
#if TUNE_STILL_IMAGE
        intraBC_ctrls->hash_4x4_blocks = !disallow_4x4;
#else
        intraBC_ctrls->hash_4x4_blocks = allow_4x4_blocks;
#endif
        intraBC_ctrls->max_block_size_hash = block_size_wide[BLOCK_8X8];
        break;
#else
    case 1:
        intraBC_ctrls->enabled             = pcs->sc_class1;
        intraBC_ctrls->ibc_shift           = 0;
        intraBC_ctrls->ibc_direction       = 0;
        intraBC_ctrls->hash_4x4_blocks     = allow_4x4_blocks;
        intraBC_ctrls->max_block_size_hash = scs->super_block_size;
        break;
    case 2:
        intraBC_ctrls->enabled             = pcs->sc_class1;
        intraBC_ctrls->ibc_shift           = 1;
        intraBC_ctrls->ibc_direction       = 0;
        intraBC_ctrls->hash_4x4_blocks     = allow_4x4_blocks;
        intraBC_ctrls->max_block_size_hash = scs->super_block_size;
        break;
    case 3:
        intraBC_ctrls->enabled             = pcs->sc_class1;
        intraBC_ctrls->ibc_shift           = 1;
        intraBC_ctrls->ibc_direction       = 1;
        intraBC_ctrls->hash_4x4_blocks     = allow_4x4_blocks;
        intraBC_ctrls->max_block_size_hash = scs->super_block_size;
        break;
    case 4:
        intraBC_ctrls->enabled             = pcs->sc_class1;
        intraBC_ctrls->ibc_shift           = 1;
        intraBC_ctrls->ibc_direction       = 0;
        intraBC_ctrls->hash_4x4_blocks     = allow_4x4_blocks;
        intraBC_ctrls->max_block_size_hash = block_size_wide[BLOCK_16X16];
        break;
    case 5:
        intraBC_ctrls->enabled             = pcs->sc_class1;
        intraBC_ctrls->ibc_shift           = 1;
        intraBC_ctrls->ibc_direction       = 0;
        intraBC_ctrls->hash_4x4_blocks     = allow_4x4_blocks;
        intraBC_ctrls->max_block_size_hash = block_size_wide[BLOCK_8X8];
        break;
    case 6:
        intraBC_ctrls->enabled             = pcs->sc_class1;
        intraBC_ctrls->ibc_shift           = 1;
        intraBC_ctrls->ibc_direction       = 1;
        intraBC_ctrls->hash_4x4_blocks     = allow_4x4_blocks;
        intraBC_ctrls->max_block_size_hash = block_size_wide[BLOCK_8X8];
        break;
#endif
    default:
        assert(0);
        break;
    }
}

/*
    set controls for Palette prediction
*/
static void set_palette_level(PictureParentControlSet* pcs, uint8_t palette_level) {
    PaletteCtrls* palette_ctrls = &pcs->palette_ctrls;

    switch (palette_level) {
    case 0:
        palette_ctrls->enabled = 0;
        break;
    case 1:
        palette_ctrls->enabled                       = 1;
        palette_ctrls->dominant_color_step           = 1;
        palette_ctrls->reduce_palette_cost_precision = 0;
        break;
    case 2:
        palette_ctrls->enabled                       = 1;
        palette_ctrls->dominant_color_step           = 2;
        palette_ctrls->reduce_palette_cost_precision = 0;
        break;
    case 3:
        palette_ctrls->enabled                       = 1;
        palette_ctrls->dominant_color_step           = 2;
        palette_ctrls->reduce_palette_cost_precision = 1;
        break;
    default:
        assert(0);
        break;
    }
}

/*
* return the max canidate count for MDS0
  Used by candidate injection and memory allocation
*/
uint16_t svt_aom_get_max_can_count(EncMode enc_mode) {
    //NOTE: this is a memory feature and not a speed feature. it should not be have any speed/quality impact.
    uint16_t mem_max_can_count;
    if (enc_mode <= ENC_M1) {
        mem_max_can_count = 1225;
    } else if (enc_mode <= ENC_M2) {
        mem_max_can_count = 1000;
    } else if (enc_mode <= ENC_M3) {
        mem_max_can_count = 720;
    } else if (enc_mode <= ENC_M4) {
        mem_max_can_count = 576;
    } else if (enc_mode <= ENC_M5) {
        mem_max_can_count = 369;
    } else if (enc_mode <= ENC_M6) {
        mem_max_can_count = 236;
    } else if (enc_mode <= ENC_M9) {
        mem_max_can_count = 190;
    } else {
        mem_max_can_count = 80;
    }
    return mem_max_can_count;
}

/******************************************************
* Derive Multi-Processes Settings for OQ
Input   : encoder mode and tune
Output  : Multi-Processes signal(s)
******************************************************/
#if TUNE_STILL_IMAGE
void svt_aom_sig_deriv_multi_processes_default(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    FrameHeader*            frm_hdr           = &pcs->frm_hdr;
    EncMode                 enc_mode          = pcs->enc_mode;
    const uint8_t           is_islice         = pcs->slice_type == I_SLICE;
    const uint8_t           is_base           = pcs->temporal_layer_index == 0;
    const EbInputResolution input_resolution  = pcs->input_resolution;
    const uint8_t           fast_decode       = scs->static_config.fast_decode;
    const uint8_t           sc_class1         = pcs->sc_class1;
    const uint8_t           is_not_last_layer = !pcs->is_highest_layer;

    // Set GM ctrls assuming super-res is off for gm-pp need
    svt_aom_set_gm_controls(pcs, svt_aom_derive_gm_level(pcs, true));

    // If enabled here, the hme enable flags should also be enabled in ResourceCoordinationProcess
    // to ensure that resources are allocated for the downsampled pictures used in HME
    pcs->enable_hme_flag        = 1;
    pcs->enable_hme_level0_flag = 1;
    if (sc_class1) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 1;
    } else if (enc_mode <= ENC_MR) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 1;
    } else {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 0;
    }

    switch (pcs->tf_ctrls.hme_me_level) {
    case 0:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 1;
        pcs->tf_enable_hme_level2_flag = 1;
        break;

    case 1:
    case 2:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 1;
        pcs->tf_enable_hme_level2_flag = 0;
        break;
    case 3:
    case 4:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 0;
        pcs->tf_enable_hme_level2_flag = 0;
        break;

    default:
        assert(0);
        break;
    }
    // Set the Multi-Pass PD level
    pcs->multi_pass_pd_level = MULTI_PASS_PD_ON;

    // Set intra-bc level
    uint8_t intrabc_level = 0;
    if (sc_class1) {
        if (is_islice) {
            if (enc_mode <= ENC_M6) {
                intrabc_level = 1;
            } else if (enc_mode <= ENC_M7) {
                intrabc_level = 5;
            } else {
                intrabc_level = 0;
            }
        } else {
            intrabc_level = 0;
        }
    }

    set_intrabc_level(pcs, scs, intrabc_level);
    frm_hdr->allow_intrabc = pcs->intraBC_ctrls.enabled;

    // Set palette level
    if (sc_class1) {
        if (enc_mode <= ENC_M2) {
            pcs->palette_level = is_base ? 2 : 0;
        } else if (enc_mode <= ENC_M9) {
            pcs->palette_level = is_islice ? 2 : 0;
        } else {
            pcs->palette_level = 0;
        }
    } else {
        pcs->palette_level = 0;
    }

    set_palette_level(pcs, pcs->palette_level);

    frm_hdr->allow_screen_content_tools = (sc_class1 && (pcs->palette_level || frm_hdr->allow_intrabc)) ? 1 : 0;

    // Set CDEF level
    uint8_t cdef_search_level = 0;
    if (!scs->seq_header.cdef_level || frm_hdr->allow_intrabc) {
        cdef_search_level = 0;
    } else if (scs->static_config.cdef_level != DEFAULT) {
        cdef_search_level = (int8_t)(scs->static_config.cdef_level);
    } else if (enc_mode <= ENC_MR) {
        cdef_search_level = 1;
    } else if (enc_mode <= ENC_M2) {
        cdef_search_level = 2;
    } else if (enc_mode <= ENC_M5) {
        cdef_search_level = 5;
    } else if (enc_mode <= ENC_M7) {
        cdef_search_level = is_base ? 5 : 6;
    } else {
        cdef_search_level = 7;
    }
    set_cdef_search_controls(pcs, cdef_search_level);
    pcs->cdef_level = cdef_search_level;

    uint8_t cdef_recon_level = 0;
    if (fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE) {
        if (enc_mode <= ENC_M8) {
            cdef_recon_level = 0;
        } else if (enc_mode <= ENC_M9) {
            cdef_recon_level = 1;
        } else {
            cdef_recon_level = 2;
        }
    } else if (fast_decode == 1) {
        cdef_recon_level = 1;
    } else { // fast_decode 2
        if (enc_mode <= ENC_M8) {
            cdef_recon_level = 2;
        } else {
            cdef_recon_level = 1;
        }
    }
    set_cdef_recon_controls(pcs, cdef_recon_level);

    uint8_t wn = 0, sg = 0;
    // If restoration filtering is enabled at the sequence level, derive the settings used for this frame
    if (scs->seq_header.enable_restoration) {
        // As allocation has already happened based on the initial input resolution/QP, the resolution/QP
        // changes should not impact enabling restoration. For some presets, restoration is off for 8K
        // and above and memory allocation is not performed. So, if we switch to smaller resolution, we need
        // to keep restoration off.
        EbInputResolution init_input_resolution;
        svt_aom_derive_input_resolution(&init_input_resolution,
                                        scs->max_initial_input_luma_width * scs->max_initial_input_luma_height);

        wn = svt_aom_get_wn_filter_level_default(enc_mode, init_input_resolution, is_not_last_layer);
        sg = svt_aom_get_sg_filter_level_default(enc_mode, init_input_resolution, fast_decode);
    }

    Av1Common* cm = pcs->av1_cm;
    svt_aom_set_wn_filter_ctrls(cm, wn);
    svt_aom_set_sg_filter_ctrls(cm, sg);

    // Set whether restoration filtering is enabled for this frame
    pcs->enable_restoration = (wn > 0 || sg > 0);

    // Set frame end cdf update mode      Settings
    // 0                                     OFF
    // 1                                     ON
    pcs->frame_end_cdf_update_mode = 1;

    if (scs->enable_hbd_mode_decision == DEFAULT) {
        if (enc_mode <= ENC_MR) {
            pcs->hbd_md = 1;
        } else if (enc_mode <= ENC_M5) {
            pcs->hbd_md = is_base ? 2 : 0;
        } else {
            pcs->hbd_md = is_islice ? 2 : 0;
        }
    } else {
        pcs->hbd_md = scs->enable_hbd_mode_decision;
    }

    pcs->max_can_count = svt_aom_get_max_can_count(enc_mode);
    if (enc_mode <= ENC_M1) {
        pcs->use_best_me_unipred_cand_only = 0;
    } else {
        pcs->use_best_me_unipred_cand_only = 1;
    }
}

void svt_aom_sig_deriv_multi_processes_rtc(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    FrameHeader*  frm_hdr           = &pcs->frm_hdr;
    EncMode       enc_mode          = pcs->enc_mode;
    const uint8_t is_islice         = pcs->slice_type == I_SLICE;
    const uint8_t is_base           = pcs->temporal_layer_index == 0;
    const uint8_t fast_decode       = scs->static_config.fast_decode;
    const uint8_t sc_class1         = pcs->sc_class1;
    const uint8_t is_not_last_layer = !pcs->is_highest_layer;
    const bool    flat_rtc          = scs->use_flat_ipp;
    // Set GM ctrls assuming super-res is off for gm-pp need
    svt_aom_set_gm_controls(pcs, svt_aom_derive_gm_level(pcs, true));

    // If enabled here, the hme enable flags should also be enabled in ResourceCoordinationProcess
    // to ensure that resources are allocated for the downsampled pictures used in HME
    pcs->enable_hme_flag        = 1;
    pcs->enable_hme_level0_flag = 1;
    if (sc_class1) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 1;
    } else if (enc_mode <= ENC_MR) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 1;
    } else if ((!flat_rtc && enc_mode <= ENC_M12) || (flat_rtc && enc_mode <= ENC_M11)) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 0;
    } else {
        pcs->enable_hme_level1_flag = 0;
        pcs->enable_hme_level2_flag = 0;
    }

    switch (pcs->tf_ctrls.hme_me_level) {
    case 0:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 1;
        pcs->tf_enable_hme_level2_flag = 1;
        break;

    case 1:
    case 2:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 1;
        pcs->tf_enable_hme_level2_flag = 0;
        break;
    case 3:
    case 4:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 0;
        pcs->tf_enable_hme_level2_flag = 0;
        break;

    default:
        assert(0);
        break;
    }
    // Set the Multi-Pass PD level
    pcs->multi_pass_pd_level = MULTI_PASS_PD_ON;

    // Set intra-bc level
    uint8_t intrabc_level = 0;
    set_intrabc_level(pcs, scs, intrabc_level);
    frm_hdr->allow_intrabc = pcs->intraBC_ctrls.enabled;

    // Set palette level
    if (sc_class1) {
        pcs->palette_level = is_islice ? 3 : 0;
    } else {
        pcs->palette_level = 0;
    }

    set_palette_level(pcs, pcs->palette_level);

    frm_hdr->allow_screen_content_tools = (sc_class1 && (pcs->palette_level || frm_hdr->allow_intrabc)) ? 1 : 0;

    // Set CDEF level
    uint8_t cdef_search_level = 0;
    if (!scs->seq_header.cdef_level || frm_hdr->allow_intrabc) {
        cdef_search_level = 0;
    } else if (scs->static_config.cdef_level != DEFAULT) {
        cdef_search_level = (int8_t)(scs->static_config.cdef_level);
    } else {
        if (sc_class1) {
            if (enc_mode <= ENC_M7) {
                cdef_search_level = 5;
            } else if (enc_mode <= ENC_M9) {
                cdef_search_level = is_base ? 5 : 6;
            } else if (enc_mode <= ENC_M10) {
                cdef_search_level = is_islice ? 5 : 6;
            } else {
                cdef_search_level = is_islice ? 5 : 7;
            }
        } else {
            if (enc_mode <= ENC_M8) {
                cdef_search_level = is_base ? 5 : 6;
            } else if (flat_rtc && enc_mode <= ENC_M11) {
                if (pcs->input_resolution <= INPUT_SIZE_360p_RANGE) {
                    cdef_search_level = 6;
                } else {
                    cdef_search_level = is_islice ? 5 : 8;
                }
            } else if (!flat_rtc && enc_mode <= ENC_M9) {
                if (pcs->input_resolution <= INPUT_SIZE_360p_RANGE) {
                    cdef_search_level = 7;
                } else {
                    cdef_search_level = is_islice ? 5 : 8;
                }
            } else {
                cdef_search_level = is_islice ? 5 : 8;
            }
        }
    }

    set_cdef_search_controls(pcs, cdef_search_level);
    pcs->cdef_level = cdef_search_level;

    uint8_t cdef_recon_level = 0;
    set_cdef_recon_controls(pcs, cdef_recon_level);

    uint8_t wn = 0, sg = 0;
    // If restoration filtering is enabled at the sequence level, derive the settings used for this frame
    if (scs->seq_header.enable_restoration) {
        // As allocation has already happened based on the initial input resolution/QP, the resolution/QP
        // changes should not impact enabling restoration. For some presets, restoration is off for 8K
        // and above and memory allocation is not performed. So, if we switch to smaller resolution, we need
        // to keep restoration off.
        EbInputResolution init_input_resolution;
        svt_aom_derive_input_resolution(&init_input_resolution,
                                        scs->max_initial_input_luma_width * scs->max_initial_input_luma_height);

        wn = svt_aom_get_wn_filter_level_rtc(enc_mode, init_input_resolution, is_not_last_layer);
        sg = svt_aom_get_sg_filter_level_rtc(enc_mode, init_input_resolution, fast_decode);
    }

    Av1Common* cm = pcs->av1_cm;
    svt_aom_set_wn_filter_ctrls(cm, wn);
    svt_aom_set_sg_filter_ctrls(cm, sg);

    // Set whether restoration filtering is enabled for this frame
    pcs->enable_restoration = (wn > 0 || sg > 0);

    // Set frame end cdf update mode      Settings
    // 0                                     OFF
    // 1                                     ON
    pcs->frame_end_cdf_update_mode = 1;

    if (scs->enable_hbd_mode_decision == DEFAULT) {
        if (enc_mode <= ENC_MR) {
            pcs->hbd_md = 1;
        } else if (enc_mode <= ENC_M5) {
            pcs->hbd_md = is_base ? 2 : 0;
        } else {
            pcs->hbd_md = is_islice ? 2 : 0;
        }
    } else {
        pcs->hbd_md = scs->enable_hbd_mode_decision;
    }

    pcs->max_can_count = svt_aom_get_max_can_count(enc_mode);
    if (enc_mode <= ENC_M1) {
        pcs->use_best_me_unipred_cand_only = 0;
    } else {
        pcs->use_best_me_unipred_cand_only = 1;
    }
}

void svt_aom_sig_deriv_multi_processes_allintra(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    FrameHeader*            frm_hdr          = &pcs->frm_hdr;
    EncMode                 enc_mode         = pcs->enc_mode;
    const EbInputResolution input_resolution = pcs->input_resolution;
    const uint8_t           fast_decode      = scs->static_config.fast_decode;

    // Set the Multi-Pass PD level
    pcs->multi_pass_pd_level = MULTI_PASS_PD_ON;

    // Set intra-bc level
    uint8_t intrabc_level = 0;
    set_intrabc_level(pcs, scs, intrabc_level);
    frm_hdr->allow_intrabc = pcs->intraBC_ctrls.enabled;

    // Set palette level
    pcs->palette_level = 0;
    set_palette_level(pcs, pcs->palette_level);

    frm_hdr->allow_screen_content_tools = (pcs->palette_level || frm_hdr->allow_intrabc) ? 1 : 0;

    // Set CDEF level
    uint8_t cdef_search_level = 0;
    if (!scs->seq_header.cdef_level || frm_hdr->allow_intrabc) {
        cdef_search_level = 0;
    } else if (scs->static_config.cdef_level != DEFAULT) {
        cdef_search_level = (int8_t)(scs->static_config.cdef_level);
    } else {
#if OPT_Q_CDEF
        if ((fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE)) {
            if (enc_mode <= ENC_M3) {
                cdef_search_level = 3;
            } else if (enc_mode <= ENC_M5) {
                cdef_search_level = 5;
            } else if (enc_mode <= ENC_M6) {
                cdef_search_level = 7;
            } else {
                // For fd1/fd2, disable CDEF search if fd0 uses level 9 or 0
                cdef_search_level = 9;
            }
        } else {
#endif
            if (enc_mode <= ENC_M3) {
                cdef_search_level = 3;
            } else if (enc_mode <= ENC_M5) {
                cdef_search_level = 5;
#if OPT_Q_CDEF
            } else if (enc_mode <= ENC_M7) {
#else
        } else if (enc_mode <= ENC_M8) {
#endif
                cdef_search_level = 7;
            } else {
                cdef_search_level = 0;
            }
#if OPT_Q_CDEF
        }
#endif
    }
    set_cdef_search_controls(pcs, cdef_search_level);
    pcs->cdef_level = cdef_search_level;

    uint8_t cdef_recon_level = 0;
    if ((fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE)) {
        if (enc_mode <= ENC_M7) {
            cdef_recon_level = 0;
        } else {
            cdef_recon_level = 1;
        }
    } else if (fast_decode == 1) {
#if OPT_FD2_ALLINTRA
        if (enc_mode <= ENC_M7) {
#else
        if ((enc_mode <= ENC_M7) || (enc_mode == ENC_M9)) {
#endif
            cdef_recon_level = 1;
        } else {
            cdef_recon_level = 3;
        }
    } else {
#if OPT_FD2_ALLINTRA
        cdef_recon_level = 3;
#else
        if (enc_mode <= ENC_M7) {
            cdef_recon_level = 4;
        } else if (enc_mode <= ENC_M8) {
            cdef_recon_level = 3;
        } else {
            cdef_recon_level = 1;
        }
#endif
    }
    set_cdef_recon_controls(pcs, cdef_recon_level);

    uint8_t wn = 0, sg = 0;
    // If restoration filtering is enabled at the sequence level, derive the settings used for this frame
    if (scs->seq_header.enable_restoration) {
        // As allocation has already happened based on the initial input resolution/QP, the resolution/QP
        // changes should not impact enabling restoration. For some presets, restoration is off for 8K
        // and above and memory allocation is not performed. So, if we switch to smaller resolution, we need
        // to keep restoration off.
        EbInputResolution init_input_resolution;
        svt_aom_derive_input_resolution(&init_input_resolution,
                                        scs->max_initial_input_luma_width * scs->max_initial_input_luma_height);

        wn = svt_aom_get_wn_filter_level_allintra(enc_mode);
        sg = svt_aom_get_sg_filter_level_allintra();
    }

    Av1Common* cm = pcs->av1_cm;
    svt_aom_set_wn_filter_ctrls(cm, wn);
    svt_aom_set_sg_filter_ctrls(cm, sg);

    // Set whether restoration filtering is enabled for this frame
    pcs->enable_restoration = (wn > 0 || sg > 0);

    // Set frame end cdf update mode      Settings
    // 0                                     OFF
    // 1                                     ON
    pcs->frame_end_cdf_update_mode = 1;
    pcs->max_can_count             = svt_aom_get_max_can_count(enc_mode);
}
#else
void svt_aom_sig_deriv_multi_processes(SequenceControlSet* scs, PictureParentControlSet* pcs) {
    FrameHeader*            frm_hdr           = &pcs->frm_hdr;
    EncMode                 enc_mode          = pcs->enc_mode;
    const uint8_t           is_islice         = pcs->slice_type == I_SLICE;
    const uint8_t           is_base           = pcs->temporal_layer_index == 0;
    const EbInputResolution input_resolution  = pcs->input_resolution;
    const uint8_t           fast_decode       = scs->static_config.fast_decode;
    const bool              rtc_tune          = scs->static_config.rtc;
    const uint8_t           sc_class1         = pcs->sc_class1;
    const uint8_t           is_not_last_layer = !pcs->is_highest_layer;
    const bool              allintra          = scs->allintra;
    const bool              flat_rtc          = rtc_tune && scs->use_flat_ipp;
    // Set GM ctrls assuming super-res is off for gm-pp need
    svt_aom_set_gm_controls(pcs, svt_aom_derive_gm_level(pcs, true));

    // If enabled here, the hme enable flags should also be enabled in ResourceCoordinationProcess
    // to ensure that resources are allocated for the downsampled pictures used in HME
    pcs->enable_hme_flag        = 1;
    pcs->enable_hme_level0_flag = 1;
    if (sc_class1) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 1;
    } else if (enc_mode <= ENC_MR) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 1;
    } else if ((!flat_rtc && enc_mode <= ENC_M12) || (flat_rtc && enc_mode <= ENC_M11)) {
        pcs->enable_hme_level1_flag = 1;
        pcs->enable_hme_level2_flag = 0;
    } else {
        pcs->enable_hme_level1_flag = 0;
        pcs->enable_hme_level2_flag = 0;
    }

    switch (pcs->tf_ctrls.hme_me_level) {
    case 0:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 1;
        pcs->tf_enable_hme_level2_flag = 1;
        break;

    case 1:
    case 2:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 1;
        pcs->tf_enable_hme_level2_flag = 0;
        break;
    case 3:
    case 4:
        pcs->tf_enable_hme_flag        = 1;
        pcs->tf_enable_hme_level0_flag = 1;
        pcs->tf_enable_hme_level1_flag = 0;
        pcs->tf_enable_hme_level2_flag = 0;
        break;

    default:
        assert(0);
        break;
    }
    // Set the Multi-Pass PD level
    pcs->multi_pass_pd_level = MULTI_PASS_PD_ON;
    // Loop filter Level                            Settings
    // 0                                            OFF
    // 1                                            CU-BASED
    // 2                                            LIGHT FRAME-BASED
    // 3                                            FULL FRAME-BASED

    //for now only I frames are allowed to use sc tools.
    //TODO: we can force all frames in GOP with the same detection status of leading I frame.
    uint8_t intrabc_level = 0;
    if (is_islice) { // DNT--Applicable for SC only
        if (rtc_tune) {
            intrabc_level = 0;
        } else {
            if (enc_mode <= ENC_M6) {
                intrabc_level = 1;
            } else if (enc_mode <= ENC_M7) {
                intrabc_level = 6;
            } else {
                intrabc_level = 0;
            }
        }
    } else {
        //this will enable sc tools for P frames. hence change Bitstream even if palette mode is OFF
        intrabc_level = 0;
    }
    set_intrabc_level(pcs, scs, intrabc_level);
    frm_hdr->allow_intrabc = pcs->intraBC_ctrls.enabled;
    // Set palette_level
    if (sc_class1) {
        if (rtc_tune) {
            pcs->palette_level = is_islice ? 3 : 0;
        } else if (enc_mode <= ENC_M2) {
            pcs->palette_level = is_base ? 2 : 0;
        } else if (enc_mode <= ENC_M9) {
            pcs->palette_level = is_islice ? 2 : 0;
        } else {
            pcs->palette_level = 0;
        }
    } else {
        pcs->palette_level = 0;
    }

    set_palette_level(pcs, pcs->palette_level);

    frm_hdr->allow_screen_content_tools = sc_class1 && pcs->palette_level > 0 ? 1 : 0;

    // Set CDEF controls
    uint8_t cdef_search_level = 0;
    if (!scs->seq_header.cdef_level || frm_hdr->allow_intrabc) {
        cdef_search_level = 0;
    } else if (scs->static_config.cdef_level != DEFAULT) {
        cdef_search_level = (int8_t)(scs->static_config.cdef_level);
    } else if (allintra) {
        if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_MR) {
                cdef_search_level = 1;
            }
            if (enc_mode <= ENC_M2) {
                cdef_search_level = 3;
            } else if (enc_mode <= ENC_M3) {
                cdef_search_level = 4;
            } else if (enc_mode <= ENC_M7) {
                cdef_search_level = 5;
            } else if (enc_mode <= ENC_M11) {
                cdef_search_level = 7;
            } else {
                cdef_search_level = 0;
            }
        } else {
            if (enc_mode <= ENC_MR) {
                cdef_search_level = 1;
            } else if (enc_mode <= ENC_M5) {
                cdef_search_level = 4;
            } else if (enc_mode <= ENC_M7) {
                cdef_search_level = 5;
            } else if (enc_mode <= ENC_M8) {
                cdef_search_level = 6;
            } else if (enc_mode <= ENC_M11) {
                cdef_search_level = 7;
            } else {
                cdef_search_level = 0;
            }
        }
    } else if (rtc_tune) {
        if (sc_class1) {
            if (enc_mode <= ENC_M7) {
                cdef_search_level = 5;
            } else if (enc_mode <= ENC_M9) {
                cdef_search_level = is_base ? 5 : 6;
            } else if (enc_mode <= ENC_M10) {
                cdef_search_level = is_islice ? 5 : 6;
            } else {
                cdef_search_level = is_islice ? 5 : 7;
            }
        } else {
            if (enc_mode <= ENC_M8) {
                cdef_search_level = is_base ? 5 : 6;
            } else if (scs->use_flat_ipp && enc_mode <= ENC_M11) {
                if (pcs->input_resolution <= INPUT_SIZE_360p_RANGE) {
                    cdef_search_level = 6;
                } else {
                    cdef_search_level = is_islice ? 5 : 8;
                }
            } else if (!scs->use_flat_ipp && enc_mode <= ENC_M9) {
                if (pcs->input_resolution <= INPUT_SIZE_360p_RANGE) {
                    cdef_search_level = 7;
                } else {
                    cdef_search_level = is_islice ? 5 : 8;
                }
            } else {
                cdef_search_level = is_islice ? 5 : 8;
            }
        }
    } else if (enc_mode <= ENC_MR) {
        cdef_search_level = 1;
    } else if (enc_mode <= ENC_M2) {
        cdef_search_level = 2;
    } else if (enc_mode <= ENC_M5) {
        cdef_search_level = 5;
    } else if (enc_mode <= ENC_M7) {
        cdef_search_level = is_base ? 5 : 6;
    } else {
        cdef_search_level = 7;
    }
    set_cdef_search_controls(pcs, cdef_search_level);
    pcs->cdef_level = cdef_search_level;

    uint8_t cdef_recon_level = 0;
    if (allintra) {
        if ((fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE)) {
            if (enc_mode <= ENC_M9) {
                cdef_recon_level = 0;
            } else {
                cdef_recon_level = 1;
            }
        } else if (fast_decode == 1) {
            if ((enc_mode <= ENC_M9) || (enc_mode == ENC_M12)) {
                cdef_recon_level = 1;
            } else {
                cdef_recon_level = 3;
            }
        } else {
            if (enc_mode <= ENC_M9) {
                cdef_recon_level = 4;
            } else if (enc_mode <= ENC_M11) {
                cdef_recon_level = 3;
            } else {
                cdef_recon_level = 1;
            }
        }
    } else if (rtc_tune) {
        cdef_recon_level = 0;
    } else if (fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE) {
        if (enc_mode <= ENC_M8) {
            cdef_recon_level = 0;
        } else if (enc_mode <= ENC_M9) {
            cdef_recon_level = 1;
        } else {
            cdef_recon_level = 2;
        }
    } else if (fast_decode == 1) {
        cdef_recon_level = 1;
    } else { // fast_decode 2
        if (enc_mode <= ENC_M8) {
            cdef_recon_level = 2;
        } else {
            cdef_recon_level = 1;
        }
    }
    set_cdef_recon_controls(pcs, cdef_recon_level);

    uint8_t wn = 0, sg = 0;
    // If restoration filtering is enabled at the sequence level, derive the settings used for this frame
    if (scs->seq_header.enable_restoration) {
        // As allocation has already happened based on the initial input resolution/QP, the resolution/QP
        // changes should not impact enabling restoration. For some presets, restoration is off for 8K
        // and above and memory allocation is not performed. So, if we switch to smaller resolution, we need
        // to keep restoration off.
        EbInputResolution init_input_resolution;
        svt_aom_derive_input_resolution(&init_input_resolution,
                                        scs->max_initial_input_luma_width * scs->max_initial_input_luma_height);

        wn = svt_aom_get_wn_filter_level(enc_mode, init_input_resolution, is_not_last_layer, allintra, rtc_tune);
        sg = svt_aom_get_sg_filter_level(enc_mode, init_input_resolution, fast_decode, allintra);
    }

    Av1Common* cm = pcs->av1_cm;
    svt_aom_set_wn_filter_ctrls(cm, wn);
    svt_aom_set_sg_filter_ctrls(cm, sg);

    // Set whether restoration filtering is enabled for this frame
    pcs->enable_restoration = (wn > 0 || sg > 0);

    // Set frame end cdf update mode      Settings
    // 0                                     OFF
    // 1                                     ON
    pcs->frame_end_cdf_update_mode = 1;

    if (scs->enable_hbd_mode_decision == DEFAULT) {
        if (enc_mode <= ENC_MR) {
            pcs->hbd_md = 1;
        } else if (enc_mode <= ENC_M5) {
            pcs->hbd_md = is_base ? 2 : 0;
        } else {
            pcs->hbd_md = is_islice ? 2 : 0;
        }
    } else {
        pcs->hbd_md = scs->enable_hbd_mode_decision;
    }

    pcs->max_can_count = svt_aom_get_max_can_count(enc_mode);
    if (enc_mode <= ENC_M1) {
        pcs->use_best_me_unipred_cand_only = 0;
    } else {
        pcs->use_best_me_unipred_cand_only = 1;
    }
}
#endif

/******************************************************
* GM controls
******************************************************/
void svt_aom_set_gm_controls(PictureParentControlSet* pcs, uint8_t gm_level) {
    GmControls* gm_ctrls = &pcs->gm_ctrls;
    switch (gm_level) {
    case 0:
        gm_ctrls->enabled    = 0;
        gm_ctrls->pp_enabled = 0;
        break;

    case 1:
        gm_ctrls->enabled                 = 1;
        gm_ctrls->identiy_exit            = 0;
        gm_ctrls->search_start_model      = TRANSLATION;
        gm_ctrls->search_end_model        = AFFINE;
        gm_ctrls->skip_identity           = 0;
        gm_ctrls->bypass_based_on_me      = 0;
        gm_ctrls->params_refinement_steps = 5;
        gm_ctrls->downsample_level        = GM_FULL;

        gm_ctrls->corners               = 4;
        gm_ctrls->chess_rfn             = 0;
        gm_ctrls->match_sz              = 13;
        gm_ctrls->inj_psq_glb           = false;
        gm_ctrls->pp_enabled            = 0;
        gm_ctrls->ref_idx0_only         = 0;
        gm_ctrls->rfn_early_exit        = 0;
        gm_ctrls->correspondence_method = CORNERS;
        break;
    case 2:
        gm_ctrls->enabled                 = 1;
        gm_ctrls->identiy_exit            = 1;
        gm_ctrls->search_start_model      = TRANSLATION;
        gm_ctrls->search_end_model        = ROTZOOM;
        gm_ctrls->skip_identity           = 0;
        gm_ctrls->bypass_based_on_me      = 0;
        gm_ctrls->params_refinement_steps = 5;
        gm_ctrls->downsample_level        = GM_FULL;
        gm_ctrls->corners                 = 2;
        gm_ctrls->chess_rfn               = 0;
        gm_ctrls->match_sz                = 7;
        gm_ctrls->inj_psq_glb             = false;
        gm_ctrls->pp_enabled              = 1;
        gm_ctrls->ref_idx0_only           = 0;
        gm_ctrls->rfn_early_exit          = 0;
        gm_ctrls->correspondence_method   = CORNERS;
        break;
    case 3:
        gm_ctrls->enabled                 = 1;
        gm_ctrls->identiy_exit            = 1;
        gm_ctrls->search_start_model      = TRANSLATION;
        gm_ctrls->search_end_model        = ROTZOOM;
        gm_ctrls->skip_identity           = 0;
        gm_ctrls->bypass_based_on_me      = 1;
        gm_ctrls->params_refinement_steps = 5;
        gm_ctrls->downsample_level        = GM_FULL;
        gm_ctrls->corners                 = 2;
        gm_ctrls->chess_rfn               = 1;
        gm_ctrls->match_sz                = 7;
        gm_ctrls->inj_psq_glb             = true;
        gm_ctrls->pp_enabled              = 0;
        gm_ctrls->ref_idx0_only           = 1;
        gm_ctrls->rfn_early_exit          = 1;
        gm_ctrls->correspondence_method   = pcs->input_resolution <= INPUT_SIZE_480p_RANGE ? MV_8x8
              : pcs->input_resolution <= INPUT_SIZE_1080p_RANGE                            ? MV_16x16
                                                                                           : MV_32x32;
        break;
    case 4:
        gm_ctrls->enabled                 = 1;
        gm_ctrls->identiy_exit            = 1;
        gm_ctrls->search_start_model      = TRANSLATION;
        gm_ctrls->search_end_model        = ROTZOOM;
        gm_ctrls->skip_identity           = 1;
        gm_ctrls->bypass_based_on_me      = 1;
        gm_ctrls->params_refinement_steps = 5;
        gm_ctrls->downsample_level        = GM_FULL;
        gm_ctrls->corners                 = 2;
        gm_ctrls->chess_rfn               = 1;
        gm_ctrls->match_sz                = 7;
        gm_ctrls->inj_psq_glb             = true;
        gm_ctrls->pp_enabled              = 0;
        gm_ctrls->ref_idx0_only           = 1;
        gm_ctrls->rfn_early_exit          = 1;
        gm_ctrls->correspondence_method   = pcs->input_resolution <= INPUT_SIZE_480p_RANGE ? MV_8x8
              : pcs->input_resolution <= INPUT_SIZE_1080p_RANGE                            ? MV_16x16
                                                                                           : MV_32x32;
        break;
    default:
        assert(0);
        break;
    }
    if (gm_ctrls->correspondence_method < CORNERS) {
        // MV-based correspondence methods rely on ME info, which is unavailable in pre-processor stage
        gm_ctrls->pp_enabled = 0;
    }
    if (gm_level) {
        assert((gm_ctrls->match_sz & 1) == 1);
    }
}

static void set_inter_comp_controls(ModeDecisionContext* ctx, uint8_t inter_comp_mode) {
    InterCompCtrls* inter_comp_ctrls = &ctx->inter_comp_ctrls;

    switch (inter_comp_mode) {
    case 0: //OFF (AVG only)
        inter_comp_ctrls->tot_comp_types      = MD_COMP_DIST;
        inter_comp_ctrls->do_nearest_nearest  = 0;
        inter_comp_ctrls->do_near_near        = 0;
        inter_comp_ctrls->do_me               = 0;
        inter_comp_ctrls->do_pme              = 0;
        inter_comp_ctrls->do_nearest_near_new = 0;
        inter_comp_ctrls->do_3x3_bi           = 0;
        inter_comp_ctrls->do_global           = 0;
        break;
    case 1: //FULL
        inter_comp_ctrls->tot_comp_types      = MD_COMP_TYPES;
        inter_comp_ctrls->do_nearest_nearest  = 1;
        inter_comp_ctrls->do_near_near        = 1;
        inter_comp_ctrls->do_me               = 1;
        inter_comp_ctrls->do_pme              = 1;
        inter_comp_ctrls->do_nearest_near_new = 1;
        inter_comp_ctrls->do_3x3_bi           = 1;
        inter_comp_ctrls->do_global           = 1;

        inter_comp_ctrls->skip_on_ref_info    = 0;
        inter_comp_ctrls->use_rate            = 1;
        inter_comp_ctrls->pred0_to_pred1_mult = 0;
        inter_comp_ctrls->max_mv_length       = 0;
        inter_comp_ctrls->no_sym_dist         = 0;
        break;
    case 2:
        inter_comp_ctrls->tot_comp_types      = MD_COMP_TYPES;
        inter_comp_ctrls->do_nearest_nearest  = 1;
        inter_comp_ctrls->do_near_near        = 1;
        inter_comp_ctrls->do_me               = 1;
        inter_comp_ctrls->do_pme              = 1;
        inter_comp_ctrls->do_nearest_near_new = 0;
        inter_comp_ctrls->do_3x3_bi           = 0;
        inter_comp_ctrls->do_global           = 1;

        inter_comp_ctrls->skip_on_ref_info    = 0;
        inter_comp_ctrls->use_rate            = 0;
        inter_comp_ctrls->pred0_to_pred1_mult = 1;
        inter_comp_ctrls->max_mv_length       = 0;
        inter_comp_ctrls->no_sym_dist         = 0;
        break;
    case 3:
        inter_comp_ctrls->tot_comp_types      = MD_COMP_TYPES;
        inter_comp_ctrls->do_nearest_nearest  = 1;
        inter_comp_ctrls->do_near_near        = 1;
        inter_comp_ctrls->do_me               = 1;
        inter_comp_ctrls->do_pme              = 0;
        inter_comp_ctrls->do_nearest_near_new = 0;
        inter_comp_ctrls->do_3x3_bi           = 0;
        inter_comp_ctrls->do_global           = 1;

        inter_comp_ctrls->skip_on_ref_info    = 1;
        inter_comp_ctrls->use_rate            = 0;
        inter_comp_ctrls->pred0_to_pred1_mult = 1;
        inter_comp_ctrls->max_mv_length       = 0;
        inter_comp_ctrls->no_sym_dist         = 1;
        break;
    case 4:
        inter_comp_ctrls->tot_comp_types      = MD_COMP_TYPES;
        inter_comp_ctrls->do_nearest_nearest  = 1;
        inter_comp_ctrls->do_near_near        = 1;
        inter_comp_ctrls->do_me               = 1;
        inter_comp_ctrls->do_pme              = 0;
        inter_comp_ctrls->do_nearest_near_new = 0;
        inter_comp_ctrls->do_3x3_bi           = 0;
        inter_comp_ctrls->do_global           = 1;

        inter_comp_ctrls->skip_on_ref_info    = 1;
        inter_comp_ctrls->use_rate            = 0;
        inter_comp_ctrls->pred0_to_pred1_mult = 4;
        inter_comp_ctrls->max_mv_length       = 32;
        inter_comp_ctrls->no_sym_dist         = 1;
        break;
    default:
        assert(0);
        break;
    }
}
#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_enable_sg_default(EncMode enc_mode, uint8_t input_resolution, uint8_t fast_decode) {
    uint8_t sg = 0;
    sg         = svt_aom_get_sg_filter_level_default(enc_mode, input_resolution, fast_decode);
    return (sg > 0);
}

uint8_t svt_aom_get_enable_sg_rtc(EncMode enc_mode, uint8_t input_resolution, uint8_t fast_decode) {
    uint8_t sg = 0;
    sg         = svt_aom_get_sg_filter_level_rtc(enc_mode, input_resolution, fast_decode);
    return (sg > 0);
}

uint8_t svt_aom_get_enable_sg_allintra() {
    uint8_t sg = 0;
    sg         = svt_aom_get_sg_filter_level_allintra();
    return (sg > 0);
}
#else
uint8_t svt_aom_get_enable_sg(EncMode enc_mode, uint8_t input_resolution, uint8_t fast_decode, bool allintra) {
    uint8_t sg = 0;
    sg         = svt_aom_get_sg_filter_level(enc_mode, input_resolution, fast_decode, allintra);
    return (sg > 0);
}
#endif
/*
* return true if restoration filtering is enabled; false otherwise
  Used by signal_derivation_pre_analysis_oq and memory allocation
*/
#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_enable_restoration_default(EncMode enc_mode, int8_t config_enable_restoration,
                                               uint8_t input_resolution, uint8_t fast_decode) {
    if (config_enable_restoration != DEFAULT) {
        return config_enable_restoration;
    }

    uint8_t wn = 0;
    for (int is_ref = 0; is_ref < 2; is_ref++) {
        wn = svt_aom_get_wn_filter_level_default(enc_mode, input_resolution, is_ref);
        if (wn) {
            break;
        }
    }
    uint8_t sg = svt_aom_get_enable_sg_default(enc_mode, input_resolution, fast_decode);
    return (sg > 0 || wn > 0);
}

uint8_t svt_aom_get_enable_restoration_rtc(EncMode enc_mode, int8_t config_enable_restoration, uint8_t input_resolution,
                                           uint8_t fast_decode) {
    if (config_enable_restoration != DEFAULT) {
        return config_enable_restoration;
    }

    uint8_t wn = 0;
    for (int is_ref = 0; is_ref < 2; is_ref++) {
        wn = svt_aom_get_wn_filter_level_rtc(enc_mode, input_resolution, is_ref);
        if (wn) {
            break;
        }
    }
    uint8_t sg = svt_aom_get_enable_sg_rtc(enc_mode, input_resolution, fast_decode);
    return (sg > 0 || wn > 0);
}

uint8_t svt_aom_get_enable_restoration_allintra(EncMode enc_mode, int8_t config_enable_restoration) {
    if (config_enable_restoration != DEFAULT) {
        return config_enable_restoration;
    }

    uint8_t wn = 0;
    for (int is_ref = 0; is_ref < 2; is_ref++) {
        wn = svt_aom_get_wn_filter_level_allintra(enc_mode);
        if (wn) {
            break;
        }
    }
    uint8_t sg = svt_aom_get_enable_sg_allintra();
    return (sg > 0 || wn > 0);
}
#else
uint8_t svt_aom_get_enable_restoration(EncMode enc_mode, int8_t config_enable_restoration, uint8_t input_resolution,
                                       uint8_t fast_decode, bool allintra, bool rtc_tune) {
    if (config_enable_restoration != DEFAULT) {
        return config_enable_restoration;
    }

    uint8_t wn = 0;
    for (int is_ref = 0; is_ref < 2; is_ref++) {
        wn = svt_aom_get_wn_filter_level(enc_mode, input_resolution, is_ref, allintra, rtc_tune);
        if (wn) {
            break;
        }
    }
    uint8_t sg = svt_aom_get_enable_sg(enc_mode, input_resolution, fast_decode, allintra);
    return (sg > 0 || wn > 0);
}
#endif

/******************************************************
* Derive Pre-Analysis settings for OQ for pcs
Input   : encoder mode and tune
Output  : Pre-Analysis signal(s)
******************************************************/
void svt_aom_sig_deriv_pre_analysis_pcs(PictureParentControlSet* pcs) {
    // Derive HME Flag
    // Set here to allocate resources for the downsampled pictures used in HME (generated in PictureAnalysis)
    // Will be later updated for SC/NSC in PictureDecisionProcess
    pcs->enable_hme_flag        = 1;
    pcs->enable_hme_level0_flag = 1;
    pcs->enable_hme_level1_flag = 1;
    pcs->enable_hme_level2_flag = 1;
    // Set here to allocate resources for the downsampled pictures used in HME (generated in PictureAnalysis)
    // Will be later updated for SC/NSC in PictureDecisionProcess
    pcs->tf_enable_hme_flag        = 1;
    pcs->tf_enable_hme_level0_flag = 1;
    pcs->tf_enable_hme_level1_flag = 1;
    pcs->tf_enable_hme_level2_flag = 1;
}

/******************************************************
* Derive Pre-Analysis settings for OQ for scs
Input   : encoder mode and tune
Output  : Pre-Analysis signal(s)
******************************************************/
void svt_aom_sig_deriv_pre_analysis_scs(SequenceControlSet* scs) {
    const int8_t enc_mode = scs->static_config.enc_mode;
    const bool   rtc_tune = scs->static_config.rtc;
    const bool   allintra = scs->allintra;
#if !TUNE_STILL_IMAGE
    const EbInputResolution input_resolution = scs->input_resolution;
#endif
    // initialize sequence level enable_superres
    scs->seq_header.enable_superres = scs->static_config.superres_mode > SUPERRES_NONE ? 1 : 0;
    uint8_t ii_allowed              = 0;
    for (uint8_t transition_present = 0; transition_present < 2; transition_present++) {
        if (ii_allowed) {
            break;
        }
        ii_allowed |= svt_aom_get_inter_intra_level(enc_mode, transition_present);
    }
    scs->seq_header.enable_interintra_compound = ii_allowed ? 1 : 0;

#if TUNE_STILL_IMAGE
    uint8_t is_filter_intra_used = allintra ? get_filter_intra_level_allintra(enc_mode)
        : rtc_tune                          ? get_filter_intra_level_rtc(enc_mode)
                                            : get_filter_intra_level_default(enc_mode);

    scs->seq_header.filter_intra_level = is_filter_intra_used ? 1 : 0;

#else
    if (get_filter_intra_level(scs, enc_mode)) {
        scs->seq_header.filter_intra_level = 1;
    } else {
        scs->seq_header.filter_intra_level = 0;
    }
#endif

    if (get_inter_compound_level(enc_mode)) {
        scs->seq_header.order_hint_info.enable_jnt_comp = 1; //DISTANCE
        scs->seq_header.enable_masked_compound          = 1; //DIFF+WEDGE
    } else {
        scs->seq_header.order_hint_info.enable_jnt_comp = 0;
        scs->seq_header.enable_masked_compound          = 0;
    }
    // For non-still-image or non-all-intra configurations, keep edge filter always ON, otherwise OFF unless angular refinement pruning techniques are active
    if (allintra) {
        // Flag indicating whether angular refinement pruning is active
#if TUNE_STILL_IMAGE
        uint32_t intra_level                = 0;
        uint32_t dist_based_ang_intra_level = 0;

        svt_aom_get_intra_mode_levels_allintra(enc_mode, &intra_level, &dist_based_ang_intra_level);

        scs->seq_header.enable_intra_edge_filter = dist_based_ang_intra_level >= 1 ||
                angular_pred_level[intra_level] == 2 || angular_pred_level[intra_level] == 3
            ? 1
            : 0;
#else
        bool is_ang_refinement_pruned = 0;

        // Check if angular refinement pruning is used in any case
        for (uint8_t is_islice = 0; is_islice <= 1; is_islice++) {
            for (uint8_t is_base = 0; is_base <= 1; is_base++) {
                for (uint8_t sc_class1 = 0; sc_class1 <= 1; sc_class1++) {
                    for (uint8_t transition_present = 0; transition_present <= 1; transition_present++) {
                        uint32_t intra_level                = 0;
                        uint32_t dist_based_ang_intra_level = 0;

                        svt_aom_get_intra_mode_levels(enc_mode,
                                                      input_resolution,
                                                      allintra,
                                                      rtc_tune,
                                                      is_islice,
                                                      is_base,
                                                      sc_class1,
                                                      transition_present,
                                                      scs->use_flat_ipp,
                                                      &intra_level,
                                                      &dist_based_ang_intra_level);

                        is_ang_refinement_pruned = dist_based_ang_intra_level >= 1 ||
                                angular_pred_level[intra_level] == 2 || angular_pred_level[intra_level] == 3
                            ? 1
                            : 0;

                        if (is_ang_refinement_pruned) {
                            break;
                        }
                    }
                }
            }
        }
        scs->seq_header.enable_intra_edge_filter = is_ang_refinement_pruned ? 1 : 0;
#endif
    } else {
        scs->seq_header.enable_intra_edge_filter = 1;
    }
    if (scs->static_config.enable_restoration_filtering == DEFAULT) {
        // As allocation has already happened based on the initial input resolution, the resolution
        // changes should not impact enabling restoration. For some presets, restoration is off for 8K
        // and above and memory allocation is not performed. So, if we switch to smaller resolution, we need
        // to keep restoration off
        EbInputResolution init_input_resolution;
        svt_aom_derive_input_resolution(&init_input_resolution,
                                        scs->max_initial_input_luma_width * scs->max_initial_input_luma_height);
#if TUNE_STILL_IMAGE
        scs->seq_header.enable_restoration = allintra
            ? svt_aom_get_enable_restoration_allintra(scs->static_config.enc_mode,
                                                      scs->static_config.enable_restoration_filtering)
            : rtc_tune ? svt_aom_get_enable_restoration_rtc(scs->static_config.enc_mode,
                                                            scs->static_config.enable_restoration_filtering,
                                                            init_input_resolution,
                                                            scs->static_config.fast_decode)
                       : svt_aom_get_enable_restoration_default(scs->static_config.enc_mode,
                                                                scs->static_config.enable_restoration_filtering,
                                                                init_input_resolution,
                                                                scs->static_config.fast_decode);
#else
        scs->seq_header.enable_restoration = svt_aom_get_enable_restoration(
            scs->static_config.enc_mode,
            scs->static_config.enable_restoration_filtering,
            init_input_resolution,
            scs->static_config.fast_decode,
            allintra,
            rtc_tune);
#endif
    } else {
        scs->seq_header.enable_restoration = (uint8_t)scs->static_config.enable_restoration_filtering;
    }

    if (scs->static_config.cdef_level == DEFAULT) {
        scs->seq_header.cdef_level = 1;
    } else {
        scs->seq_header.cdef_level = (uint8_t)(scs->static_config.cdef_level > 0);
    }

    scs->seq_header.enable_warped_motion = 1;
}

/*
* check if the reference picture is in same frame size
* true -- in same frame size
* false -- reference picture not exist or in difference frame size
*/
bool svt_aom_is_ref_same_size(PictureControlSet* pcs, uint8_t list_idx, uint8_t ref_idx) {
    // skip the checking if reference scaling and super-res are disabled
    if (pcs->ppcs->is_not_scaled) {
        return true;
    }
    if (pcs->slice_type != B_SLICE) {
        return false;
    }
    if (pcs->ref_pic_ptr_array[list_idx][ref_idx] == NULL) {
        return false;
    }

    EbReferenceObject* ref_obj = (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;
    if (ref_obj == NULL || ref_obj->reference_picture == NULL) {
        return false;
    }

    return ref_obj->reference_picture->width == pcs->ppcs->frame_width &&
        ref_obj->reference_picture->height == pcs->ppcs->frame_height;
}

static void set_obmc_controls(ModeDecisionContext* ctx, uint8_t obmc_mode) {
    ObmcControls* obmc_ctrls = &ctx->obmc_ctrls;
    switch (obmc_mode) {
    case 0:
        obmc_ctrls->enabled = 0;
        break;
    case 1:
        obmc_ctrls->enabled                = 1;
        obmc_ctrls->max_blk_size_to_refine = 128;
        obmc_ctrls->max_blk_size           = 128;
        obmc_ctrls->refine_level           = 0;
        obmc_ctrls->trans_face_off         = 0;
        obmc_ctrls->fpel_search_range      = 16;
        obmc_ctrls->fpel_search_diag       = 1;
        break;
    case 2:
        obmc_ctrls->enabled                = 1;
        obmc_ctrls->max_blk_size_to_refine = 64;
        obmc_ctrls->max_blk_size           = 128;
        obmc_ctrls->refine_level           = 1;
        obmc_ctrls->trans_face_off         = 0;
        obmc_ctrls->fpel_search_range      = 16;
        obmc_ctrls->fpel_search_diag       = 1;
        break;
    case 3:
        obmc_ctrls->enabled                = 1;
        obmc_ctrls->max_blk_size_to_refine = 32;
        obmc_ctrls->max_blk_size           = 128;
        obmc_ctrls->refine_level           = 1;
        obmc_ctrls->trans_face_off         = 0;
        obmc_ctrls->fpel_search_range      = 8;
        obmc_ctrls->fpel_search_diag       = 0;
        break;
    case 4:
        obmc_ctrls->enabled                = 1;
        obmc_ctrls->max_blk_size_to_refine = 32;
        obmc_ctrls->max_blk_size           = 128;
        obmc_ctrls->refine_level           = 1;
        obmc_ctrls->trans_face_off         = 1;
        obmc_ctrls->fpel_search_range      = 16;
        obmc_ctrls->fpel_search_diag       = 1;
        break;
    case 5:
        obmc_ctrls->enabled                = 1;
        obmc_ctrls->max_blk_size_to_refine = 32;
        obmc_ctrls->max_blk_size           = 32;
        obmc_ctrls->refine_level           = 4;
        obmc_ctrls->trans_face_off         = 1;
        obmc_ctrls->fpel_search_range      = 8;
        obmc_ctrls->fpel_search_diag       = 0;
        break;
    case 6:
        obmc_ctrls->enabled                = 1;
        obmc_ctrls->max_blk_size_to_refine = 16;
        obmc_ctrls->max_blk_size           = 16;
        obmc_ctrls->refine_level           = 4;
        obmc_ctrls->trans_face_off         = 1;
        obmc_ctrls->fpel_search_range      = 8;
        obmc_ctrls->fpel_search_diag       = 0;
        break;
    default:
        obmc_ctrls->enabled = 0;
        break;
    }
}

// Return true if the passed dimensions require 8x8 blocks for conformance.
// The function assumes the max block size is 64x64 because ISLICEs do not use 128x128 blocks,
// even when enabled at the sequence level. The function is not NSQ aware.
static bool dimensions_require_8x8(const uint16_t aligned_width, const uint16_t aligned_height) {
    // start checking if 64x64 is allowed b/c we don't use 128x128 for ISLICE
    const uint16_t start_bsize     = 64;
    uint16_t       leftover_width  = (aligned_width % start_bsize);
    uint16_t       leftover_height = (aligned_height % start_bsize);
    for (uint16_t half_bsize = start_bsize >> 1; half_bsize >= 8; half_bsize >>= 1) {
        // If allowable block size is found, don't require 8x8
        if ((leftover_width == 0 || leftover_width > half_bsize) &&
            (leftover_height == 0 || leftover_height > half_bsize)) {
            return false;
        }
        leftover_width  = leftover_width % half_bsize;
        leftover_height = leftover_height % half_bsize;
    }
    // No allowable block size from 16x16-64x64 was found; therefore, require 8x8
    return true;
}

static void set_depth_removal_level_controls(PictureControlSet* pcs, ModeDecisionContext* ctx,
                                             uint8_t depth_removal_level) {
    DepthRemovalCtrls* depth_removal_ctrls = &ctx->depth_removal_ctrls;
    if (pcs->slice_type == I_SLICE) {
#if CLN_DR
        depth_removal_ctrls->enabled = 0;
#else
        // Base term (resolution-independent)
        uint16_t base_var_th_b16 = 0;
        uint16_t base_var_th_b32 = 0;
        uint16_t base_var_th_b64 = 0;

        switch (depth_removal_level) {
        case 0:
            depth_removal_ctrls->enabled = 0;
            break;
        case 1:
            depth_removal_ctrls->enabled = 1;
            base_var_th_b16              = 500;
            base_var_th_b32              = 0;
            base_var_th_b64              = 0;
            break;
        case 2:
            depth_removal_ctrls->enabled = 1;
            base_var_th_b16              = 1500;
            base_var_th_b32              = 0;
            base_var_th_b64              = 10;
            break;

        case 3:
            depth_removal_ctrls->enabled = 1;
            base_var_th_b16              = 15000;
            base_var_th_b32              = 200;
            base_var_th_b64              = 0;
            break;
        }

        if (depth_removal_ctrls->enabled) {
            SbGeom* sb_geom = &pcs->ppcs->sb_geom[ctx->sb_index];

            uint16_t variance;
            if (pcs->scs->super_block_size == 64) {
                variance = pcs->ppcs->variance[ctx->sb_index][ME_TIER_ZERO_PU_64x64];
            } else {
                get_sb128_variance(pcs, ctx, &variance);
            }

            uint32_t q_weight, q_weight_denom;
            svt_aom_get_qp_based_th_scaling_factors(
                pcs->scs->qp_based_th_scaling_ctrls.i_depth_removal_qp_based_th_scaling,
                &q_weight,
                &q_weight_denom,
                pcs->scs->static_config.qp);
            uint16_t var_th_b16                       = base_var_th_b16 == (uint16_t)~0
                                      ? base_var_th_b16
                                      : DIVIDE_AND_ROUND(base_var_th_b16 * q_weight, q_weight_denom);
            uint16_t var_th_b32                       = base_var_th_b32 == (uint16_t)~0
                                      ? base_var_th_b32
                                      : DIVIDE_AND_ROUND(base_var_th_b32 * q_weight, q_weight_denom);
            uint16_t var_th_b64                       = base_var_th_b64 == (uint16_t)~0
                                      ? base_var_th_b64
                                      : DIVIDE_AND_ROUND(base_var_th_b64 * q_weight, q_weight_denom);

            depth_removal_ctrls->disallow_below_64x64 = (ctx->max_block_size >= 64 && sb_geom->width % 64 == 0 &&
                                                         sb_geom->height % 64 == 0)
                ? (depth_removal_ctrls->disallow_below_64x64 || variance < var_th_b64)
                : 0;

            depth_removal_ctrls->disallow_below_32x32 = (sb_geom->width % 32 == 0 && sb_geom->height % 32 == 0)
                ? (depth_removal_ctrls->disallow_below_32x32 || variance < var_th_b32)
                : 0;

            depth_removal_ctrls->disallow_below_16x16 = (sb_geom->width % 16 == 0 && sb_geom->height % 16 == 0)
                ? (depth_removal_ctrls->disallow_below_16x16 || variance < var_th_b16)
                : 0;
        }
#endif
    } else {
        // me_distortion => EB_8_BIT_MD
        uint32_t fast_lambda = ctx->fast_lambda_md[EB_8_BIT_MD];

        uint32_t sb_size = 64 * 64;

        uint64_t cost_th_rate = 1 << 13;

        uint64_t disallow_4x4_cost_th_multiplier         = 0;
        uint64_t disallow_below_16x16_cost_th_multiplier = 0;
        uint64_t disallow_below_32x32_cost_th_multiplier = 0;
        uint64_t disallow_below_64x64_cost_th_multiplier = 0;

        int64_t dev_16x16_to_8x8_th   = MAX_SIGNED_VALUE;
        int64_t dev_32x32_to_16x16_th = MAX_SIGNED_VALUE;
        int64_t dev_32x32_to_8x8_th   = MAX_SIGNED_VALUE;

        int8_t qp_scale_factor = 0;

        // modulate the depth-removal level using: (1) tpl-information, specifically the SB delta-qp, and (2) the high-freq information
        if (pcs->ppcs->frm_hdr.delta_q_params.delta_q_present || pcs->ppcs->r0_delta_qp_md) {
            int diff = ctx->sb_ptr->qindex - quantizer_to_qindex[pcs->ppcs->picture_qp];
            if (diff <= -12) {
                depth_removal_level = MAX(0, (int)depth_removal_level - 4);
            } else if (diff <= -6) {
                depth_removal_level = MAX(0, (int)depth_removal_level - 3);
            } else if (diff <= -3) {
                depth_removal_level = MAX(0, (int)depth_removal_level - 2);
            } else if (diff < 0) {
                depth_removal_level = MAX(0, (int)depth_removal_level - 1);
            }
        }

        switch (depth_removal_level) {
        case 0:
            depth_removal_ctrls->enabled = 0;
            break;

        case 1:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 0;
            disallow_below_32x32_cost_th_multiplier = 0;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 0;
            dev_32x32_to_16x16_th                   = 0;
            qp_scale_factor                         = 1;
            break;

        case 2:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 0;
            disallow_below_32x32_cost_th_multiplier = 0;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 10;
            dev_32x32_to_16x16_th                   = 0;
            qp_scale_factor                         = 1;
            break;
        case 3:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 0;
            disallow_below_32x32_cost_th_multiplier = 0;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 20;
            dev_32x32_to_16x16_th                   = 0;
            qp_scale_factor                         = 1;
            break;
        case 4:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 0;
            disallow_below_32x32_cost_th_multiplier = 0;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 30;
            dev_32x32_to_16x16_th                   = 0;
            qp_scale_factor                         = 1;
            break;
        case 5:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 6;
            disallow_below_32x32_cost_th_multiplier = 6;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 40;
            dev_32x32_to_16x16_th                   = 0;
            qp_scale_factor                         = 1;
            break;
        case 6:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 6;
            disallow_below_32x32_cost_th_multiplier = 6;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 50;
            dev_32x32_to_16x16_th                   = 25;
            qp_scale_factor                         = 1;
            break;
        case 7:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 6;
            disallow_below_32x32_cost_th_multiplier = 6;
            disallow_below_64x64_cost_th_multiplier = 0;
            dev_16x16_to_8x8_th                     = 50;
            dev_32x32_to_16x16_th                   = 25;
            qp_scale_factor                         = 2;
            break;
        case 8:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 16;
            disallow_below_32x32_cost_th_multiplier = 8;
            disallow_below_64x64_cost_th_multiplier = 8;
            dev_16x16_to_8x8_th                     = 100;
            dev_32x32_to_16x16_th                   = 50;
            qp_scale_factor                         = 3;
            break;
        case 9:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 32;
            disallow_below_32x32_cost_th_multiplier = 8;
            disallow_below_64x64_cost_th_multiplier = 8;
            dev_16x16_to_8x8_th                     = 100;
            dev_32x32_to_16x16_th                   = 50;
            qp_scale_factor                         = 3;
            break;
        case 10:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 128;
            disallow_below_32x32_cost_th_multiplier = 8;
            disallow_below_64x64_cost_th_multiplier = 8;
            dev_16x16_to_8x8_th                     = 200;
            dev_32x32_to_16x16_th                   = 75;
            qp_scale_factor                         = 3;
            break;
        case 11:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 128;
            disallow_below_32x32_cost_th_multiplier = 8;
            disallow_below_64x64_cost_th_multiplier = 8;
            dev_16x16_to_8x8_th                     = 250;
            dev_32x32_to_16x16_th                   = 125;
            qp_scale_factor                         = 3;
            break;
        case 12:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 128;
            disallow_below_32x32_cost_th_multiplier = 16;
            disallow_below_64x64_cost_th_multiplier = 8;
            dev_16x16_to_8x8_th                     = 250;
            dev_32x32_to_16x16_th                   = 150;
            qp_scale_factor                         = 4;
            break;
        case 13:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 256;
            disallow_below_32x32_cost_th_multiplier = 16;
            disallow_below_64x64_cost_th_multiplier = 8;
            dev_16x16_to_8x8_th                     = 250;
            dev_32x32_to_16x16_th                   = 150;
            qp_scale_factor                         = 4;
            break;
        case 14:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 256;
            disallow_below_32x32_cost_th_multiplier = 16;
            disallow_below_64x64_cost_th_multiplier = 16;
            dev_16x16_to_8x8_th                     = 250;
            dev_32x32_to_16x16_th                   = 150;
            qp_scale_factor                         = 4;
            break;
        case 15:
            depth_removal_ctrls->enabled            = 1;
            disallow_4x4_cost_th_multiplier         = 64;
            disallow_below_16x16_cost_th_multiplier = 384;
            disallow_below_32x32_cost_th_multiplier = 24;
            disallow_below_64x64_cost_th_multiplier = 24;
            dev_16x16_to_8x8_th                     = 300;
            dev_32x32_to_16x16_th                   = 200;
            qp_scale_factor                         = 4;
            break;
        }
        if (depth_removal_ctrls->enabled) {
            SbGeom* sb_geom = &pcs->ppcs->sb_geom[ctx->sb_index];

            // Use info from ref. frames (if available)
            bool use_ref_info = true;
            if (use_ref_info) {
                const bool is_ref_l0_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0);
                const bool is_ref_l1_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0);

                if (pcs->slice_type != I_SLICE && is_ref_l0_avail) {
                    EbReferenceObject* ref_obj_l0 =
                        (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;

                    uint8_t sb_min_sq_size = (uint8_t)~0;

                    if (abs((int)(pcs->picture_number - ref_obj_l0->ref_poc)) <= 1) {
                        sb_min_sq_size = ref_obj_l0->sb_min_sq_size[ctx->sb_index];
                    }

                    if (pcs->slice_type == B_SLICE && is_ref_l1_avail && pcs->ppcs->ref_list1_count_try) {
                        EbReferenceObject* ref_obj_l1 =
                            (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;

                        if (abs((int)(pcs->picture_number - ref_obj_l1->ref_poc)) <= 1) {
                            sb_min_sq_size = MIN(sb_min_sq_size, ref_obj_l1->sb_min_sq_size[ctx->sb_index]);
                        }
                    }

                    if (sb_min_sq_size != (uint8_t)~0) {
                        if (sb_min_sq_size >= 64) {
                            dev_32x32_to_16x16_th = dev_32x32_to_16x16_th + 5;
                            dev_16x16_to_8x8_th   = dev_16x16_to_8x8_th + 20;
                        } else if (sb_min_sq_size >= 32) {
                            dev_16x16_to_8x8_th = dev_16x16_to_8x8_th + 15;
                        }
                    }
                }
            }
            uint32_t dist_64, dist_32, dist_16, dist_8, me_8x8_cost_variance;
            if (pcs->scs->super_block_size == 64) {
                dist_64              = pcs->ppcs->me_64x64_distortion[ctx->sb_index];
                dist_32              = pcs->ppcs->me_32x32_distortion[ctx->sb_index];
                dist_16              = pcs->ppcs->me_16x16_distortion[ctx->sb_index];
                dist_8               = pcs->ppcs->me_8x8_distortion[ctx->sb_index];
                me_8x8_cost_variance = pcs->ppcs->me_8x8_cost_variance[ctx->sb_index];
            } else {
                get_sb128_me_data(pcs, ctx, &dist_64, &dist_32, &dist_16, &dist_8, &me_8x8_cost_variance);
            }

            //dev_16x16_to_8x8_th , dev_32x32_to_16x16_th = f(me_8x8_cost_variance)
            me_8x8_cost_variance /= MAX((MAX(63 - (pcs->ppcs->picture_qp + 10), 1)), 1);
            if (me_8x8_cost_variance < LOW_8x8_DIST_VAR_TH) {
                dev_16x16_to_8x8_th = dev_16x16_to_8x8_th << 2;
            } else if (me_8x8_cost_variance < HIGH_8x8_DIST_VAR_TH) {
                dev_16x16_to_8x8_th   = dev_16x16_to_8x8_th << 1;
                dev_32x32_to_16x16_th = dev_32x32_to_16x16_th >> 1;
            } else {
                dev_16x16_to_8x8_th   = 0;
                dev_32x32_to_16x16_th = 0;
            }

            //dev_16x16_to_8x8_th , dev_32x32_to_16x16_th = f(QP)
            dev_16x16_to_8x8_th *= MAX((MAX(63 - (pcs->ppcs->picture_qp + 10), 1) >> 4), 1) * qp_scale_factor;
            dev_32x32_to_16x16_th *= MAX((MAX(63 - (pcs->ppcs->picture_qp + 10), 1) >> 4), 1) * qp_scale_factor;
            // dev_32x32_to_8x8_th = f(dev_32x32_to_16x16_th); a bit higher
            dev_32x32_to_8x8_th = (dev_32x32_to_16x16_th * ((1 << 2) + 1)) >> 2;

            uint64_t disallow_below_16x16_cost_th = disallow_below_16x16_cost_th_multiplier
                ? RDCOST(fast_lambda, cost_th_rate, (sb_size >> 3) * disallow_below_16x16_cost_th_multiplier)
                : 0;
            uint64_t disallow_below_32x32_cost_th = disallow_below_32x32_cost_th_multiplier
                ? RDCOST(fast_lambda, cost_th_rate, (sb_size >> 3) * disallow_below_32x32_cost_th_multiplier)
                : 0;
            uint64_t disallow_below_64x64_cost_th = disallow_below_64x64_cost_th_multiplier
                ? RDCOST(fast_lambda, cost_th_rate, (sb_size >> 3) * disallow_below_64x64_cost_th_multiplier)
                : 0;

            uint64_t cost_64x64 = RDCOST(fast_lambda, 0, dist_64);
            uint64_t cost_32x32 = RDCOST(fast_lambda, 0, dist_32);
            uint64_t cost_16x16 = RDCOST(fast_lambda, 0, dist_16);
            uint64_t cost_8x8   = RDCOST(fast_lambda, 0, dist_8);

            int64_t dev_32x32_to_16x16 = (int64_t)(((int64_t)MAX(cost_32x32, 1) - (int64_t)MAX(cost_16x16, 1)) * 1000) /
                (int64_t)MAX(cost_16x16, 1);

            int64_t dev_32x32_to_8x8 = (int64_t)(((int64_t)MAX(cost_32x32, 1) - (int64_t)MAX(cost_8x8, 1)) * 1000) /
                (int64_t)MAX(cost_8x8, 1);

            int64_t dev_16x16_to_8x8 = (int64_t)(((int64_t)MAX(cost_16x16, 1) - (int64_t)MAX(cost_8x8, 1)) * 1000) /
                (int64_t)MAX(cost_8x8, 1);

            // Enable depth removal at a given depth if the entire SB can be covered by blocks of that size (to avoid
            // disallowing necessary blocks).
            depth_removal_ctrls->disallow_below_64x64 = (((sb_geom->width % 64) == 0 || (sb_geom->width % 64) > 32) &&
                                                         ((sb_geom->height % 64) == 0 || (sb_geom->height % 64) > 32))
                ? (depth_removal_ctrls->disallow_below_64x64 || cost_64x64 < disallow_below_64x64_cost_th)
                : 0;

            depth_removal_ctrls->disallow_below_32x32 = (((sb_geom->width % 32) == 0 || (sb_geom->width % 32) > 16) &&
                                                         ((sb_geom->height % 32) == 0 || (sb_geom->height % 32) > 16))
                ? (depth_removal_ctrls->disallow_below_32x32 || cost_32x32 < disallow_below_32x32_cost_th ||
                   (dev_32x32_to_16x16 < dev_32x32_to_16x16_th && dev_32x32_to_8x8 < dev_32x32_to_8x8_th))
                : 0;

            depth_removal_ctrls->disallow_below_16x16 = (!dimensions_require_8x8(sb_geom->width, sb_geom->height))
                ? (depth_removal_ctrls->disallow_below_16x16 || cost_16x16 < disallow_below_16x16_cost_th ||
                   dev_16x16_to_8x8 < dev_16x16_to_8x8_th)
                : 0;
            if (!ctx->disallow_4x4 && disallow_4x4_cost_th_multiplier) {
                uint64_t disallow_4x4_cost_th = RDCOST(
                    fast_lambda, cost_th_rate, (sb_size >> 1) * disallow_4x4_cost_th_multiplier);

                if (cost_8x8 < disallow_4x4_cost_th && me_8x8_cost_variance < LOW_8x8_DIST_VAR_TH) {
                    ctx->disallow_4x4 = 1;
                }
            }
        }
    }
}

/*
 * Control NSQ search
 */
static void md_nsq_motion_search_controls(ModeDecisionContext* ctx, uint8_t md_nsq_mv_search_level) {
    MdNsqMotionSearchCtrls* md_nsq_me_ctrls = &ctx->md_nsq_me_ctrls;

    switch (md_nsq_mv_search_level) {
    case 0:
        md_nsq_me_ctrls->enabled = 0;
        break;
    case 1:
        md_nsq_me_ctrls->enabled                = 1;
        md_nsq_me_ctrls->dist_type              = VAR;
        md_nsq_me_ctrls->full_pel_search_width  = 32;
        md_nsq_me_ctrls->full_pel_search_height = 16;
        md_nsq_me_ctrls->enable_psad            = 1;
        break;
    case 2:
        md_nsq_me_ctrls->enabled                = 1;
        md_nsq_me_ctrls->dist_type              = VAR;
        md_nsq_me_ctrls->full_pel_search_width  = 16;
        md_nsq_me_ctrls->full_pel_search_height = 8;
        md_nsq_me_ctrls->enable_psad            = 1;
        break;
    default:
        assert(0);
        break;
    }
}

void svt_aom_md_pme_search_controls(ModeDecisionContext* ctx, uint8_t md_pme_level) {
    MdPmeCtrls* md_pme_ctrls = &ctx->md_pme_ctrls;

    switch (md_pme_level) {
    case 0:
        md_pme_ctrls->enabled = 0;
        break;
    case 1:
        md_pme_ctrls->enabled                      = 1;
        md_pme_ctrls->dist_type                    = VAR;
        md_pme_ctrls->full_pel_search_width        = 9;
        md_pme_ctrls->full_pel_search_height       = 9;
        md_pme_ctrls->early_check_mv_th_multiplier = MIN_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_cost_th     = MAX_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_mv_th       = MIN_SIGNED_VALUE;
        md_pme_ctrls->post_fp_pme_to_me_cost_th    = MAX_SIGNED_VALUE;
        md_pme_ctrls->post_fp_pme_to_me_mv_th      = MIN_SIGNED_VALUE;
        md_pme_ctrls->enable_psad                  = 0;
        md_pme_ctrls->sa_q_weight                  = 0;
        break;
    case 2:
        md_pme_ctrls->enabled                      = 1;
        md_pme_ctrls->dist_type                    = VAR;
        md_pme_ctrls->full_pel_search_width        = 9;
        md_pme_ctrls->full_pel_search_height       = 9;
        md_pme_ctrls->early_check_mv_th_multiplier = MIN_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_cost_th     = MAX_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_mv_th       = MIN_SIGNED_VALUE;
        md_pme_ctrls->post_fp_pme_to_me_cost_th    = MAX_SIGNED_VALUE;
        md_pme_ctrls->post_fp_pme_to_me_mv_th      = MIN_SIGNED_VALUE;
        md_pme_ctrls->enable_psad                  = 0;
        md_pme_ctrls->sa_q_weight                  = 1;
        break;
    case 3:
        md_pme_ctrls->enabled                      = 1;
        md_pme_ctrls->dist_type                    = VAR;
        md_pme_ctrls->full_pel_search_width        = 9;
        md_pme_ctrls->full_pel_search_height       = 7;
        md_pme_ctrls->early_check_mv_th_multiplier = MIN_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_cost_th     = MAX_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_mv_th       = 16;
        md_pme_ctrls->post_fp_pme_to_me_cost_th    = 50;
        md_pme_ctrls->post_fp_pme_to_me_mv_th      = MIN_SIGNED_VALUE;
        md_pme_ctrls->enable_psad                  = 0;
        md_pme_ctrls->sa_q_weight                  = 1;
        break;
    case 4:
        md_pme_ctrls->enabled                      = 1;
        md_pme_ctrls->dist_type                    = SAD;
        md_pme_ctrls->full_pel_search_width        = 7;
        md_pme_ctrls->full_pel_search_height       = 5;
        md_pme_ctrls->early_check_mv_th_multiplier = MIN_SIGNED_VALUE;
        md_pme_ctrls->pre_fp_pme_to_me_cost_th     = 25;
        md_pme_ctrls->pre_fp_pme_to_me_mv_th       = 16;
        md_pme_ctrls->post_fp_pme_to_me_cost_th    = 50;
        md_pme_ctrls->post_fp_pme_to_me_mv_th      = 32;
        md_pme_ctrls->enable_psad                  = 1;
        md_pme_ctrls->sa_q_weight                  = 1;
        break;
    case 5:
        md_pme_ctrls->enabled                      = 1;
        md_pme_ctrls->dist_type                    = SAD;
        md_pme_ctrls->full_pel_search_width        = 7;
        md_pme_ctrls->full_pel_search_height       = 5;
        md_pme_ctrls->early_check_mv_th_multiplier = 64;
        md_pme_ctrls->pre_fp_pme_to_me_cost_th     = 25;
        md_pme_ctrls->pre_fp_pme_to_me_mv_th       = 16;
        md_pme_ctrls->post_fp_pme_to_me_cost_th    = 50;
        md_pme_ctrls->post_fp_pme_to_me_mv_th      = 32;
        md_pme_ctrls->enable_psad                  = 1;
        md_pme_ctrls->sa_q_weight                  = 1;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_subres_controls(ModeDecisionContext* ctx, uint8_t subres_level) {
    SubresCtrls* subres_ctrls = &ctx->subres_ctrls;

    switch (subres_level) {
    case 0:
        subres_ctrls->step = 0;
        break;
    case 1:
        subres_ctrls->step = 1;
        break;
    case 2:
        subres_ctrls->step = 2;
        break;
    default:
        assert(0);
        break;
    }
    // Set the TH used to determine if subres is safe to use (based on ODD vs. EVEN rows' distortion)
    if (subres_ctrls->step == 0) {
        subres_ctrls->odd_to_even_deviation_th = 0;
    } else {
        subres_ctrls->odd_to_even_deviation_th = 5;
    }
}

static void set_pf_controls(ModeDecisionContext* ctx, uint8_t pf_level) {
    PfCtrls* pf_ctrls = &ctx->pf_ctrls;

    switch (pf_level) {
    case 0:
        pf_ctrls->pf_shape = ONLY_DC_SHAPE;
        break;
    case 1:
        pf_ctrls->pf_shape = DEFAULT_SHAPE;
        break;
    case 2:
        pf_ctrls->pf_shape = N2_SHAPE;
        break;
    case 3:
        pf_ctrls->pf_shape = N4_SHAPE;
        break;
    default:
        assert(0);
        break;
    }
}

/*
 * Control Adaptive ME search
 */
static void md_sq_motion_search_controls(ModeDecisionContext* ctx, uint8_t md_sq_mv_search_level) {
    MdSqMotionSearchCtrls* md_sq_me_ctrls = &ctx->md_sq_me_ctrls;

    switch (md_sq_mv_search_level) {
    case 0:
        md_sq_me_ctrls->enabled = 0;
        break;
    case 1:
        md_sq_me_ctrls->enabled   = 1;
        md_sq_me_ctrls->dist_type = SAD;

        md_sq_me_ctrls->pame_distortion_th = 10;

        md_sq_me_ctrls->sprs_lev0_enabled    = 1;
        md_sq_me_ctrls->sprs_lev0_step       = 4;
        md_sq_me_ctrls->sprs_lev0_w          = 15;
        md_sq_me_ctrls->sprs_lev0_h          = 15;
        md_sq_me_ctrls->max_sprs_lev0_w      = 150;
        md_sq_me_ctrls->max_sprs_lev0_h      = 150;
        md_sq_me_ctrls->sprs_lev0_multiplier = 500;

        md_sq_me_ctrls->sprs_lev1_enabled    = 1;
        md_sq_me_ctrls->sprs_lev1_step       = 2;
        md_sq_me_ctrls->sprs_lev1_w          = 4;
        md_sq_me_ctrls->sprs_lev1_h          = 4;
        md_sq_me_ctrls->max_sprs_lev1_w      = 50;
        md_sq_me_ctrls->max_sprs_lev1_h      = 50;
        md_sq_me_ctrls->sprs_lev1_multiplier = 500;

        md_sq_me_ctrls->sprs_lev2_enabled = 1;
        md_sq_me_ctrls->sprs_lev2_step    = 1;
        md_sq_me_ctrls->sprs_lev2_w       = 3;
        md_sq_me_ctrls->sprs_lev2_h       = 3;
        md_sq_me_ctrls->enable_psad       = 1;
        break;
    case 2:
        md_sq_me_ctrls->enabled            = 1;
        md_sq_me_ctrls->dist_type          = SAD;
        md_sq_me_ctrls->pame_distortion_th = 10;

        md_sq_me_ctrls->sprs_lev0_enabled    = 1;
        md_sq_me_ctrls->sprs_lev0_step       = 4;
        md_sq_me_ctrls->sprs_lev0_w          = 15;
        md_sq_me_ctrls->sprs_lev0_h          = 15;
        md_sq_me_ctrls->max_sprs_lev0_w      = 150;
        md_sq_me_ctrls->max_sprs_lev0_h      = 150;
        md_sq_me_ctrls->sprs_lev0_multiplier = 400;

        md_sq_me_ctrls->sprs_lev1_enabled    = 1;
        md_sq_me_ctrls->sprs_lev1_step       = 2;
        md_sq_me_ctrls->sprs_lev1_w          = 4;
        md_sq_me_ctrls->sprs_lev1_h          = 4;
        md_sq_me_ctrls->max_sprs_lev1_w      = 50;
        md_sq_me_ctrls->max_sprs_lev1_h      = 50;
        md_sq_me_ctrls->sprs_lev1_multiplier = 400;

        md_sq_me_ctrls->sprs_lev2_enabled = 1;
        md_sq_me_ctrls->sprs_lev2_step    = 1;
        md_sq_me_ctrls->sprs_lev2_w       = 3;
        md_sq_me_ctrls->sprs_lev2_h       = 3;
        md_sq_me_ctrls->enable_psad       = 1;
        break;
    case 3:
        md_sq_me_ctrls->enabled            = 1;
        md_sq_me_ctrls->dist_type          = SAD;
        md_sq_me_ctrls->pame_distortion_th = 10;

        md_sq_me_ctrls->sprs_lev0_enabled    = 1;
        md_sq_me_ctrls->sprs_lev0_step       = 4;
        md_sq_me_ctrls->sprs_lev0_w          = 15;
        md_sq_me_ctrls->sprs_lev0_h          = 15;
        md_sq_me_ctrls->max_sprs_lev0_w      = 150;
        md_sq_me_ctrls->max_sprs_lev0_h      = 150;
        md_sq_me_ctrls->sprs_lev0_multiplier = 300;

        md_sq_me_ctrls->sprs_lev1_enabled    = 1;
        md_sq_me_ctrls->sprs_lev1_step       = 2;
        md_sq_me_ctrls->sprs_lev1_w          = 4;
        md_sq_me_ctrls->sprs_lev1_h          = 4;
        md_sq_me_ctrls->max_sprs_lev1_w      = 50;
        md_sq_me_ctrls->max_sprs_lev1_h      = 50;
        md_sq_me_ctrls->sprs_lev1_multiplier = 300;

        md_sq_me_ctrls->sprs_lev2_enabled = 1;
        md_sq_me_ctrls->sprs_lev2_step    = 1;
        md_sq_me_ctrls->sprs_lev2_w       = 3;
        md_sq_me_ctrls->sprs_lev2_h       = 3;
        md_sq_me_ctrls->enable_psad       = 1;
        break;
    case 4:
        md_sq_me_ctrls->enabled            = 1;
        md_sq_me_ctrls->dist_type          = SAD;
        md_sq_me_ctrls->pame_distortion_th = 10;

        md_sq_me_ctrls->sprs_lev0_enabled    = 1;
        md_sq_me_ctrls->sprs_lev0_step       = 4;
        md_sq_me_ctrls->sprs_lev0_w          = 15;
        md_sq_me_ctrls->sprs_lev0_h          = 15;
        md_sq_me_ctrls->max_sprs_lev0_w      = 150;
        md_sq_me_ctrls->max_sprs_lev0_h      = 150;
        md_sq_me_ctrls->sprs_lev0_multiplier = 100;

        md_sq_me_ctrls->sprs_lev1_enabled    = 1;
        md_sq_me_ctrls->sprs_lev1_step       = 2;
        md_sq_me_ctrls->sprs_lev1_w          = 4;
        md_sq_me_ctrls->sprs_lev1_h          = 4;
        md_sq_me_ctrls->max_sprs_lev1_w      = 50;
        md_sq_me_ctrls->max_sprs_lev1_h      = 50;
        md_sq_me_ctrls->sprs_lev1_multiplier = 100;

        md_sq_me_ctrls->sprs_lev2_enabled = 1;
        md_sq_me_ctrls->sprs_lev2_step    = 1;
        md_sq_me_ctrls->sprs_lev2_w       = 3;
        md_sq_me_ctrls->sprs_lev2_h       = 3;
        md_sq_me_ctrls->enable_psad       = 1;
        break;
    default:
        assert(0);
        break;
    }
}

/*
 * Control Subpel search of ME MV(s)
 */
static void md_subpel_me_controls(ModeDecisionContext* ctx, uint8_t md_subpel_me_level) {
    MdSubPelSearchCtrls* md_subpel_me_ctrls = &ctx->md_subpel_me_ctrls;

    switch (md_subpel_me_level) {
    case 0:
        md_subpel_me_ctrls->enabled = 0;
        break;
    case 1:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_8_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 2;
        md_subpel_me_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE;
        md_subpel_me_ctrls->pred_variance_th      = 0;
        md_subpel_me_ctrls->abs_th_mult           = 0;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 0;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 0;
        md_subpel_me_ctrls->mvp_th                = 0;
        md_subpel_me_ctrls->hp_mv_th              = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->bias_fp               = 0;
        break;
    case 2:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 2;
        md_subpel_me_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE;
        md_subpel_me_ctrls->pred_variance_th      = 0;
        md_subpel_me_ctrls->abs_th_mult           = 0;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 0;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 18;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 0;
        break;
    case 3:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 2;
        md_subpel_me_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE;
        md_subpel_me_ctrls->pred_variance_th      = 0;
        md_subpel_me_ctrls->abs_th_mult           = 0;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 0;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 18;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 104;
        break;
    case 4:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 2;
        md_subpel_me_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 0;
        md_subpel_me_ctrls->abs_th_mult           = 0;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 0;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 18;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 104;
        break;
    case 5:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 2;
        md_subpel_me_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 0;
        md_subpel_me_ctrls->abs_th_mult           = 0;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 0;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 18;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 110;
        break;
    case 6:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 1;
        md_subpel_me_ctrls->max_precision         = QUARTER_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 0;
        md_subpel_me_ctrls->abs_th_mult           = 0;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 3;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 12;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 110;
        break;
    case 7:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 1;
        md_subpel_me_ctrls->max_precision         = QUARTER_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 50;
        md_subpel_me_ctrls->abs_th_mult           = 8;
        md_subpel_me_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_me_ctrls->skip_diag_refinement  = 3;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 12;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 110;
        break;
    case 8:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 1;
        md_subpel_me_ctrls->max_precision         = QUARTER_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 100;
        md_subpel_me_ctrls->abs_th_mult           = 8;
        md_subpel_me_ctrls->round_dev_th          = -15;
        md_subpel_me_ctrls->skip_diag_refinement  = 4;
        md_subpel_me_ctrls->skip_zz_mv            = 0;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 12;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 110;
        break;
    case 9:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 1;
        md_subpel_me_ctrls->max_precision         = QUARTER_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 100;
        md_subpel_me_ctrls->abs_th_mult           = 8;
        md_subpel_me_ctrls->round_dev_th          = -25;
        md_subpel_me_ctrls->skip_diag_refinement  = 4;
        md_subpel_me_ctrls->skip_zz_mv            = 1;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 12;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 110;
        break;
    case 10:
        md_subpel_me_ctrls->enabled               = 1;
        md_subpel_me_ctrls->subpel_search_type    = USE_4_TAPS;
        md_subpel_me_ctrls->subpel_iters_per_step = 1;
        md_subpel_me_ctrls->max_precision         = HALF_PEL;
        md_subpel_me_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_me_ctrls->pred_variance_th      = 100;
        md_subpel_me_ctrls->abs_th_mult           = 8;
        md_subpel_me_ctrls->round_dev_th          = -25;
        md_subpel_me_ctrls->skip_diag_refinement  = 4;
        md_subpel_me_ctrls->skip_zz_mv            = 1;
        md_subpel_me_ctrls->min_blk_sz            = 4;
        md_subpel_me_ctrls->mvp_th                = 12;
        md_subpel_me_ctrls->hp_mv_th              = 32;
        md_subpel_me_ctrls->bias_fp               = 110;
        break;

    default:
        assert(0);
        break;
    }
}

/*
 * Control Subpel search of PME MV(s)
 */
static void md_subpel_pme_controls(ModeDecisionContext* ctx, uint8_t md_subpel_pme_level) {
    MdSubPelSearchCtrls* md_subpel_pme_ctrls = &ctx->md_subpel_pme_ctrls;

    switch (md_subpel_pme_level) {
    case 0:
        md_subpel_pme_ctrls->enabled = 0;
        break;
    case 1:
        md_subpel_pme_ctrls->enabled               = 1;
        md_subpel_pme_ctrls->subpel_search_type    = USE_8_TAPS;
        md_subpel_pme_ctrls->subpel_iters_per_step = 2;
        md_subpel_pme_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_pme_ctrls->subpel_search_method  = SUBPEL_TREE;
        md_subpel_pme_ctrls->pred_variance_th      = 0;
        md_subpel_pme_ctrls->abs_th_mult           = 0;
        md_subpel_pme_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_pme_ctrls->skip_zz_mv            = 0;
        md_subpel_pme_ctrls->min_blk_sz            = 0;
        md_subpel_pme_ctrls->mvp_th                = 0;
        md_subpel_pme_ctrls->hp_mv_th              = 0;
        md_subpel_pme_ctrls->bias_fp               = 0;
        break;
    case 2:
        md_subpel_pme_ctrls->enabled               = 1;
        md_subpel_pme_ctrls->subpel_search_type    = USE_8_TAPS;
        md_subpel_pme_ctrls->subpel_iters_per_step = 2;
        md_subpel_pme_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_pme_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_pme_ctrls->pred_variance_th      = 0;
        md_subpel_pme_ctrls->abs_th_mult           = 0;
        md_subpel_pme_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_pme_ctrls->skip_zz_mv            = 0;
        md_subpel_pme_ctrls->min_blk_sz            = 0;
        md_subpel_pme_ctrls->mvp_th                = 0;
        md_subpel_pme_ctrls->hp_mv_th              = 0;
        md_subpel_pme_ctrls->bias_fp               = 104;
        break;
    case 3:
        md_subpel_pme_ctrls->enabled               = 1;
        md_subpel_pme_ctrls->subpel_search_type    = USE_8_TAPS;
        md_subpel_pme_ctrls->subpel_iters_per_step = 2;
        md_subpel_pme_ctrls->max_precision         = EIGHTH_PEL;
        md_subpel_pme_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_pme_ctrls->pred_variance_th      = 0;
        md_subpel_pme_ctrls->abs_th_mult           = 0;
        md_subpel_pme_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_pme_ctrls->skip_zz_mv            = 0;
        md_subpel_pme_ctrls->min_blk_sz            = 0;
        md_subpel_pme_ctrls->mvp_th                = 0;
        md_subpel_pme_ctrls->hp_mv_th              = 0;
        md_subpel_pme_ctrls->bias_fp               = 110;
        break;
    case 4:
        md_subpel_pme_ctrls->enabled               = 1;
        md_subpel_pme_ctrls->subpel_search_type    = USE_8_TAPS;
        md_subpel_pme_ctrls->subpel_iters_per_step = 2;
        md_subpel_pme_ctrls->max_precision         = HALF_PEL;
        md_subpel_pme_ctrls->subpel_search_method  = SUBPEL_TREE_PRUNED;
        md_subpel_pme_ctrls->pred_variance_th      = 0;
        md_subpel_pme_ctrls->abs_th_mult           = 0;
        md_subpel_pme_ctrls->round_dev_th          = MAX_SIGNED_VALUE;
        md_subpel_pme_ctrls->skip_zz_mv            = 0;
        md_subpel_pme_ctrls->min_blk_sz            = 0;
        md_subpel_pme_ctrls->mvp_th                = 0;
        md_subpel_pme_ctrls->hp_mv_th              = 0;
        md_subpel_pme_ctrls->bias_fp               = 110;
        break;
    default:
        assert(0);
        break;
    }
}

/*
 * Control RDOQ
 */
static void set_rdoq_controls(ModeDecisionContext* ctx, uint8_t rdoq_level) {
    RdoqCtrls* rdoq_ctrls = &ctx->rdoq_ctrls;

    switch (rdoq_level) {
    case 0:
        rdoq_ctrls->enabled = 0;
        break;
    case 1:
        rdoq_ctrls->enabled = 1;
#if OPT_RDOQ_BIS
        rdoq_ctrls->cut_off_num = 0;
#else
        rdoq_ctrls->cut_off_div       = 0;
        rdoq_ctrls->eob_fast_y_inter  = 0;
        rdoq_ctrls->eob_fast_y_intra  = 0;
        rdoq_ctrls->eob_fast_uv_inter = 0;
        rdoq_ctrls->eob_fast_uv_intra = 0;
        rdoq_ctrls->fp_q_y            = 1;
        rdoq_ctrls->fp_q_uv           = 1;
        rdoq_ctrls->satd_factor       = (uint8_t)~0;
        rdoq_ctrls->early_exit_th     = 0;
#endif
        rdoq_ctrls->skip_uv      = 0;
        rdoq_ctrls->dct_dct_only = 0;
        rdoq_ctrls->eob_th       = (uint8_t)~0;
        rdoq_ctrls->eob_fast_th  = (uint8_t)~0;
        break;
    case 2:
        rdoq_ctrls->enabled = 1;
#if OPT_RDOQ_BIS
        rdoq_ctrls->cut_off_num   = 80;
        rdoq_ctrls->cut_off_denum = 100;
#else
        rdoq_ctrls->cut_off_div       = 4;
        rdoq_ctrls->eob_fast_y_inter  = 0;
        rdoq_ctrls->eob_fast_y_intra  = 0;
        rdoq_ctrls->eob_fast_uv_inter = 0;
        rdoq_ctrls->eob_fast_uv_intra = 0;
        rdoq_ctrls->fp_q_y            = 1;
        rdoq_ctrls->fp_q_uv           = 1;
        rdoq_ctrls->satd_factor       = (uint8_t)~0;
        rdoq_ctrls->early_exit_th     = 0;
#endif
        rdoq_ctrls->skip_uv      = 0;
        rdoq_ctrls->dct_dct_only = 0;
        rdoq_ctrls->eob_th       = (uint8_t)~0;
        rdoq_ctrls->eob_fast_th  = (uint8_t)~0;
        break;
    case 3:
        rdoq_ctrls->enabled = 1;
#if OPT_RDOQ_BIS
        rdoq_ctrls->cut_off_num   = 60;
        rdoq_ctrls->cut_off_denum = 100;
#else
        rdoq_ctrls->cut_off_div       = 4;
        rdoq_ctrls->eob_fast_y_inter  = 0;
        rdoq_ctrls->eob_fast_y_intra  = 0;
        rdoq_ctrls->eob_fast_uv_inter = 0;
        rdoq_ctrls->eob_fast_uv_intra = 0;
        rdoq_ctrls->fp_q_y            = 1;
        rdoq_ctrls->fp_q_uv           = 1;
        rdoq_ctrls->satd_factor       = (uint8_t)~0;
        rdoq_ctrls->early_exit_th     = 0;
#endif
        rdoq_ctrls->skip_uv      = 1;
        rdoq_ctrls->dct_dct_only = 1;
        rdoq_ctrls->eob_th       = (uint8_t)~0;
        rdoq_ctrls->eob_fast_th  = (uint8_t)~0;
        break;
    case 4:
        rdoq_ctrls->enabled = 1;
#if OPT_RDOQ_BIS
        rdoq_ctrls->cut_off_num   = 60;
        rdoq_ctrls->cut_off_denum = 100;
#else
        rdoq_ctrls->cut_off_div       = 4;
        rdoq_ctrls->eob_fast_y_inter  = 0;
        rdoq_ctrls->eob_fast_y_intra  = 0;
        rdoq_ctrls->eob_fast_uv_inter = 0;
        rdoq_ctrls->eob_fast_uv_intra = 0;
        rdoq_ctrls->fp_q_y            = 1;
        rdoq_ctrls->fp_q_uv           = 1;
        rdoq_ctrls->satd_factor       = (uint8_t)~0;
        rdoq_ctrls->early_exit_th     = 0;
#endif
        rdoq_ctrls->skip_uv      = 1;
        rdoq_ctrls->dct_dct_only = 1;
        rdoq_ctrls->eob_th       = (uint8_t)~0;
        rdoq_ctrls->eob_fast_th  = 30;
        break;
    case 5:
        rdoq_ctrls->enabled = 1;
#if OPT_RDOQ_BIS
        rdoq_ctrls->cut_off_num   = 60;
        rdoq_ctrls->cut_off_denum = 100;
#else
        rdoq_ctrls->cut_off_div       = 4;
        rdoq_ctrls->eob_fast_y_inter  = 0;
        rdoq_ctrls->eob_fast_y_intra  = 0;
        rdoq_ctrls->eob_fast_uv_inter = 0;
        rdoq_ctrls->eob_fast_uv_intra = 0;
        rdoq_ctrls->fp_q_y            = 1;
        rdoq_ctrls->fp_q_uv           = 1;
        rdoq_ctrls->satd_factor       = (uint8_t)~0;
        rdoq_ctrls->early_exit_th     = 0;
#endif
        rdoq_ctrls->skip_uv      = 1;
        rdoq_ctrls->dct_dct_only = 1;
        rdoq_ctrls->eob_th       = 85;
        rdoq_ctrls->eob_fast_th  = 0;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_sq_txs_ctrls(ModeDecisionContext* ctx, uint8_t psq_txs_lvl) {
    NsqPsqTxsCtrls* nsq_psq_txs_ctrls = &ctx->nsq_psq_txs_ctrls;
    switch (psq_txs_lvl) {
    case 0:
        nsq_psq_txs_ctrls->enabled = 0;
        break;
    case 1:
        nsq_psq_txs_ctrls->enabled     = 1;
        nsq_psq_txs_ctrls->hv_to_sq_th = 1000;
        nsq_psq_txs_ctrls->h_to_v_th   = 100;
        break;
    case 2:
        nsq_psq_txs_ctrls->enabled     = 1;
        nsq_psq_txs_ctrls->hv_to_sq_th = 250;
        nsq_psq_txs_ctrls->h_to_v_th   = 25;
        break;
    case 3:
        nsq_psq_txs_ctrls->enabled     = 1;
        nsq_psq_txs_ctrls->hv_to_sq_th = 150;
        nsq_psq_txs_ctrls->h_to_v_th   = 15;
        break;
    default:
        assert(0);
        break;
    }
}

void svt_aom_set_txt_controls(ModeDecisionContext* ctx, uint8_t txt_level) {
    TxtControls* txt_ctrls = &ctx->txt_ctrls;

    switch (txt_level) {
    case 0:
        txt_ctrls->enabled = 0;

        txt_ctrls->txt_group_inter_lt_16x16    = 1;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = 1;

        txt_ctrls->txt_group_intra_lt_16x16    = 1;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = 1;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 0;
        txt_ctrls->satd_early_exit_th_inter    = 0;
        txt_ctrls->satd_th_q_weight            = 0;
        txt_ctrls->txt_rate_cost_th            = 0;
        break;
    case 1:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 0;
        txt_ctrls->satd_early_exit_th_inter    = 0;
        txt_ctrls->satd_th_q_weight            = 0;
        txt_ctrls->txt_rate_cost_th            = 0;
        break;
    case 2:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 20;
        txt_ctrls->satd_early_exit_th_inter    = 20;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 250;
        break;
    case 3:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = MAX_TX_TYPE_GROUP;

        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 15;
        txt_ctrls->satd_early_exit_th_inter    = 15;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 250;
        break;
    case 4:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = MAX_TX_TYPE_GROUP;

        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 10;
        txt_ctrls->satd_early_exit_th_inter    = 10;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 250;
        break;
    case 5:
        txt_ctrls->enabled                     = 1;
        txt_ctrls->txt_group_inter_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = MAX_TX_TYPE_GROUP;

        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 10;
        txt_ctrls->satd_early_exit_th_inter    = 5;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 100;

        break;
    case 6:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = 5;

        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 10;
        txt_ctrls->satd_early_exit_th_inter    = 5;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 100;
        break;
    case 7:
        // ref lvl_5
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = 5;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = 5;

        txt_ctrls->txt_group_intra_lt_16x16    = MAX_TX_TYPE_GROUP;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = MAX_TX_TYPE_GROUP;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 10;
        txt_ctrls->satd_early_exit_th_inter    = 5;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 100;
        break;
    case 8:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = 4;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = 3;

        txt_ctrls->txt_group_intra_lt_16x16    = 5;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = 4;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 10;
        txt_ctrls->satd_early_exit_th_inter    = 5;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 100;
        break;
    case 9:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = 3;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = 2;

        txt_ctrls->txt_group_intra_lt_16x16    = 4;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = 3;

        txt_ctrls->early_exit_dist_th       = 0;
        txt_ctrls->early_exit_coeff_th      = 0;
        txt_ctrls->satd_early_exit_th_intra = 10;
        txt_ctrls->satd_early_exit_th_inter = 5;
        txt_ctrls->satd_th_q_weight         = 1;
        txt_ctrls->txt_rate_cost_th         = 65;
        break;
    case 10:
        txt_ctrls->enabled = 1;

        txt_ctrls->txt_group_inter_lt_16x16    = 2;
        txt_ctrls->txt_group_inter_gt_eq_16x16 = 1;

        txt_ctrls->txt_group_intra_lt_16x16    = 3;
        txt_ctrls->txt_group_intra_gt_eq_16x16 = 2;
        txt_ctrls->early_exit_dist_th          = 0;
        txt_ctrls->early_exit_coeff_th         = 0;
        txt_ctrls->satd_early_exit_th_intra    = 10;
        txt_ctrls->satd_early_exit_th_inter    = 5;
        txt_ctrls->satd_th_q_weight            = 1;
        txt_ctrls->txt_rate_cost_th            = 50;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_interpolation_search_level_ctrls(ModeDecisionContext* ctx, uint8_t interpolation_search_level) {
    InterpolationSearchCtrls* ifs_ctrls = &ctx->ifs_ctrls;

    switch (interpolation_search_level) {
    case 0:
        ifs_ctrls->level                 = IFS_OFF;
        ifs_ctrls->subsampled_distortion = 0;
        ifs_ctrls->skip_sse_rd_model     = 0;
        break;
    case 1:
        ifs_ctrls->level                 = IFS_MDS0;
        ifs_ctrls->subsampled_distortion = 0;
        ifs_ctrls->skip_sse_rd_model     = 0;
        break;
    case 2:
        ifs_ctrls->level                 = IFS_MDS1;
        ifs_ctrls->subsampled_distortion = 0;
        ifs_ctrls->skip_sse_rd_model     = 0;
        break;
    case 3:
        ifs_ctrls->level                 = IFS_MDS2;
        ifs_ctrls->subsampled_distortion = 0;
        ifs_ctrls->skip_sse_rd_model     = 0;
        break;
    case 4:
        ifs_ctrls->level                 = IFS_MDS3;
        ifs_ctrls->subsampled_distortion = 0;
        ifs_ctrls->skip_sse_rd_model     = 0;
        break;
    case 5:
        ifs_ctrls->level                 = IFS_MDS3;
        ifs_ctrls->subsampled_distortion = 1;
        ifs_ctrls->skip_sse_rd_model     = 1;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_cand_reduction_ctrls(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t cand_reduction_level,
                                     const uint32_t picture_qp, uint32_t me_8x8_cost_variance,
                                     uint32_t me_64x64_distortion, uint8_t l0_was_skip, uint8_t l1_was_skip,
                                     uint8_t ref_skip_perc) {
    CandReductionCtrls* cand_reduction_ctrls = &ctx->cand_reduction_ctrls;
    const bool          is_lpd1              = ctx->lpd1_ctrls.pd1_level > REGULAR_PD1;
    const bool          is_not_last_layer    = !pcs->ppcs->is_highest_layer;
    const bool          use_flat_ipp         = pcs->scs->use_flat_ipp;
    switch (cand_reduction_level) {
    case 0:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 0;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 0;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 0;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 3;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 3;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 0;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled = 0;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = 0;

        break;

    case 1:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 1;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 0;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 0;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 3;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 3;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 0;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled = 0;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = 0;

        break;
    case 2:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 1;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 0;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 1;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 3;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 3;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 0;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled    = 1;
        cand_reduction_ctrls->cand_elimination_ctrls.dc_only_th = is_lpd1 ? (use_flat_ipp            ? 200
                                                                                 : is_not_last_layer ? 30
                                                                                                     : 200)
                                                                          : (is_not_last_layer ? 10 : 200);
        cand_reduction_ctrls->cand_elimination_ctrls.skip_dc_th = 0;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = 0;

        break;

    case 3:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 1;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 0;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 1;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 1;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 3;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 0;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled    = 1;
        cand_reduction_ctrls->cand_elimination_ctrls.dc_only_th = is_lpd1 ? (use_flat_ipp            ? 200
                                                                                 : is_not_last_layer ? 30
                                                                                                     : 200)
                                                                          : (is_not_last_layer ? 10 : 200);
        cand_reduction_ctrls->cand_elimination_ctrls.skip_dc_th = 0;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = 1;

        break;

    case 4:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 1;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 8;
        cand_reduction_ctrls->redundant_cand_ctrls.mag_th   = 64;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 1;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 1;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 1;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 0;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled    = 1;
        cand_reduction_ctrls->cand_elimination_ctrls.dc_only_th = is_lpd1 ? (use_flat_ipp            ? 600
                                                                                 : is_not_last_layer ? 30
                                                                                                     : 600)
                                                                          : (is_not_last_layer ? 10 : 200);
        cand_reduction_ctrls->cand_elimination_ctrls.skip_dc_th = 0;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = 1;

        break;

    case 5:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 1;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 8;
        cand_reduction_ctrls->redundant_cand_ctrls.mag_th   = 64;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 1;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 1;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 1;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 1;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled    = 1;
        cand_reduction_ctrls->cand_elimination_ctrls.dc_only_th = is_lpd1 ? (use_flat_ipp            ? 800
                                                                                 : is_not_last_layer ? 30
                                                                                                     : 600)
                                                                          : (is_not_last_layer ? 10 : 200);
        cand_reduction_ctrls->cand_elimination_ctrls.skip_dc_th = is_not_last_layer ? 5 : 15;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = (pcs->ppcs->is_highest_layer ||
                                                           ((l0_was_skip && l1_was_skip && ref_skip_perc > 35) &&
                                                            me_8x8_cost_variance < (500 * picture_qp) &&
                                                            me_64x64_distortion < (500 * picture_qp)))
            ? 2
            : 1;
        break;

    case 6:
        // Filter INTRA reduction
        cand_reduction_ctrls->reduce_filter_intra = 1;
        // redundant_cand_level
        cand_reduction_ctrls->redundant_cand_ctrls.score_th = 8;
        cand_reduction_ctrls->redundant_cand_ctrls.mag_th   = 64;

        // use_neighbouring_mode
        cand_reduction_ctrls->use_neighbouring_mode_ctrls.enabled = 1;

        // near_count_ctrls
        cand_reduction_ctrls->near_count_ctrls.enabled         = 1;
        cand_reduction_ctrls->near_count_ctrls.near_count      = 0;
        cand_reduction_ctrls->near_count_ctrls.near_near_count = 1;

        // lpd1_mvp_best_me_list (LPD1 only signal)
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 1;

        // cand_elimination_ctrls
        cand_reduction_ctrls->cand_elimination_ctrls.enabled    = 1;
        cand_reduction_ctrls->cand_elimination_ctrls.dc_only_th = is_lpd1 ? (use_flat_ipp            ? 800
                                                                                 : is_not_last_layer ? 30
                                                                                                     : 600)
                                                                          : (is_not_last_layer ? 10 : 200);
        cand_reduction_ctrls->cand_elimination_ctrls.skip_dc_th = is_not_last_layer ? 5 : 15;

        // reduce_unipred_candidates
        cand_reduction_ctrls->reduce_unipred_candidates = (pcs->ppcs->is_highest_layer ||
                                                           ((l0_was_skip && l1_was_skip && ref_skip_perc > 35) &&
                                                            me_8x8_cost_variance < (500 * picture_qp) &&
                                                            me_64x64_distortion < (500 * picture_qp)))
            ? 2
            : 1;

        break;

    default:
        assert(0);
        break;
    }

    // lpd1_mvp_best_me_list can only use this feature when a single unipred ME candidate is selected,
    if (!(pcs->ppcs->ref_list0_count_try == 1 && pcs->ppcs->ref_list1_count_try == 1 &&
          pcs->ppcs->use_best_me_unipred_cand_only)) {
        cand_reduction_ctrls->lpd1_mvp_best_me_list = 0;
    }
}

uint8_t svt_aom_set_chroma_controls(ModeDecisionContext* ctx, uint8_t uv_level) {
    UvCtrls* uv_ctrls = ctx ? &ctx->uv_ctrls : NULL;
    uint8_t  uv_mode  = 0;

    switch (uv_level) {
    case 0:
        uv_mode = CHROMA_MODE_2;
        if (uv_ctrls) {
            uv_ctrls->enabled = 0;
        }
        break;
    case 1:
        uv_mode = CHROMA_MODE_0;
        if (uv_ctrls) {
            uv_ctrls->enabled                = 1;
            uv_ctrls->ind_uv_last_mds        = 0;
            uv_ctrls->inter_vs_intra_cost_th = 0;
            uv_ctrls->skip_ind_uv_if_only_dc = 0;
            uv_ctrls->uv_nic_scaling_num     = 16;
        }
        break;
    case 2:
        uv_mode = CHROMA_MODE_0;
        if (uv_ctrls) {
            uv_ctrls->enabled                = 1;
            uv_ctrls->ind_uv_last_mds        = 1;
            uv_ctrls->inter_vs_intra_cost_th = 0;
            uv_ctrls->skip_ind_uv_if_only_dc = 0;
            uv_ctrls->uv_nic_scaling_num     = 8;
        }
        break;
    case 3:
        uv_mode = CHROMA_MODE_0;
        if (uv_ctrls) {
            uv_ctrls->enabled                = 1;
            uv_ctrls->ind_uv_last_mds        = 1;
            uv_ctrls->inter_vs_intra_cost_th = 100;
            uv_ctrls->skip_ind_uv_if_only_dc = 0;
            uv_ctrls->uv_nic_scaling_num     = 1;
        }
        break;
    case 4:
        uv_mode = CHROMA_MODE_0;
        if (uv_ctrls) {
            uv_ctrls->enabled                = 1;
            uv_ctrls->ind_uv_last_mds        = 2;
            uv_ctrls->inter_vs_intra_cost_th = 100;
            uv_ctrls->skip_ind_uv_if_only_dc = 1;
            uv_ctrls->uv_nic_scaling_num     = 1;
        }
        break;
    case 5:
        uv_mode = CHROMA_MODE_1;
        if (uv_ctrls) {
            uv_ctrls->enabled = 1;
        }
        break;
    default:
        assert(0);
        break;
    }

    if (ctx) {
        uv_ctrls->uv_mode = uv_mode;
    }

    return uv_mode;
}

#define MAX_WARP_LVL 4

void svt_aom_set_wm_controls(ModeDecisionContext* ctx, uint8_t wm_level) {
    WmCtrls* wm_ctrls = &ctx->wm_ctrls;

    switch (wm_level) {
    case 0:
        wm_ctrls->enabled = 0;
        break;
    case 1:
        wm_ctrls->enabled                 = 1;
        wm_ctrls->use_wm_for_mvp          = 1;
        wm_ctrls->refinement_iterations   = 16;
        wm_ctrls->refine_diag             = 1;
        wm_ctrls->refine_level            = 0;
        wm_ctrls->lower_band_th           = 0;
        wm_ctrls->upper_band_th           = (uint16_t)~0;
        wm_ctrls->shut_approx_if_not_mds0 = 0;
        break;
    case 2:
        wm_ctrls->enabled                 = 1;
        wm_ctrls->use_wm_for_mvp          = 1;
        wm_ctrls->refinement_iterations   = 8;
        wm_ctrls->refine_diag             = 0;
        wm_ctrls->refine_level            = 1;
        wm_ctrls->lower_band_th           = 0;
        wm_ctrls->upper_band_th           = (uint16_t)~0;
        wm_ctrls->shut_approx_if_not_mds0 = 1;
        break;
    case 3:
        wm_ctrls->enabled                 = 1;
        wm_ctrls->use_wm_for_mvp          = 1;
        wm_ctrls->refinement_iterations   = 8;
        wm_ctrls->refine_diag             = 0;
        wm_ctrls->refine_level            = 1;
        wm_ctrls->lower_band_th           = 1 << 10;
        wm_ctrls->upper_band_th           = (uint16_t)~0;
        wm_ctrls->shut_approx_if_not_mds0 = 1;
        break;
    case MAX_WARP_LVL:
        wm_ctrls->enabled                 = 1;
        wm_ctrls->use_wm_for_mvp          = 0;
        wm_ctrls->refinement_iterations   = 0;
        wm_ctrls->refine_diag             = 0;
        wm_ctrls->refine_level            = 1;
        wm_ctrls->lower_band_th           = 0;
        wm_ctrls->upper_band_th           = (uint16_t)~0;
        wm_ctrls->shut_approx_if_not_mds0 = 0;
        break;
    default:
        assert(0);
        break;
    }
}

// Get the nic_level used for each preset (to be passed to setting function: svt_aom_set_nic_controls())
#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_nic_level_default(EncMode enc_mode, uint8_t is_base, uint8_t sc_class1) {
    uint8_t nic_level;
#if FTR_NIC_DREFI_NEW_LVL_DEFS
    if (enc_mode <= ENC_MR) {
        nic_level = is_base ? 1 : 2;
    } else if (enc_mode <= ENC_M0) {
        nic_level = is_base ? 2 : 4;
    } else if (enc_mode <= ENC_M1) {
        nic_level = is_base ? 4 : 5;
    } else if (enc_mode <= ENC_M3) {
        nic_level = is_base ? 5 : 6;
    } else if (!sc_class1 && enc_mode <= ENC_M4) {
        nic_level = 7;
#else
    if (enc_mode <= ENC_MR) {
        nic_level = is_base ? 1 : 2;
    } else if (enc_mode <= ENC_M0) {
        nic_level = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M1) {
        nic_level = is_base ? 3 : 5;
    } else if (enc_mode <= ENC_M3) {
        nic_level = is_base ? 5 : 6;
    } else if (!sc_class1 && enc_mode <= ENC_M4) {
        nic_level = 6;
#endif
    } else if (enc_mode <= ENC_M6) {
        nic_level = 8;
    } else if (enc_mode <= ENC_M7) {
        nic_level = is_base ? 8 : 9;
    } else if (enc_mode <= ENC_M8) {
        nic_level = 9;
    } else if (enc_mode <= ENC_M9) {
        nic_level = is_base ? 9 : 11;
    } else if (enc_mode <= ENC_M10) {
        nic_level = is_base ? 10 : 11;
    } else {
        nic_level = 11;
    }
    return nic_level;
}

uint8_t svt_aom_get_nic_level_rtc(EncMode enc_mode, bool use_flat_ipp) {
    uint8_t nic_level;

    if (use_flat_ipp) {
        if (enc_mode <= ENC_M7) {
            nic_level = 8;
        } else if (enc_mode <= ENC_M8) {
            nic_level = 9;
        } else {
            nic_level = 11;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            nic_level = 8;
        } else if (enc_mode <= ENC_M9) {
            nic_level = 9;
        } else {
            nic_level = 11;
        }
    }
    return nic_level;
}

uint8_t svt_aom_get_nic_level_allintra(EncMode enc_mode) {
    uint8_t nic_level;
#if FTR_NIC_DREFI_NEW_LVL_DEFS
    if (enc_mode <= ENC_M0) {
        nic_level = 2;
    } else if (enc_mode <= ENC_M2) {
        nic_level = 3;
    } else if (enc_mode <= ENC_M4) {
        nic_level = 5;
    } else if (enc_mode <= ENC_M5) {
        nic_level = 6;
    } else if (enc_mode <= ENC_M6) {
        nic_level = 7;
    } else if (enc_mode <= ENC_M8) {
        nic_level = 9;
    } else {
        nic_level = 11;
    }
#else
    if (enc_mode <= ENC_M0) {
        nic_level = 16; // level 2n
    } else if (enc_mode <= ENC_M2) {
        nic_level = 19; // level 2.3n
    } else if (enc_mode <= ENC_M4) {
        nic_level = 12; // level 5.25
    } else if (enc_mode <= ENC_M5) {
        nic_level = 13; // level 5.7
    } else if (enc_mode <= ENC_M6) {
        nic_level = 14; // level 5.9
    } else if (enc_mode <= ENC_M8) {
        nic_level = 9;
    } else {
        nic_level = 11;
    }
#endif

    return nic_level;
}
#else
uint8_t svt_aom_get_nic_level(SequenceControlSet* scs, EncMode enc_mode, uint8_t is_base, bool rtc_tune,
                              uint8_t sc_class1) {
    uint8_t    nic_level;
    const bool flat_rtc = rtc_tune && scs->use_flat_ipp;
    if (scs->allintra) {
        if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_MR) {
                nic_level = 2;
            } else if (enc_mode <= ENC_M1) {
                nic_level = 3;
            } else if (enc_mode <= ENC_M2) {
                nic_level = 4;
            } else if (enc_mode <= ENC_M4) {
                nic_level = 5;
            } else if (enc_mode <= ENC_M8) {
                nic_level = 7;
            } else if (enc_mode <= ENC_M9) {
                nic_level = 9;
            } else if (enc_mode <= ENC_M10) {
                nic_level = 10;
            } else {
                nic_level = 11;
            }
        } else {
            if (enc_mode <= ENC_MR) {
                nic_level = 2;
            } else if (enc_mode <= ENC_M0) {
                nic_level = 3;
            } else if (enc_mode <= ENC_M1) {
                nic_level = 4;
            } else if (enc_mode <= ENC_M4) {
                nic_level = 5;
            } else if (enc_mode <= ENC_M5) {
                nic_level = 6;
            } else if (enc_mode <= ENC_M8) {
                nic_level = 7;
            } else if (enc_mode <= ENC_M9) {
                nic_level = 8;
            } else if (enc_mode <= ENC_M10) {
                nic_level = 10;
            } else {
                nic_level = 11;
            }
        }
    } else if (flat_rtc) { // FLAT-RTC
        if (enc_mode <= ENC_M7) {
            nic_level = 8;
        } else if (enc_mode <= ENC_M8) {
            nic_level = 9;
        } else {
            nic_level = 11;
        }
    } else if (rtc_tune) { // Non-FLAT RTC
        if (enc_mode <= ENC_M7) {
            nic_level = 8;
        } else if (enc_mode <= ENC_M9) {
            nic_level = 9;
        } else {
            nic_level = 11;
        }
    } else if (enc_mode <= ENC_MR) {
        nic_level = is_base ? 1 : 2;
    } else if (enc_mode <= ENC_M0) {
        nic_level = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M1) {
        nic_level = is_base ? 3 : 5;
    } else if (enc_mode <= ENC_M3) {
        nic_level = is_base ? 5 : 6;
    } else if (!sc_class1 && enc_mode <= ENC_M4) {
        nic_level = 6;
    } else if (enc_mode <= ENC_M6) {
        nic_level = 8;
    } else if (enc_mode <= ENC_M7) {
        nic_level = is_base ? 8 : 9;
    } else if (enc_mode <= ENC_M8) {
        nic_level = 9;
    } else if (enc_mode <= ENC_M9) {
        nic_level = is_base ? 9 : 11;
    } else if (enc_mode <= ENC_M10) {
        nic_level = is_base ? 10 : 11;
    } else {
        nic_level = 11;
    }
    return nic_level;
}
#endif
/*
* Set the NIC scaling and pruning controls.
*
* This function is used in MD to set the NIC controls and is also used at memory allocation
* to allocate the candidate buffers.  Therefore, the function returns the nic_scaling_level
* (index into MD_STAGE_NICS_SCAL_NUM array).
*
* When called at memory allocation, there is no context (it is passed as NULL) so the signals
* are not set.
*/
uint8_t svt_aom_set_nic_controls(ModeDecisionContext* ctx, uint8_t nic_level) {
    NicPruningCtrls* nic_pruning_ctrls = ctx ? &ctx->nic_ctrls.pruning_ctrls : NULL;
    uint8_t          nic_scaling_level = 0;
    uint8_t          md_staging_mode   = MD_STAGING_MODE_0;

    switch (nic_level) {
    case 0: // MAX NIC scaling; no pruning
        // NIC scaling level
        nic_scaling_level = 0;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = (uint64_t)~0;
            nic_pruning_ctrls->mds2_class_th = (uint64_t)~0;
            nic_pruning_ctrls->mds3_class_th = (uint64_t)~0;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = (uint64_t)~0;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = (uint64_t)~0;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = (uint64_t)~0;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = (uint64_t)~0;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;

    case 1:
        // NIC scaling level
        nic_scaling_level = 0;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = (uint64_t)~0;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 4;

            nic_pruning_ctrls->mds3_class_th = 25;
            nic_pruning_ctrls->mds3_band_cnt = 4;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = (uint64_t)~0;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = (uint64_t)~0;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 50;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 50;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        md_staging_mode = MD_STAGING_MODE_1;
#else
        md_staging_mode = MD_STAGING_MODE_2;
#endif
        break;

    case 2:
        // NIC scaling level
        nic_scaling_level = 1;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = (uint64_t)~0;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 4;

            nic_pruning_ctrls->mds3_class_th = 25;
            nic_pruning_ctrls->mds3_band_cnt = 8;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 500;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 30;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 30;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        md_staging_mode = MD_STAGING_MODE_1;
#else
        md_staging_mode = MD_STAGING_MODE_2;
#endif
        break;

    case 3:
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        // NIC scaling level
        nic_scaling_level = 3;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = (uint64_t)~0;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 4;

            nic_pruning_ctrls->mds3_class_th = 25;
            nic_pruning_ctrls->mds3_band_cnt = 8;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 500;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 30;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 25;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
#else
        // NIC scaling level
        nic_scaling_level = 3;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 500;
            nic_pruning_ctrls->mds1_band_cnt = 3;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 8;

            nic_pruning_ctrls->mds3_class_th = 20;
            nic_pruning_ctrls->mds3_band_cnt = 12;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 20;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
        md_staging_mode = MD_STAGING_MODE_2;
        break;
#endif
    case 4:
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        // NIC scaling level
        nic_scaling_level = 3;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 500;
            nic_pruning_ctrls->mds1_band_cnt = 3;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 8;

            nic_pruning_ctrls->mds3_class_th = 20;
            nic_pruning_ctrls->mds3_band_cnt = 12;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 20;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
#else
        // NIC scaling level
        nic_scaling_level = 4;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 500;
            nic_pruning_ctrls->mds1_band_cnt = 3;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 8;

            nic_pruning_ctrls->mds3_class_th = 20;
            nic_pruning_ctrls->mds3_band_cnt = 12;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 20;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
        md_staging_mode = MD_STAGING_MODE_2;
        break;
#endif
    case 5:
        // NIC scaling level
        nic_scaling_level = 6;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 300;
            nic_pruning_ctrls->mds1_band_cnt = 4;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 15;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_cand_base_th        = 20;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        md_staging_mode = MD_STAGING_MODE_1;
#else
        md_staging_mode = MD_STAGING_MODE_2;
#endif
        break;
    case 6:
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        // NIC scaling level
        nic_scaling_level = 6;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 200;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 10;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 5;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 15;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
#else
        // NIC scaling level
        nic_scaling_level = 9;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 300;
            nic_pruning_ctrls->mds1_band_cnt = 4;

            nic_pruning_ctrls->mds2_class_th = 25;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 15;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 2;
            nic_pruning_ctrls->mds2_cand_base_th        = 20;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 0;
            nic_pruning_ctrls->mds2_relative_dev_th     = 0;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = (uint8_t)~0;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
#endif
    case 7:
#if FTR_NIC_DREFI_NEW_LVL_DEFS
        // NIC scaling level
        nic_scaling_level = 8;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 200;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 10;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 5;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 15;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
#else
        // NIC scaling level
        nic_scaling_level = 10;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 200;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 10;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 5;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 0;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1200;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 15;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 15;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
#endif
    case 8:
        // NIC scaling level
        nic_scaling_level = 13;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 200;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 10;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 5;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 1;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 300;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 300;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 3;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 3;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;

    case 9:
        // NIC scaling level
        nic_scaling_level = 14;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 200;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 10;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 5;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 1;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 100;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 100;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 1;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 1;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;

    case 10:
        // NIC scaling level
        nic_scaling_level = 15;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 150;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 5;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 5;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 1;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 1;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 1;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 1;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;

    case 11:
        // NIC scaling level
        nic_scaling_level = 15;

        if (nic_pruning_ctrls) {
            // Class pruning settings
            nic_pruning_ctrls->mds1_class_th = 75;
            nic_pruning_ctrls->mds1_band_cnt = 16;

            nic_pruning_ctrls->mds2_class_th = 0;
            nic_pruning_ctrls->mds2_band_cnt = 10;

            nic_pruning_ctrls->mds3_class_th = 0;
            nic_pruning_ctrls->mds3_band_cnt = 16;

            nic_pruning_ctrls->enable_skipping_mds1 = 1;

            // Cand pruning settings
            nic_pruning_ctrls->mds1_cand_base_th_intra  = 1;
            nic_pruning_ctrls->mds1_cand_base_th_inter  = 1;
            nic_pruning_ctrls->mds1_cand_th_rank_factor = 3;
            nic_pruning_ctrls->mds2_cand_base_th        = 1;
            nic_pruning_ctrls->mds2_cand_th_rank_factor = 1;
            nic_pruning_ctrls->mds2_relative_dev_th     = 5;
            nic_pruning_ctrls->mds3_cand_base_th        = 1;

            nic_pruning_ctrls->merge_inter_cands_mult = 4;
        }
        md_staging_mode = MD_STAGING_MODE_1;
        break;
    default:
        assert(0);
        break;
    }

    if (ctx) {
        NicScalingCtrls* nic_scaling_ctrls    = &ctx->nic_ctrls.scaling_ctrls;
        nic_scaling_ctrls->stage1_scaling_num = MD_STAGE_NICS_SCAL_NUM[nic_scaling_level][MD_STAGE_1];
        nic_scaling_ctrls->stage2_scaling_num = MD_STAGE_NICS_SCAL_NUM[nic_scaling_level][MD_STAGE_2];
        nic_scaling_ctrls->stage3_scaling_num = MD_STAGE_NICS_SCAL_NUM[nic_scaling_level][MD_STAGE_3];
        ctx->nic_ctrls.md_staging_mode        = md_staging_mode;
    }
    // return NIC scaling level that can be used for memory allocation
    return nic_scaling_level;
}

void svt_aom_set_nsq_geom_ctrls(ModeDecisionContext* ctx, uint8_t nsq_geom_level, uint8_t* allow_HVA_HVB,
                                uint8_t* allow_HV4, uint8_t* min_nsq_bsize) {
    NsqGeomCtrls  nsq_geom_ctrls_struct = {0};
    NsqGeomCtrls* nsq_geom_ctrls        = &nsq_geom_ctrls_struct;
    switch (nsq_geom_level) {
    case 0:
        nsq_geom_ctrls->enabled            = 0;
        nsq_geom_ctrls->min_nsq_block_size = 0;
        nsq_geom_ctrls->allow_HV4          = 0;
        nsq_geom_ctrls->allow_HVA_HVB      = 0;
        break;

    case 1:
        nsq_geom_ctrls->enabled            = 1;
        nsq_geom_ctrls->min_nsq_block_size = 0;
        nsq_geom_ctrls->allow_HV4          = 1;
        nsq_geom_ctrls->allow_HVA_HVB      = 1;
        break;

    case 2:
        nsq_geom_ctrls->enabled            = 1;
        nsq_geom_ctrls->min_nsq_block_size = 0;
        nsq_geom_ctrls->allow_HV4          = 1;
        nsq_geom_ctrls->allow_HVA_HVB      = 0;
        break;
    case 3:
        nsq_geom_ctrls->enabled            = 1;
        nsq_geom_ctrls->min_nsq_block_size = 8;
        nsq_geom_ctrls->allow_HV4          = 0;
        nsq_geom_ctrls->allow_HVA_HVB      = 0;
        break;
    case 4:
        nsq_geom_ctrls->enabled            = 1;
        nsq_geom_ctrls->min_nsq_block_size = 16;
        nsq_geom_ctrls->allow_HV4          = 0;
        nsq_geom_ctrls->allow_HVA_HVB      = 0;
        break;
    default:
        assert(0);
        break;
    }

    if (allow_HVA_HVB) {
        *allow_HVA_HVB = nsq_geom_ctrls->allow_HVA_HVB;
    }
    if (allow_HV4) {
        *allow_HV4 = nsq_geom_ctrls->allow_HV4;
    }
    if (min_nsq_bsize) {
        *min_nsq_bsize = nsq_geom_ctrls->min_nsq_block_size;
    }
    if (ctx) {
        memcpy(&ctx->nsq_geom_ctrls, nsq_geom_ctrls, sizeof(NsqGeomCtrls));
    }
}

static void set_nsq_search_ctrls(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t nsq_search_level) {
    bool me_dist_mod;
    // Whether or not to modulate the nsq_search_level using me-distortion
    if (pcs->slice_type == I_SLICE) {
        me_dist_mod = 0;
    } else {
        if (pcs->enc_mode <= ENC_MR) {
            me_dist_mod = 0;
        } else {
            me_dist_mod = 1;
        }
    }
    NsqSearchCtrls* nsq_search_ctrls = &ctx->nsq_search_ctrls;
    if (pcs->mimic_only_tx_4x4) {
        nsq_search_level = 0;
    } else if (me_dist_mod && nsq_search_level) {
        uint32_t dist_64, dist_32, dist_16, dist_8, me_8x8_cost_variance;
        if (pcs->scs->super_block_size == 64) {
            dist_64              = pcs->ppcs->me_64x64_distortion[ctx->sb_index];
            dist_32              = pcs->ppcs->me_32x32_distortion[ctx->sb_index];
            dist_16              = pcs->ppcs->me_16x16_distortion[ctx->sb_index];
            dist_8               = pcs->ppcs->me_8x8_distortion[ctx->sb_index];
            me_8x8_cost_variance = pcs->ppcs->me_8x8_cost_variance[ctx->sb_index];
        } else {
            get_sb128_me_data(pcs, ctx, &dist_64, &dist_32, &dist_16, &dist_8, &me_8x8_cost_variance);
        }

        int error_per_sample = 3;
        if (dist_8 <= (pcs->scs->super_block_size * pcs->scs->super_block_size * error_per_sample) &&
            me_8x8_cost_variance <= 10000) {
            nsq_search_level = MIN(nsq_search_level + 1, 19);
        }
    }

    switch (nsq_search_level) {
    case 0:
        nsq_search_ctrls->enabled                   = 0;
        nsq_search_ctrls->sq_weight                 = (uint32_t)~0;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 0;
        nsq_search_ctrls->lower_depth_split_cost_th = 0;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 0;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 0;
        nsq_search_ctrls->component_multiple_th     = 0;
        nsq_search_ctrls->hv_weight                 = (uint32_t)~0;
        break;

    case 1:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 105;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 0;
        nsq_search_ctrls->lower_depth_split_cost_th = 0;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 0;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 0;
        nsq_search_ctrls->component_multiple_th     = 0;
        nsq_search_ctrls->hv_weight                 = 115;
        break;

    case 2:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 105;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 150;
        nsq_search_ctrls->lower_depth_split_cost_th = 3;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 0;
        nsq_search_ctrls->component_multiple_th     = 0;
        nsq_search_ctrls->hv_weight                 = 115;
        break;

    case 3:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 105;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 100;
        nsq_search_ctrls->lower_depth_split_cost_th = 3;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 0;
        nsq_search_ctrls->hv_weight                 = 115;
        break;

    case 4:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 100;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 100;
        nsq_search_ctrls->lower_depth_split_cost_th = 3;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 80;
        nsq_search_ctrls->hv_weight                 = 115;
        break;

    case 5:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 100;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 100;
        nsq_search_ctrls->lower_depth_split_cost_th = 5;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 80;
        nsq_search_ctrls->hv_weight                 = 110;
        break;
    case 6:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 100;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 100;
        nsq_search_ctrls->lower_depth_split_cost_th = 5;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 80;
        nsq_search_ctrls->hv_weight                 = 100;
        break;

    case 7:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 80;
        nsq_search_ctrls->lower_depth_split_cost_th = 5;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 0;
        nsq_search_ctrls->non_HV_split_rate_th      = 0;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 80;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 8:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 80;
        nsq_search_ctrls->lower_depth_split_cost_th = 5;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 30;
        nsq_search_ctrls->non_HV_split_rate_th      = 20;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 80;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 9:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 80;
        nsq_search_ctrls->lower_depth_split_cost_th = 5;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 40;
        nsq_search_ctrls->non_HV_split_rate_th      = 30;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 60;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 10:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 60;
        nsq_search_ctrls->lower_depth_split_cost_th = 10;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 40;
        nsq_search_ctrls->non_HV_split_rate_th      = 30;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 60;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 11:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 60;
        nsq_search_ctrls->lower_depth_split_cost_th = 10;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 50;
        nsq_search_ctrls->non_HV_split_rate_th      = 30;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 40;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 12:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 60;
        nsq_search_ctrls->lower_depth_split_cost_th = 10;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 50;
        nsq_search_ctrls->non_HV_split_rate_th      = 30;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 20;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 13:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 60;
        nsq_search_ctrls->lower_depth_split_cost_th = 10;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 60;
        nsq_search_ctrls->non_HV_split_rate_th      = 40;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 20;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 14:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 95;
        nsq_search_ctrls->max_part0_to_part1_dev    = 5;
        nsq_search_ctrls->nsq_split_cost_th         = 50;
        nsq_search_ctrls->lower_depth_split_cost_th = 10;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 60;
        nsq_search_ctrls->non_HV_split_rate_th      = 40;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 20;
        nsq_search_ctrls->hv_weight                 = 100;
        break;
    case 15:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 90;
        nsq_search_ctrls->max_part0_to_part1_dev    = 20;
        nsq_search_ctrls->nsq_split_cost_th         = 40;
        nsq_search_ctrls->lower_depth_split_cost_th = 20;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 60;
        nsq_search_ctrls->non_HV_split_rate_th      = 50;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 15;
        nsq_search_ctrls->hv_weight                 = 75;
        break;
    case 16:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 90;
        nsq_search_ctrls->max_part0_to_part1_dev    = 50;
        nsq_search_ctrls->nsq_split_cost_th         = 40;
        nsq_search_ctrls->lower_depth_split_cost_th = 20;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 70;
        nsq_search_ctrls->non_HV_split_rate_th      = 60;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 15;
        nsq_search_ctrls->hv_weight                 = 75;
        break;
    case 17:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 90;
        nsq_search_ctrls->max_part0_to_part1_dev    = 50;
        nsq_search_ctrls->nsq_split_cost_th         = 40;
        nsq_search_ctrls->lower_depth_split_cost_th = 20;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 70;
        nsq_search_ctrls->non_HV_split_rate_th      = 60;
        nsq_search_ctrls->rate_th_offset_lte16      = 15;
        nsq_search_ctrls->psq_txs_lvl               = 1;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 10;
        nsq_search_ctrls->hv_weight                 = 75;
        break;
    case 18:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 90;
        nsq_search_ctrls->max_part0_to_part1_dev    = 75;
        nsq_search_ctrls->nsq_split_cost_th         = 40;
        nsq_search_ctrls->lower_depth_split_cost_th = 20;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 80;
        nsq_search_ctrls->non_HV_split_rate_th      = 70;
        nsq_search_ctrls->rate_th_offset_lte16      = 15;
        nsq_search_ctrls->psq_txs_lvl               = 1;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 5;
        nsq_search_ctrls->hv_weight                 = 75;
        break;
    case 19:
        nsq_search_ctrls->enabled                   = 1;
        nsq_search_ctrls->sq_weight                 = 90;
        nsq_search_ctrls->max_part0_to_part1_dev    = 80;
        nsq_search_ctrls->nsq_split_cost_th         = 35;
        nsq_search_ctrls->lower_depth_split_cost_th = 20;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 85;
        nsq_search_ctrls->non_HV_split_rate_th      = 70;
        nsq_search_ctrls->rate_th_offset_lte16      = 15;
        nsq_search_ctrls->psq_txs_lvl               = 1;
        nsq_search_ctrls->sub_depth_block_lvl       = 1;
        nsq_search_ctrls->component_multiple_th     = 5;
        nsq_search_ctrls->hv_weight                 = 75;
        break;
    default:
        assert(0);
        break;
    }
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.nsq_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->scs->static_config.qp);
    nsq_search_ctrls->component_multiple_th = DIVIDE_AND_ROUND(nsq_search_ctrls->component_multiple_th * q_weight,
                                                               q_weight_denom);
    nsq_search_ctrls->nsq_split_cost_th     = DIVIDE_AND_ROUND(nsq_search_ctrls->nsq_split_cost_th * q_weight,
                                                           q_weight_denom);
    int max_part0_to_part1_dev_offset       = 5;
    max_part0_to_part1_dev_offset = DIVIDE_AND_ROUND(max_part0_to_part1_dev_offset * q_weight, q_weight_denom);
    nsq_search_ctrls->max_part0_to_part1_dev = MAX(
        (int)((int)nsq_search_ctrls->max_part0_to_part1_dev - max_part0_to_part1_dev_offset), 0);
    if (ctx->pd_pass == PD_PASS_0) {
        nsq_search_ctrls->sq_weight                 = 90;
        nsq_search_ctrls->max_part0_to_part1_dev    = 0;
        nsq_search_ctrls->nsq_split_cost_th         = 60;
        nsq_search_ctrls->lower_depth_split_cost_th = 10;
        nsq_search_ctrls->H_vs_V_split_rate_th      = 60;
        nsq_search_ctrls->non_HV_split_rate_th      = 60;
        nsq_search_ctrls->rate_th_offset_lte16      = 10;
        nsq_search_ctrls->psq_txs_lvl               = 0;
        nsq_search_ctrls->sub_depth_block_lvl       = 0;
        nsq_search_ctrls->component_multiple_th     = 10;
        nsq_search_ctrls->hv_weight                 = 90;
    }

    set_sq_txs_ctrls(ctx, ctx->nsq_search_ctrls.psq_txs_lvl);
}
#if TUNE_STILL_IMAGE
void svt_aom_get_intra_mode_levels_default(EncMode enc_mode, bool is_islice, bool is_base, int transition_present,
                                           uint32_t* intra_level_ptr, uint32_t* dist_based_ang_intra_level_ptr) {
    uint32_t intra_level;
    uint32_t dist_based_ang_intra_level;

    if (enc_mode <= ENC_MR) {
        intra_level                = is_base ? 1 : 2;
        dist_based_ang_intra_level = 0;
    } else if (enc_mode <= ENC_M2) {
        intra_level                = is_base ? 1 : 2;
        dist_based_ang_intra_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M5) {
        intra_level                = is_base ? 1 : 6;
        dist_based_ang_intra_level = is_islice ? 0 : 2;
    } else if (enc_mode <= ENC_M7) {
        intra_level                = is_base ? 2 : 6;
        dist_based_ang_intra_level = is_islice ? 0 : 2;
    } else if (enc_mode <= ENC_M11) {
        intra_level                = (is_islice || transition_present == 1) ? 4 : 6;
        dist_based_ang_intra_level = is_islice ? 0 : 2;
    } else {
#if OPT_PER_BLK_INTRA
        intra_level = MAX_INTRA_LEVEL - 1;
#else
        intra_level = 8;
#endif
        dist_based_ang_intra_level = 0;
    }

    *intra_level_ptr                = intra_level;
    *dist_based_ang_intra_level_ptr = dist_based_ang_intra_level;
}

void svt_aom_get_intra_mode_levels_rtc(EncMode enc_mode, bool is_islice, bool sc_class1, int transition_present,
                                       bool flat_rtc_tune, uint32_t* intra_level_ptr,
                                       uint32_t* dist_based_ang_intra_level_ptr) {
    uint32_t intra_level;
    uint32_t dist_based_ang_intra_level;

    if ((!flat_rtc_tune && enc_mode <= ENC_M7) || (flat_rtc_tune && enc_mode <= ENC_M9)) {
        intra_level                = (is_islice || transition_present == 1) ? 1 : 6;
        dist_based_ang_intra_level = 1;
    } else if (enc_mode <= ENC_M9 || sc_class1) {
        intra_level                = (is_islice || transition_present == 1) ? 4 : 6;
        dist_based_ang_intra_level = 1;
    } else if (enc_mode <= ENC_M11) {
        intra_level                = (is_islice || transition_present == 1) ? 4 : 6;
        dist_based_ang_intra_level = 2;
    } else {
#if OPT_PER_BLK_INTRA
        intra_level = MAX_INTRA_LEVEL - 1;
#else
        intra_level = 8;
#endif
        dist_based_ang_intra_level = 0;
    }

    *intra_level_ptr                = intra_level;
    *dist_based_ang_intra_level_ptr = dist_based_ang_intra_level;
}

void svt_aom_get_intra_mode_levels_allintra(EncMode enc_mode, uint32_t* intra_level_ptr,
                                            uint32_t* dist_based_ang_intra_level_ptr) {
    uint32_t intra_level;
    uint32_t dist_based_ang_intra_level;

    if (enc_mode <= ENC_M4) {
        intra_level                = 1;
        dist_based_ang_intra_level = 0;
    } else if (enc_mode <= ENC_M5) {
        intra_level                = 2;
        dist_based_ang_intra_level = 0;
    } else if (enc_mode <= ENC_M7) {
        intra_level                = 7;
        dist_based_ang_intra_level = 0;
    } else {
        intra_level                = 8;
        dist_based_ang_intra_level = 0;
    }

    *intra_level_ptr                = intra_level;
    *dist_based_ang_intra_level_ptr = dist_based_ang_intra_level;
}
#else
void svt_aom_get_intra_mode_levels(EncMode enc_mode, uint32_t input_resolution, bool allintra, bool rtc_tune,
                                   bool is_islice, bool is_base, bool sc_class1, int transition_present,
                                   bool flat_rtc_tune, uint32_t* intra_level_ptr,
                                   uint32_t* dist_based_ang_intra_level_ptr) {
    uint32_t intra_level;
    uint32_t dist_based_ang_intra_level;

    if (allintra) {
        if (input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_M4) {
                intra_level                = 1;
                dist_based_ang_intra_level = 0;
            } else if (enc_mode <= ENC_M6) {
                intra_level                = 2;
                dist_based_ang_intra_level = 0;
            } else if (enc_mode <= ENC_M7) {
                intra_level                = 5;
                dist_based_ang_intra_level = 0;
            } else if (enc_mode <= ENC_M10) {
                intra_level                = 7;
                dist_based_ang_intra_level = 0;
            } else {
                intra_level                = 8;
                dist_based_ang_intra_level = 0;
            }
        } else {
            if (enc_mode <= ENC_M5) {
                intra_level                = 1;
                dist_based_ang_intra_level = 0;
            } else if (enc_mode <= ENC_M7) {
                intra_level                = 2;
                dist_based_ang_intra_level = 0;
            } else if (enc_mode <= ENC_M10) {
                intra_level                = 7;
                dist_based_ang_intra_level = 0;
            } else {
                intra_level                = 8;
                dist_based_ang_intra_level = 0;
            }
        }
    } else if (rtc_tune) {
        if ((!flat_rtc_tune && enc_mode <= ENC_M7) || (flat_rtc_tune && enc_mode <= ENC_M9)) {
            intra_level                = (is_islice || transition_present == 1) ? 1 : 6;
            dist_based_ang_intra_level = 1;
        } else if (enc_mode <= ENC_M9 || sc_class1) {
            intra_level                = (is_islice || transition_present == 1) ? 4 : 6;
            dist_based_ang_intra_level = 1;
        } else if (enc_mode <= ENC_M11) {
            intra_level                = (is_islice || transition_present == 1) ? 4 : 6;
            dist_based_ang_intra_level = 2;
        } else {
            intra_level                = 8;
            dist_based_ang_intra_level = 0;
        }
    } else if (enc_mode <= ENC_MR) {
        intra_level                = is_base ? 1 : 2;
        dist_based_ang_intra_level = 0;
    } else if (enc_mode <= ENC_M2) {
        intra_level                = is_base ? 1 : 2;
        dist_based_ang_intra_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M5) {
        intra_level                = is_base ? 1 : 6;
        dist_based_ang_intra_level = is_islice ? 0 : 2;
    } else if (enc_mode <= ENC_M7) {
        intra_level                = is_base ? 2 : 6;
        dist_based_ang_intra_level = is_islice ? 0 : 2;
    } else if (enc_mode <= ENC_M11) {
        intra_level                = (is_islice || transition_present == 1) ? 4 : 6;
        dist_based_ang_intra_level = is_islice ? 0 : 2;
    } else {
        intra_level                = 8;
        dist_based_ang_intra_level = 0;
    }

    *intra_level_ptr                = intra_level;
    *dist_based_ang_intra_level_ptr = dist_based_ang_intra_level;
}
#endif

static void set_inter_intra_ctrls(ModeDecisionContext* ctx, uint8_t inter_intra_level) {
    InterIntraCompCtrls* ii_ctrls = &ctx->inter_intra_comp_ctrls;

    switch (inter_intra_level) {
    case 0:
        ii_ctrls->enabled        = 0;
        ii_ctrls->use_rd_model   = 0;
        ii_ctrls->wedge_mode_sq  = 0;
        ii_ctrls->wedge_mode_nsq = 0;
        break;
    case 1:
        ii_ctrls->enabled        = 1;
        ii_ctrls->use_rd_model   = 1;
        ii_ctrls->wedge_mode_sq  = 1;
        ii_ctrls->wedge_mode_nsq = 1;
        break;
    case 2:
        ii_ctrls->enabled        = 1;
        ii_ctrls->use_rd_model   = 0;
        ii_ctrls->wedge_mode_sq  = 0;
        ii_ctrls->wedge_mode_nsq = 2;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_lpd0_ctrls(ModeDecisionContext* ctx, uint8_t lpd0_lvl) {
    Lpd0Ctrls* ctrls = &ctx->lpd0_ctrls;
    // Light-PD0 only compatible with 8bit MD
    if (ctx->hbd_md) {
        ctx->lpd0_ctrls.pd0_level = REGULAR_PD0;
        return;
    }
    switch (lpd0_lvl) {
    case 0:
        ctrls->pd0_level = REGULAR_PD0; // Light-PD0 path not used
        break;
    case 1:
        ctrls->pd0_level                     = LPD0_LVL_0;
        ctrls->use_lpd0_detector[LPD0_LVL_0] = 0;
        break;
    case 2:
        ctrls->pd0_level                     = LPD0_LVL_1;
        ctrls->use_lpd0_detector[LPD0_LVL_0] = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1] = 0;
        break;
    case 3:
        ctrls->pd0_level                     = LPD0_LVL_2;
        ctrls->use_lpd0_detector[LPD0_LVL_0] = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1] = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_2] = 0;
        break;
    case 4:
        ctrls->pd0_level                           = LPD0_LVL_3;
        ctrls->use_lpd0_detector[LPD0_LVL_0]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_2]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_3]       = 1;
        ctrls->use_ref_info[LPD0_LVL_3]            = 2;
        ctrls->me_8x8_cost_variance_th[LPD0_LVL_3] = 250000;
        ctrls->edge_dist_th[LPD0_LVL_3]            = 16384;
        ctrls->neigh_me_dist_shift[LPD0_LVL_3]     = 3;
        break;
    case 5:
        ctrls->pd0_level                           = LPD0_LVL_4;
        ctrls->use_lpd0_detector[LPD0_LVL_0]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_2]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_3]       = 1;
        ctrls->use_ref_info[LPD0_LVL_3]            = 2;
        ctrls->me_8x8_cost_variance_th[LPD0_LVL_3] = 250000;
        ctrls->edge_dist_th[LPD0_LVL_3]            = 16384;
        ctrls->neigh_me_dist_shift[LPD0_LVL_3]     = 3;

        // Set LPD0_LVL_3 controls
        ctrls->use_lpd0_detector[LPD0_LVL_4]       = 1;
        ctrls->use_ref_info[LPD0_LVL_4]            = 1;
        ctrls->me_8x8_cost_variance_th[LPD0_LVL_4] = 250000 >> 1;
        ctrls->edge_dist_th[LPD0_LVL_4]            = 16384;
        ctrls->neigh_me_dist_shift[LPD0_LVL_4]     = 2;
        break;
    case 6:
        ctrls->pd0_level                     = LPD0_LVL_4;
        ctrls->use_lpd0_detector[LPD0_LVL_0] = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1] = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_2] = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_3] = 0;
        // Set LPD0_LVL_3 controls
        ctrls->use_lpd0_detector[LPD0_LVL_4]       = 1;
        ctrls->use_ref_info[LPD0_LVL_4]            = 0;
        ctrls->me_8x8_cost_variance_th[LPD0_LVL_4] = 500000;
        ctrls->edge_dist_th[LPD0_LVL_4]            = 16384;
        ctrls->neigh_me_dist_shift[LPD0_LVL_4]     = 2;
        break;
    case 7:
        ctrls->pd0_level = VERY_LIGHT_PD0;

        ctrls->use_lpd0_detector[LPD0_LVL_0]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_2]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_3]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_4]       = 1;
        ctrls->use_ref_info[LPD0_LVL_4]            = 0;
        ctrls->me_8x8_cost_variance_th[LPD0_LVL_4] = 500000 << 1;
        ctrls->edge_dist_th[LPD0_LVL_4]            = (uint32_t)~0;
        ctrls->neigh_me_dist_shift[LPD0_LVL_4]     = (uint16_t)~0;

        // Set VERY_LIGHT_PD0 controls
        ctrls->use_lpd0_detector[VERY_LIGHT_PD0]       = 1;
        ctrls->use_ref_info[VERY_LIGHT_PD0]            = 1;
        ctrls->me_8x8_cost_variance_th[VERY_LIGHT_PD0] = 250000;
        ctrls->edge_dist_th[VERY_LIGHT_PD0]            = 16384;
        ctrls->neigh_me_dist_shift[VERY_LIGHT_PD0]     = 2;
        break;
    case 8:
        ctrls->pd0_level = VERY_LIGHT_PD0;

        ctrls->use_lpd0_detector[LPD0_LVL_0]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_1]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_2]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_3]       = 0;
        ctrls->use_lpd0_detector[LPD0_LVL_4]       = 1;
        ctrls->use_ref_info[LPD0_LVL_4]            = 0;
        ctrls->me_8x8_cost_variance_th[LPD0_LVL_4] = 500000 << 1;
        ctrls->edge_dist_th[LPD0_LVL_4]            = (uint32_t)~0;
        ctrls->neigh_me_dist_shift[LPD0_LVL_4]     = (uint16_t)~0;

        // Set VERY_LIGHT_PD0 controls
        ctrls->use_lpd0_detector[VERY_LIGHT_PD0]       = 1;
        ctrls->use_ref_info[VERY_LIGHT_PD0]            = 2;
        ctrls->me_8x8_cost_variance_th[VERY_LIGHT_PD0] = 250000;
        ctrls->edge_dist_th[VERY_LIGHT_PD0]            = 16384;
        ctrls->neigh_me_dist_shift[VERY_LIGHT_PD0]     = 2;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_lpd1_ctrls(ModeDecisionContext* ctx, uint8_t lpd1_lvl) {
    Lpd1Ctrls* ctrls = &ctx->lpd1_ctrls;
    switch (lpd1_lvl) {
    case 0:
        ctrls->pd1_level = REGULAR_PD1; // Light-PD1 path not used
        break;
    case 1:
        ctrls->pd1_level = LPD1_LVL_0;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 25;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 40 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 0;
        ctrls->max_mv_length[LPD1_LVL_0]           = 300;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = 250000 >> 3;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = 1024;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = 1;
        break;
    case 2:
        ctrls->pd1_level = LPD1_LVL_0;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 25;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 40 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 16;
        ctrls->max_mv_length[LPD1_LVL_0]           = 300;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = 250000 >> 3;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = 1024;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = 1;
        break;
    case 3:
        ctrls->pd1_level = LPD1_LVL_0;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 35;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 100 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 16;
        ctrls->max_mv_length[LPD1_LVL_0]           = 900;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = 750000 >> 3;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = 16384;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = 3;
        break;
    case 4:
        ctrls->pd1_level = LPD1_LVL_1;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 256 << 4;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 512 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 32;
        ctrls->max_mv_length[LPD1_LVL_0]           = 2048;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = 500000;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = 16384;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = 3;

        // Set LPD1 level 1 controls
        ctrls->use_lpd1_detector[LPD1_LVL_1]       = 1;
        ctrls->use_ref_info[LPD1_LVL_1]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_1]            = 256 << 1;
        ctrls->cost_th_rate[LPD1_LVL_1]            = 6000 + 125 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_1]             = 32;
        ctrls->max_mv_length[LPD1_LVL_1]           = 1600;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_1] = 500000 >> 3;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_1]   = 16384;
        ctrls->skip_pd0_me_shift[LPD1_LVL_1]       = 3;
        break;
    case 5:
        ctrls->pd1_level = LPD1_LVL_3;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 512;
        ctrls->max_mv_length[LPD1_LVL_0]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = 16384 * 7;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = 5;

        // Set LPD1 level 1 controls
        ctrls->use_lpd1_detector[LPD1_LVL_1]       = 1;
        ctrls->use_ref_info[LPD1_LVL_1]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_1]            = 256 << 8;
        ctrls->cost_th_rate[LPD1_LVL_1]            = 6000 + 4096 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_1]             = 256;
        ctrls->max_mv_length[LPD1_LVL_1]           = 2048 * 8;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_1] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_1]   = 16384 * 6;
        ctrls->skip_pd0_me_shift[LPD1_LVL_1]       = 5;

        // Set LPD1 level 2 controls
        ctrls->use_lpd1_detector[LPD1_LVL_2]       = 1;
        ctrls->use_ref_info[LPD1_LVL_2]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_2]            = 256 << 8;
        ctrls->cost_th_rate[LPD1_LVL_2]            = 6000 + 4096 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_2]             = 164;
        ctrls->max_mv_length[LPD1_LVL_2]           = 2048 * 8;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_2] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_2]   = 16384 * 6;
        ctrls->skip_pd0_me_shift[LPD1_LVL_2]       = 5;

        // Set LPD1 level 3 controls
        ctrls->use_lpd1_detector[LPD1_LVL_3]       = 1;
        ctrls->use_ref_info[LPD1_LVL_3]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_3]            = 256 << 8;
        ctrls->cost_th_rate[LPD1_LVL_3]            = 6000 + 4096 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_3]             = 128;
        ctrls->max_mv_length[LPD1_LVL_3]           = 2048 * 8;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_3] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_3]   = 16384 * 6;
        ctrls->skip_pd0_me_shift[LPD1_LVL_3]       = 5;
        break;
    case 6:
        ctrls->pd1_level = LPD1_LVL_4;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 512;
        ctrls->max_mv_length[LPD1_LVL_0]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = 16384 * 7;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = 5;

        // Set LPD1 level 1 controls
        ctrls->use_lpd1_detector[LPD1_LVL_1]       = 1;
        ctrls->use_ref_info[LPD1_LVL_1]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_1]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_1]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_1]             = 512;
        ctrls->max_mv_length[LPD1_LVL_1]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_1] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_1]   = 16384 * 7;
        ctrls->skip_pd0_me_shift[LPD1_LVL_1]       = 5;

        // Set LPD1 level 2 controls
        ctrls->use_lpd1_detector[LPD1_LVL_2]       = 1;
        ctrls->use_ref_info[LPD1_LVL_2]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_2]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_2]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_2]             = 256;
        ctrls->max_mv_length[LPD1_LVL_2]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_2] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_2]   = 16384 * 7;
        ctrls->skip_pd0_me_shift[LPD1_LVL_2]       = 5;

        // Set LPD1 level 3 controls
        ctrls->use_lpd1_detector[LPD1_LVL_3]       = 1;
        ctrls->use_ref_info[LPD1_LVL_3]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_3]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_3]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_3]             = 164;
        ctrls->max_mv_length[LPD1_LVL_3]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_3] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_3]   = 16384 * 7;
        ctrls->skip_pd0_me_shift[LPD1_LVL_3]       = 5;

        // Set LPD1 level 4 controls
        ctrls->use_lpd1_detector[LPD1_LVL_4]       = 1;
        ctrls->use_ref_info[LPD1_LVL_4]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_4]            = 256 << 4;
        ctrls->cost_th_rate[LPD1_LVL_4]            = 6000 + 1024 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_4]             = 128;
        ctrls->max_mv_length[LPD1_LVL_4]           = 2048;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_4] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_4]   = 16384 * 2;
        ctrls->skip_pd0_me_shift[LPD1_LVL_4]       = 3;
        break;
    case 7:
        ctrls->pd1_level = LPD1_LVL_5;

        // Set LPD1 level 0 controls
        ctrls->use_lpd1_detector[LPD1_LVL_0]       = 1;
        ctrls->use_ref_info[LPD1_LVL_0]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_0]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_0]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_0]             = 256;
        ctrls->max_mv_length[LPD1_LVL_0]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_0] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_0]   = (uint32_t)~0;
        ctrls->skip_pd0_me_shift[LPD1_LVL_0]       = (uint16_t)~0;

        // Set LPD1 level 1 controls
        ctrls->use_lpd1_detector[LPD1_LVL_1]       = 1;
        ctrls->use_ref_info[LPD1_LVL_1]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_1]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_1]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_1]             = 128;
        ctrls->max_mv_length[LPD1_LVL_1]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_1] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_1]   = (uint32_t)~0;
        ctrls->skip_pd0_me_shift[LPD1_LVL_1]       = (uint16_t)~0;

        // Set LPD1 level 2 controls
        ctrls->use_lpd1_detector[LPD1_LVL_2]       = 1;
        ctrls->use_ref_info[LPD1_LVL_2]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_2]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_2]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_2]             = 96;
        ctrls->max_mv_length[LPD1_LVL_2]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_2] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_2]   = (uint32_t)~0;
        ctrls->skip_pd0_me_shift[LPD1_LVL_2]       = (uint16_t)~0;

        // Set LPD1 level 3 controls
        ctrls->use_lpd1_detector[LPD1_LVL_3]       = 1;
        ctrls->use_ref_info[LPD1_LVL_3]            = 0;
        ctrls->cost_th_dist[LPD1_LVL_3]            = 256 << 10;
        ctrls->cost_th_rate[LPD1_LVL_3]            = 6000 + 8192 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_3]             = 96;
        ctrls->max_mv_length[LPD1_LVL_3]           = 2048 * 16;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_3] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_3]   = (uint32_t)~0;
        ctrls->skip_pd0_me_shift[LPD1_LVL_3]       = (uint16_t)~0;

        // Set LPD1 level 4 controls
        ctrls->use_lpd1_detector[LPD1_LVL_4]       = 1;
        ctrls->use_ref_info[LPD1_LVL_4]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_4]            = 256 << 8;
        ctrls->cost_th_rate[LPD1_LVL_4]            = 6000 + 4096 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_4]             = 64;
        ctrls->max_mv_length[LPD1_LVL_4]           = 2048 * 8;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_4] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_4]   = 16384 * 6;
        ctrls->skip_pd0_me_shift[LPD1_LVL_4]       = 5;

        // Set LPD1 level 5 controls
        ctrls->use_lpd1_detector[LPD1_LVL_5]       = 1;
        ctrls->use_ref_info[LPD1_LVL_5]            = 1;
        ctrls->cost_th_dist[LPD1_LVL_5]            = 256 << 8;
        ctrls->cost_th_rate[LPD1_LVL_5]            = 6000 + 4096 * 500;
        ctrls->nz_coeff_th[LPD1_LVL_5]             = 64;
        ctrls->max_mv_length[LPD1_LVL_5]           = 2048 * 8;
        ctrls->me_8x8_cost_variance_th[LPD1_LVL_5] = (uint32_t)~0;
        ctrls->skip_pd0_edge_dist_th[LPD1_LVL_5]   = 16384 * 6;
        ctrls->skip_pd0_me_shift[LPD1_LVL_5]       = 5;
        break;
    default:
        assert(0);
        break;
    }
}

/*
 * Generate per-SB/per-PD MD settings
 */
void svt_aom_set_bipred3x3_controls(ModeDecisionContext* ctx, uint8_t bipred3x3_injection) {
    Bipred3x3Controls* bipred3x3_ctrls = &ctx->bipred3x3_ctrls;

    switch (bipred3x3_injection) {
    case 0:
        bipred3x3_ctrls->enabled = 0;
        break;
    case 1:
        bipred3x3_ctrls->enabled       = 1;
        bipred3x3_ctrls->search_diag   = 1;
        bipred3x3_ctrls->use_best_list = 0;
        bipred3x3_ctrls->use_l0_l1_dev = (uint8_t)~0;
        break;
    case 2:
        bipred3x3_ctrls->enabled       = 1;
        bipred3x3_ctrls->search_diag   = 0;
        bipred3x3_ctrls->use_best_list = 0;
        bipred3x3_ctrls->use_l0_l1_dev = (uint8_t)~0;
        break;
    case 3:
        bipred3x3_ctrls->enabled       = 1;
        bipred3x3_ctrls->search_diag   = 0;
        bipred3x3_ctrls->use_best_list = 1;
        bipred3x3_ctrls->use_l0_l1_dev = (uint8_t)~0;
        break;
    case 4:
        bipred3x3_ctrls->enabled       = 1;
        bipred3x3_ctrls->search_diag   = 0;
        bipred3x3_ctrls->use_best_list = 1;
        bipred3x3_ctrls->use_l0_l1_dev = 20;
        break;
    default:
        assert(0);
        break;
    }
}

void svt_aom_set_dist_based_ref_pruning_controls(ModeDecisionContext* ctx, uint8_t dist_based_ref_pruning_level) {
    RefPruningControls* ref_pruning_ctrls = &ctx->ref_pruning_ctrls;

    switch (dist_based_ref_pruning_level) {
    case 0:
        ref_pruning_ctrls->enabled = 0;
        break;
    case 1:
        ref_pruning_ctrls->enabled = 1;

        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = (uint32_t)~0;

        ref_pruning_ctrls->use_tpl_info_offset      = 0;
        ref_pruning_ctrls->check_closest_multiplier = 0;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;

        break;

    case 2:
        ref_pruning_ctrls->enabled = 1;

        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 150;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = (uint32_t)~0;

        ref_pruning_ctrls->use_tpl_info_offset      = 0;
        ref_pruning_ctrls->check_closest_multiplier = 0;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;

        break;
    case 3:
        ref_pruning_ctrls->enabled = 1;

        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = 30;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = 30;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 60;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = 30;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = 30;

        ref_pruning_ctrls->use_tpl_info_offset      = 0;
        ref_pruning_ctrls->check_closest_multiplier = 0;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;

        break;
    case 4:
        ref_pruning_ctrls->enabled = 1;

        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = 30;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = 30;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = 30;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 30;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = 30;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = 30;

        ref_pruning_ctrls->use_tpl_info_offset      = 20;
        ref_pruning_ctrls->check_closest_multiplier = 1;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;

        break;
    case 5:
        ref_pruning_ctrls->enabled                              = 1;
        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = 30;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = 0;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = 30;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 30;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = 30;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = 0;

        ref_pruning_ctrls->use_tpl_info_offset      = 20;
        ref_pruning_ctrls->check_closest_multiplier = 1;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;

        break;
    case 6:
        ref_pruning_ctrls->enabled                              = 1;
        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = 30;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = 0;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = 30;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = 30;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 30;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = 30;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = 0;

        ref_pruning_ctrls->use_tpl_info_offset      = 20;
        ref_pruning_ctrls->check_closest_multiplier = 1;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;
        break;
    case 7:
        ref_pruning_ctrls->enabled                              = 1;
        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = 10;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = 0;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = 10;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = 10;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = 10;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 10;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = 10;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = 10;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = 0;

        ref_pruning_ctrls->use_tpl_info_offset      = 20;
        ref_pruning_ctrls->check_closest_multiplier = 1;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;

        break;
    case 8:
        ref_pruning_ctrls->enabled = 1;

        ref_pruning_ctrls->max_dev_to_best[PA_ME_GROUP]         = 0;
        ref_pruning_ctrls->max_dev_to_best[UNI_3x3_GROUP]       = 0;
        ref_pruning_ctrls->max_dev_to_best[BI_3x3_GROUP]        = 0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEW_NEAR_GROUP] = 0;
        ref_pruning_ctrls->max_dev_to_best[NRST_NEAR_GROUP]     = 0;
        ref_pruning_ctrls->max_dev_to_best[PRED_ME_GROUP]       = 0;
        ref_pruning_ctrls->max_dev_to_best[GLOBAL_GROUP]        = (uint32_t)~0;
        ref_pruning_ctrls->max_dev_to_best[WARP_GROUP]          = 0;
        ref_pruning_ctrls->max_dev_to_best[OBMC_GROUP]          = 0;
        ref_pruning_ctrls->max_dev_to_best[INTER_INTRA_GROUP]   = 0;
        ref_pruning_ctrls->max_dev_to_best[INTER_COMP_GROUP]    = 0;

        ref_pruning_ctrls->use_tpl_info_offset      = 20;
        ref_pruning_ctrls->check_closest_multiplier = 1;

        ref_pruning_ctrls->closest_refs[PA_ME_GROUP]         = 1;
        ref_pruning_ctrls->closest_refs[UNI_3x3_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[BI_3x3_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEW_NEAR_GROUP] = 1;
        ref_pruning_ctrls->closest_refs[NRST_NEAR_GROUP]     = 1;
        ref_pruning_ctrls->closest_refs[PRED_ME_GROUP]       = 1;
        ref_pruning_ctrls->closest_refs[GLOBAL_GROUP]        = 1;
        ref_pruning_ctrls->closest_refs[WARP_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[OBMC_GROUP]          = 1;
        ref_pruning_ctrls->closest_refs[INTER_INTRA_GROUP]   = 1;
        ref_pruning_ctrls->closest_refs[INTER_COMP_GROUP]    = 1;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_txs_controls(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t txs_level) {
    TxsControls* txs_ctrls = &ctx->txs_ctrls;

    if (pcs->mimic_only_tx_4x4) {
        txs_level = 1;
    }

    switch (txs_level) {
    case 0:
        txs_ctrls->enabled = 0;
        break;
    case 1:
        txs_ctrls->enabled                   = 1;
        txs_ctrls->prev_depth_coeff_exit_th  = 1;
        txs_ctrls->intra_class_max_depth_sq  = 2;
        txs_ctrls->intra_class_max_depth_nsq = 2;
        txs_ctrls->inter_class_max_depth_sq  = 2;
        txs_ctrls->inter_class_max_depth_nsq = 2;
        txs_ctrls->depth1_txt_group_offset   = 0;
        txs_ctrls->depth2_txt_group_offset   = 0;
        txs_ctrls->quadrant_th_sf            = 0;
        break;
    case 2:
        txs_ctrls->enabled                   = 1;
        txs_ctrls->prev_depth_coeff_exit_th  = 1;
        txs_ctrls->intra_class_max_depth_sq  = 2;
        txs_ctrls->intra_class_max_depth_nsq = 2;
        txs_ctrls->inter_class_max_depth_sq  = 1;
        txs_ctrls->inter_class_max_depth_nsq = 1;
        txs_ctrls->depth1_txt_group_offset   = 0;
        txs_ctrls->depth2_txt_group_offset   = 0;
        txs_ctrls->quadrant_th_sf            = 0;
        break;
    case 3:
        txs_ctrls->enabled                   = 1;
        txs_ctrls->prev_depth_coeff_exit_th  = 1;
        txs_ctrls->intra_class_max_depth_sq  = 1;
        txs_ctrls->intra_class_max_depth_nsq = 0;
        txs_ctrls->inter_class_max_depth_sq  = 1;
        txs_ctrls->inter_class_max_depth_nsq = 0;
        txs_ctrls->depth1_txt_group_offset   = 3;
        txs_ctrls->depth2_txt_group_offset   = 3;
        txs_ctrls->quadrant_th_sf            = 0;
        break;
    case 4:
        txs_ctrls->enabled                   = 1;
        txs_ctrls->prev_depth_coeff_exit_th  = 2;
        txs_ctrls->intra_class_max_depth_sq  = 1;
        txs_ctrls->intra_class_max_depth_nsq = 1;
        txs_ctrls->inter_class_max_depth_sq  = 0;
        txs_ctrls->inter_class_max_depth_nsq = 0;
        txs_ctrls->depth1_txt_group_offset   = 4;
        txs_ctrls->depth2_txt_group_offset   = 4;
        txs_ctrls->quadrant_th_sf            = 100;
        break;
    case 5:
        txs_ctrls->enabled                   = 1;
        txs_ctrls->prev_depth_coeff_exit_th  = 100;
        txs_ctrls->intra_class_max_depth_sq  = 1;
        txs_ctrls->intra_class_max_depth_nsq = 1;
        txs_ctrls->inter_class_max_depth_sq  = 0;
        txs_ctrls->inter_class_max_depth_nsq = 0;
        txs_ctrls->depth1_txt_group_offset   = 4;
        txs_ctrls->depth2_txt_group_offset   = 4;
        txs_ctrls->quadrant_th_sf            = 100;
        break;
    default:
        txs_ctrls->enabled = 0;
        break;
    }
}

static void set_filter_intra_ctrls(ModeDecisionContext* ctx, uint8_t fi_lvl) {
    FilterIntraCtrls* filter_intra_ctrls = &ctx->filter_intra_ctrls;

    switch (fi_lvl) {
    case 0:
        filter_intra_ctrls->enabled = 0;
        break;
    case 1:
        filter_intra_ctrls->enabled               = 1;
        filter_intra_ctrls->max_filter_intra_mode = FILTER_PAETH_PRED;
        break;
    case 2:
        filter_intra_ctrls->enabled               = 1;
        filter_intra_ctrls->max_filter_intra_mode = FILTER_DC_PRED;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_spatial_sse_full_loop_level(ModeDecisionContext* ctx, uint8_t spatial_sse_full_loop_level) {
    SpatialSSECtrls* spatial_sse_ctrls = &ctx->spatial_sse_ctrls;

    switch (spatial_sse_full_loop_level) {
    case 0:
        spatial_sse_ctrls->level = SSSE_OFF;
        break;
    case 1:
        spatial_sse_ctrls->level = SSSE_MDS1;
        break;
    case 2:
        spatial_sse_ctrls->level = SSSE_MDS2;
        break;
    case 3:
        spatial_sse_ctrls->level = SSSE_MDS3;
        break;
    default:
        assert(0);
        break;
    }
}

// Compute a qp-aware threshold based on the variance of the SB, used to apply selectively INTRA at PD0
static uint64_t compute_intra_pd0_th(SequenceControlSet* scs, ModeDecisionContext* ctx) {
    uint32_t fast_lambda      = ctx->hbd_md ? ctx->fast_lambda_md[EB_10_BIT_MD] : ctx->fast_lambda_md[EB_8_BIT_MD];
    uint32_t sb_size          = scs->super_block_size * scs->super_block_size;
    uint64_t cost_th_rate     = 1 << 13;
    uint64_t use_intra_pd0_th = 0;

    use_intra_pd0_th = RDCOST(fast_lambda, cost_th_rate, sb_size * 6);
    return use_intra_pd0_th;
}

// Compute a qp-aware threshold based on the variance of the SB, used to apply selectively subres
static uint64_t compute_subres_th(SequenceControlSet* scs, ModeDecisionContext* ctx) {
    uint32_t fast_lambda   = ctx->hbd_md ? ctx->fast_lambda_md[EB_10_BIT_MD] : ctx->fast_lambda_md[EB_8_BIT_MD];
    uint32_t sb_size       = scs->super_block_size * scs->super_block_size;
    uint64_t cost_th_rate  = 1 << 13;
    uint64_t use_subres_th = 0;

    use_subres_th = RDCOST(fast_lambda, cost_th_rate, sb_size * 6);
    return use_subres_th;
}

static void set_lpd1_tx_ctrls(ModeDecisionContext* ctx, uint8_t lpd1_tx_level) {
    Lpd1TxCtrls* ctrls = &ctx->lpd1_tx_ctrls;

    switch (lpd1_tx_level) {
    case 0:
        ctrls->zero_y_coeff_exit            = 0;
        ctrls->skip_nrst_nrst_luma_tx       = 0;
        ctrls->skip_tx_th                   = 0;
        ctrls->use_uv_shortcuts_on_y_coeffs = 0;

        ctrls->use_mds3_shortcuts_th = 0;
        ctrls->use_neighbour_info    = 0;
        break;
    case 1:
        ctrls->zero_y_coeff_exit            = 1;
        ctrls->chroma_detector_level        = 1;
        ctrls->skip_nrst_nrst_luma_tx       = 0;
        ctrls->skip_tx_th                   = 0;
        ctrls->use_uv_shortcuts_on_y_coeffs = 1;

        ctrls->use_mds3_shortcuts_th = 30;
        ctrls->use_neighbour_info    = 0;
        break;
    case 2:
        ctrls->zero_y_coeff_exit            = 1;
        ctrls->chroma_detector_level        = 1;
        ctrls->skip_nrst_nrst_luma_tx       = 1;
        ctrls->skip_tx_th                   = 30;
        ctrls->use_uv_shortcuts_on_y_coeffs = 1;

        ctrls->use_mds3_shortcuts_th = 30;
        ctrls->use_neighbour_info    = 0;
        break;
    case 3:
        ctrls->zero_y_coeff_exit            = 1;
        ctrls->chroma_detector_level        = 2;
        ctrls->skip_nrst_nrst_luma_tx       = 1;
        ctrls->skip_tx_th                   = 30;
        ctrls->use_uv_shortcuts_on_y_coeffs = 1;

        ctrls->use_mds3_shortcuts_th = 30;
        ctrls->use_neighbour_info    = 0;
        break;
    case 4:
        ctrls->zero_y_coeff_exit            = 1;
        ctrls->chroma_detector_level        = 2;
        ctrls->skip_nrst_nrst_luma_tx       = 1;
        ctrls->skip_tx_th                   = 30;
        ctrls->use_uv_shortcuts_on_y_coeffs = 1;

        ctrls->use_mds3_shortcuts_th = 60;
        ctrls->use_neighbour_info    = 1;
        break;
    case 5:
        ctrls->zero_y_coeff_exit            = 1;
        ctrls->chroma_detector_level        = 2;
        ctrls->skip_nrst_nrst_luma_tx       = 1;
        ctrls->skip_tx_th                   = 60;
        ctrls->use_uv_shortcuts_on_y_coeffs = 1;

        ctrls->use_mds3_shortcuts_th = 60;
        ctrls->use_neighbour_info    = 2;
        break;
    case 6:
        ctrls->zero_y_coeff_exit            = 1;
        ctrls->chroma_detector_level        = 2;
        ctrls->skip_nrst_nrst_luma_tx       = 1;
        ctrls->skip_tx_th                   = 100;
        ctrls->use_uv_shortcuts_on_y_coeffs = 1;

        ctrls->use_mds3_shortcuts_th = 150;
        ctrls->use_neighbour_info    = 2;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_cfl_ctrls(ModeDecisionContext* ctx, uint8_t cfl_level) {
    CflCtrls* ctrls = &ctx->cfl_ctrls;

    switch (cfl_level) {
    case 0:
        ctrls->enabled = 0;
        break;
    case 1:
        ctrls->enabled = 1;
        ctrls->itr_th  = 2;
        ctrls->cplx_th = 0;
        break;
    case 2:
        ctrls->enabled = 1;
        ctrls->itr_th  = 1;
        ctrls->cplx_th = 0;
        break;
    case 3:
        ctrls->enabled = 1;
        ctrls->itr_th  = 1;
        ctrls->cplx_th = 5;
        break;
    case 4:
        ctrls->enabled = 1;
        ctrls->itr_th  = 1;
        ctrls->cplx_th = 10;
        break;
    case 5:
        ctrls->enabled = 1;
        ctrls->itr_th  = 1;
        ctrls->cplx_th = 55;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_rate_est_ctrls(ModeDecisionContext* ctx, uint8_t rate_est_level) {
    MdRateEstCtrls* ctrls = &ctx->rate_est_ctrls;

    switch (rate_est_level) {
    case 0:
        ctrls->update_skip_ctx_dc_sign_ctx = 0;
        ctrls->update_skip_coeff_ctx       = 0;
        ctrls->coeff_rate_est_lvl          = 0;
        ctrls->lpd0_qp_offset              = 8;
        ctrls->pd0_fast_coeff_est_level    = 2;
        break;
    case 1:
        ctrls->update_skip_ctx_dc_sign_ctx = 1;
        ctrls->update_skip_coeff_ctx       = 1;
        ctrls->coeff_rate_est_lvl          = 1;
        ctrls->lpd0_qp_offset              = 0;
        ctrls->pd0_fast_coeff_est_level    = 1;
        break;
    case 2:
        ctrls->update_skip_ctx_dc_sign_ctx = 1;
        ctrls->update_skip_coeff_ctx       = 0;
        ctrls->coeff_rate_est_lvl          = 1;
        ctrls->lpd0_qp_offset              = 0;
        ctrls->pd0_fast_coeff_est_level    = 2;
        break;
    case 3:
        ctrls->update_skip_ctx_dc_sign_ctx = 1;
        ctrls->update_skip_coeff_ctx       = 0;
        ctrls->coeff_rate_est_lvl          = 2;
        ctrls->lpd0_qp_offset              = 0;
        ctrls->pd0_fast_coeff_est_level    = 2;
        break;
    case 4:
        ctrls->update_skip_ctx_dc_sign_ctx = 0;
        ctrls->update_skip_coeff_ctx       = 0;
        ctrls->coeff_rate_est_lvl          = 2;
        ctrls->lpd0_qp_offset              = 0;
        ctrls->pd0_fast_coeff_est_level    = 2;
        break;
    default:
        assert(0);
        break;
    }
}

/*
Loop over TPL blocks in the SB to update intra information.  Return 1 if the stats for the SB are valid; else return 0.

sb_ang_intra_count: Number of TPL blocks in the SB where the best_mode was an angular intra mode
sb_max_intra: The maximum intra mode selected by any TPL block in the SB (DC_PRED is lowest, PAETH_PRED is highest)
sb_intra_count: Number of TPL blocks in the SB where the best_mode was an intra mode
*/
static bool get_sb_tpl_intra_stats(PictureControlSet* pcs, ModeDecisionContext* ctx, int* sb_ang_intra_count,
                                   PredictionMode* sb_max_intra, int* sb_intra_count) {
    PictureParentControlSet* ppcs = pcs->ppcs;

    // Check that TPL data is available and that INTRA was tested in TPL.
    // Note that not all INTRA modes may be tested in TPL.
    if (ppcs->tpl_ctrls.enable && ppcs->tpl_src_data_ready &&
        (pcs->temporal_layer_index < ppcs->hierarchical_levels || !ppcs->tpl_ctrls.disable_intra_pred_nref)) {
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
        SbGeom* sb_geom = &ppcs->sb_geom[ctx->sb_index];
        int     sb_cols = MAX(1, sb_geom->width / tpl_blk_size);
        int     sb_rows = MAX(1, sb_geom->height / tpl_blk_size);

        int            ang_intra_count = 0;
        PredictionMode max_intra       = DC_PRED;
        int            intra_count     = 0;

        // Loop over all blocks in the SB
        for (int i = 0; i < sb_rows; i++) {
            TplSrcStats* tpl_src_stats_buffer =
                &ppcs->pa_me_data->tpl_src_stats_buffer[((mb_origin_y >> 4) + (i * tpl_blk_step)) * aligned16_width +
                                                        (mb_origin_x >> 4)];
            for (int j = 0; j < sb_cols; j++) {
                if (is_intra_mode(tpl_src_stats_buffer->best_mode)) {
                    max_intra = MAX(max_intra, tpl_src_stats_buffer->best_mode);
                    intra_count++;
                }

                if (av1_is_directional_mode(tpl_src_stats_buffer->best_mode)) {
                    ang_intra_count++;
                }
                tpl_src_stats_buffer += tpl_blk_step;
            }
        }

        *sb_ang_intra_count = ang_intra_count;
        *sb_max_intra       = max_intra;
        *sb_intra_count     = intra_count;
        return 1;
    }
    return 0;
}

static void set_intra_ctrls(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t intra_level,
                            uint8_t dist_based_ang_intra_level) {
    IntraCtrls*              ctrls = &ctx->intra_ctrls;
    PictureParentControlSet* ppcs  = pcs->ppcs;

    // If intra is disallowed at the pic level, must disallow at SB level
    if (pcs->skip_intra) {
        intra_level = 0;
    }

    assert(IMPLIES(pcs->slice_type == I_SLICE, intra_level > 0));

    switch (intra_level) {
    case 0:
        ctrls->enable_intra          = 0;
        ctrls->intra_mode_end        = DC_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        break;
    case 1:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = PAETH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        break;
    case 2:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = PAETH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        // Only use TPL info if all INTRA modes are tested
        if (ppcs->tpl_ctrls.enable && ppcs->tpl_ctrls.intra_mode_end == PAETH_PRED) {
            int            sb_ang_intra_count;
            PredictionMode sb_max_intra;
            int            sb_intra_count;
            if (get_sb_tpl_intra_stats(pcs, ctx, &sb_ang_intra_count, &sb_max_intra, &sb_intra_count)) {
                // if SB has angluar modes, use full search
                if (sb_ang_intra_count) {
                    ctrls->angular_pred_level = 1;
                } else {
                    ctrls->angular_pred_level = 3;
                }
            }
        }
        break;
    case 3:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = PAETH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        // Only use TPL info if all INTRA modes are tested
        if (ppcs->tpl_ctrls.enable && ppcs->tpl_ctrls.intra_mode_end == PAETH_PRED) {
            int            sb_ang_intra_count;
            PredictionMode sb_max_intra;
            int            sb_intra_count;
            if (get_sb_tpl_intra_stats(pcs, ctx, &sb_ang_intra_count, &sb_max_intra, &sb_intra_count)) {
                // if SB has angluar modes, use full search
                if (sb_ang_intra_count) {
                    ctrls->angular_pred_level = 1;
                } else {
                    ctrls->angular_pred_level = 3;
                }
                ctrls->intra_mode_end = sb_max_intra;
            }
        }
        break;
    case 4:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = SMOOTH_H_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        // Only use TPL info if all INTRA modes are tested
        if (ppcs->tpl_ctrls.enable && ppcs->tpl_ctrls.intra_mode_end == PAETH_PRED) {
            int            sb_ang_intra_count;
            PredictionMode sb_max_intra;
            int            sb_intra_count;
            if (get_sb_tpl_intra_stats(pcs, ctx, &sb_ang_intra_count, &sb_max_intra, &sb_intra_count)) {
                int tpl_blk_size = ppcs->tpl_ctrls.dispenser_search_level == 0 ? 16
                    : ppcs->tpl_ctrls.dispenser_search_level == 1              ? 32
                                                                               : 64;
                // Get actual SB width (for cases of incomplete SBs)
                SbGeom* sb_geom = &ppcs->sb_geom[ctx->sb_index];
                int     sb_cols = sb_geom->width / tpl_blk_size;
                int     sb_rows = sb_geom->height / tpl_blk_size;

                // if more than a quarter of SB is angular, use safe intra_level
                if (sb_ang_intra_count > ((sb_rows * sb_cols) >> 2)) {
                    ctrls->angular_pred_level = 1;
                } else if (sb_ang_intra_count > 2) {
                    ctrls->angular_pred_level = 2;
                } else {
                    ctrls->angular_pred_level = 4;
                }
                ctrls->intra_mode_end = sb_max_intra;
            }
        }
        break;
    case 5:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = SMOOTH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        // There is no check that all TPL modes are checked, so should only use info about
        // general intra modes, not the specific intra mode selected or whether it's angular
        if (ppcs->tpl_ctrls.enable) {
            int            sb_ang_intra_count;
            PredictionMode sb_max_intra;
            int            sb_intra_count;
            if (get_sb_tpl_intra_stats(pcs, ctx, &sb_ang_intra_count, &sb_max_intra, &sb_intra_count)) {
                if (sb_intra_count > 0) {
                    ctrls->angular_pred_level = 2;
                    ctrls->intra_mode_end     = PAETH_PRED;
                } else if (pcs->ref_intra_percentage < 30) {
                    ctrls->angular_pred_level = 0;
                    ctrls->intra_mode_end     = SMOOTH_PRED;
                }
            }
        }
        break;
    case 6:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = SMOOTH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 0;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 0;
#endif
        break;
#if OPT_PER_BLK_INTRA
    case 7:
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = SMOOTH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 1;
        ctrls->prune_using_edge_info = 0;
        break;
    case 8:
#else
    case 7:
#endif
        ctrls->enable_intra          = 1;
        ctrls->intra_mode_end        = SMOOTH_PRED;
        ctrls->angular_pred_level    = angular_pred_level[intra_level];
        ctrls->prune_using_best_mode = 1;
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_edge_info = 1;
#endif
        break;
    case MAX_INTRA_LEVEL - 1:
        ctrls->enable_intra       = 1;
        ctrls->intra_mode_end     = DC_PRED;
        ctrls->angular_pred_level = angular_pred_level[intra_level];
#if OPT_PER_BLK_INTRA
        ctrls->prune_using_best_mode = 0;
        ctrls->prune_using_edge_info = 0;
#else
        ctrls->prune_using_best_mode = 1;
#endif
        break;
    default:
        assert(0);
        break;
    }
    switch (dist_based_ang_intra_level) {
    case 0:
        ctrls->skip_angular_delta1_th = -1;
        ctrls->skip_angular_delta2_th = -1;
        ctrls->skip_angular_delta3_th = -1;
        break;
    case 1:
        ctrls->skip_angular_delta1_th = 25;
        ctrls->skip_angular_delta2_th = 25;
        ctrls->skip_angular_delta3_th = 25;
        break;
    case 2:
        ctrls->skip_angular_delta1_th = 20;
        ctrls->skip_angular_delta2_th = 10;
        ctrls->skip_angular_delta3_th = 5;
        break;
    default:
        assert(0);
        break;
    }
    /* For PD1, the ability to skip intra must be set at the pic level to ensure all SBs
    perform inverse TX and generate the recon. */
    if (ctx->pd_pass == PD_PASS_1) {
        ctx->skip_intra = pcs->skip_intra;

    } else {
        ctx->skip_intra = !(ctrls->enable_intra) || pcs->skip_intra;
#if !FTR_VLPD0
        assert(IMPLIES(ctx->lpd0_ctrls.pd0_level == VERY_LIGHT_PD0, ctx->skip_intra == 1));
#endif
    }
}

static void set_tx_shortcut_ctrls(PictureControlSet* pcs, ModeDecisionContext* ctx, uint8_t tx_shortcut_level) {
    PictureParentControlSet* ppcs  = pcs->ppcs;
    TxShortcutCtrls*         ctrls = &ctx->tx_shortcut_ctrls;

    switch (tx_shortcut_level) {
    case 0:
        ctrls->bypass_tx_th          = 0;
        ctrls->apply_pf_on_coeffs    = 0;
        ctrls->use_mds3_shortcuts_th = 0;
        ctrls->chroma_detector_level = 0;
        break;
    case 1:
        ctrls->bypass_tx_th          = 4;
        ctrls->apply_pf_on_coeffs    = 1;
        ctrls->use_mds3_shortcuts_th = 0;
        ctrls->chroma_detector_level = 1;
        break;
    case 2:
        ctrls->bypass_tx_th          = 1;
        ctrls->apply_pf_on_coeffs    = 1;
        ctrls->use_mds3_shortcuts_th = 0;
        ctrls->chroma_detector_level = 1;
        break;
    case 3:
        ctrls->bypass_tx_th          = 1;
        ctrls->apply_pf_on_coeffs    = 1;
        ctrls->use_mds3_shortcuts_th = 10;
        ctrls->chroma_detector_level = 1;
        break;
    default:
        assert(0);
        break;
    }

    // Chroma detector should be used in M11 and below (at least in REF frames) to prevent blurring artifacts in some clips
    if (tx_shortcut_level && !ppcs->is_highest_layer && pcs->enc_mode <= ENC_M9) {
        assert(ctrls->chroma_detector_level &&
               "Chroma detector should be used for ref frames in low presets to prevent blurring "
               "artifacts.");
    }
}

static void set_mds0_controls(ModeDecisionContext* ctx, uint8_t mds0_level) {
    Mds0Ctrls* ctrls = &ctx->mds0_ctrls;
    switch (mds0_level) {
    case 0:
        ctrls->pruning_method_th = 0;
        break;
    case 1:
        ctrls->pruning_method_th                       = 100;
        ctrls->per_class_dist_to_cost_th[CAND_CLASS_0] = 50;
        ctrls->per_class_dist_to_cost_th[CAND_CLASS_1] = 10;
        ctrls->per_class_dist_to_cost_th[CAND_CLASS_2] = 10;
        ctrls->per_class_dist_to_cost_th[CAND_CLASS_3] = 50;
        break;
    case 2:
        ctrls->pruning_method_th = (uint8_t)~0;
        ctrls->dist_to_cost_th   = 0;
        break;
    default:
        assert(0);
        break;
    }
}

static void set_skip_sub_depth_ctrls(SkipSubDepthCtrls* skip_sub_depth_ctrls, uint8_t skip_sub_depth_lvl) {
    switch (skip_sub_depth_lvl) {
    case 0:
        skip_sub_depth_ctrls->enabled = 0;
        break;

    case 1:
        skip_sub_depth_ctrls->enabled = 1;
        // Cond1 ctrls
        skip_sub_depth_ctrls->max_size          = 16;
        skip_sub_depth_ctrls->quad_deviation_th = 250;
        skip_sub_depth_ctrls->coeff_perc        = 15;

        break;
    case 2:

        skip_sub_depth_ctrls->enabled = 1;
        // Cond1 ctrls
        skip_sub_depth_ctrls->max_size          = 16;
        skip_sub_depth_ctrls->quad_deviation_th = 250;
        skip_sub_depth_ctrls->coeff_perc        = 25;

        break;
    default:
        assert(0);
        break;
    }
}
#if !CLN_REMOVE_VAR_SUB_DEPTH
static void set_var_skip_sub_depth_ctrls(VarSkipSubDepthCtrls* var_skip_sub_depth_ctrls,
                                         uint8_t               var_skip_sub_depth_lvl) {
    switch (var_skip_sub_depth_lvl) {
    case 0:
        var_skip_sub_depth_ctrls->enabled = 0;
        break;

    case 1:
        var_skip_sub_depth_ctrls->enabled  = 1;
        var_skip_sub_depth_ctrls->coeff_th = 25;
        var_skip_sub_depth_ctrls->min_size = 16;
        var_skip_sub_depth_ctrls->max_size = 16;
        // b16
        var_skip_sub_depth_ctrls->edge_th[2][0] = 0;
        var_skip_sub_depth_ctrls->edge_th[2][1] = 75;
        var_skip_sub_depth_ctrls->edge_th[2][2] = 1750;

        break;
    case 2:
        var_skip_sub_depth_ctrls->enabled  = 1;
        var_skip_sub_depth_ctrls->coeff_th = 40;
        var_skip_sub_depth_ctrls->min_size = 16;
        var_skip_sub_depth_ctrls->max_size = 32;
        // b16
        var_skip_sub_depth_ctrls->edge_th[2][0] = 100;
        var_skip_sub_depth_ctrls->edge_th[2][1] = 150;
        var_skip_sub_depth_ctrls->edge_th[2][2] = 2000;
        // b32
        var_skip_sub_depth_ctrls->edge_th[1][0] = 75;
        var_skip_sub_depth_ctrls->edge_th[1][1] = 100;
        var_skip_sub_depth_ctrls->edge_th[1][2] = 1500;
        break;
    default:
        assert(0);
        break;
    }
}
#endif
void set_block_based_depth_refinement_controls(ModeDecisionContext* ctx, uint8_t block_based_depth_refinement_level) {
    DepthRefinementCtrls* depth_refinement_ctrls = &ctx->depth_refinement_ctrls;

    switch (block_based_depth_refinement_level) {
    case 0:
        depth_refinement_ctrls->mode = PD0_DEPTH_NO_RESTRICTION;
        break;
    case 1:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 200;
        depth_refinement_ctrls->s2_parent_to_current_th    = 0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 200;
        depth_refinement_ctrls->e2_sub_to_current_th       = 0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 10;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->cost_band_based_modulation = 0;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 0;
        depth_refinement_ctrls->split_rate_th              = 0;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 0;
        depth_refinement_ctrls->use_ref_info               = 0;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 2:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 90;
        depth_refinement_ctrls->s2_parent_to_current_th    = 0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 90;
        depth_refinement_ctrls->e2_sub_to_current_th       = 0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 10;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->cost_band_based_modulation = 0;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 10;
        depth_refinement_ctrls->split_rate_th              = 10;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 0;
        depth_refinement_ctrls->use_ref_info               = 0;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 3:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 60;
        depth_refinement_ctrls->s2_parent_to_current_th    = 0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 60;
        depth_refinement_ctrls->e2_sub_to_current_th       = 0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 10;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->cost_band_based_modulation = 0;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 10;
        depth_refinement_ctrls->split_rate_th              = 10;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 0;
        depth_refinement_ctrls->use_ref_info               = 0;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 4:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 30;
        depth_refinement_ctrls->s2_parent_to_current_th    = 0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 30;
        depth_refinement_ctrls->e2_sub_to_current_th       = 0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 10;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->cost_band_based_modulation = 0;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 10;
        depth_refinement_ctrls->split_rate_th              = 10;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 0;
        depth_refinement_ctrls->use_ref_info               = 0;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 5:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 30;
        depth_refinement_ctrls->s2_parent_to_current_th    = (uint8_t)~0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 30;
        depth_refinement_ctrls->e2_sub_to_current_th       = (uint8_t)~0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 10;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->cost_band_based_modulation = 0;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 10;
        depth_refinement_ctrls->split_rate_th              = 10;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 2;
        depth_refinement_ctrls->use_ref_info               = 0;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 6:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 15;
        depth_refinement_ctrls->s2_parent_to_current_th    = (uint8_t)~0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 15;
        depth_refinement_ctrls->e2_sub_to_current_th       = (uint8_t)~0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 10;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->cost_band_based_modulation = 0;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 20;
        depth_refinement_ctrls->split_rate_th              = 10;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 1;
        depth_refinement_ctrls->use_ref_info               = 0;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 7:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 15;
        depth_refinement_ctrls->s2_parent_to_current_th    = (uint8_t)~0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 15;
        depth_refinement_ctrls->e2_sub_to_current_th       = (uint8_t)~0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 0;
        depth_refinement_ctrls->cost_band_based_modulation = 1;
        depth_refinement_ctrls->max_cost_multiplier        = 400;
        depth_refinement_ctrls->max_band_cnt               = 4;
        depth_refinement_ctrls->decrement_per_band[0]      = MAX_SIGNED_VALUE;
        depth_refinement_ctrls->decrement_per_band[1]      = MAX_SIGNED_VALUE;
        depth_refinement_ctrls->decrement_per_band[2]      = 10;
        depth_refinement_ctrls->decrement_per_band[3]      = 5;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 20;
        depth_refinement_ctrls->split_rate_th              = 5;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 1;
        depth_refinement_ctrls->use_ref_info               = 1;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 2;
        break;
    case 8:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 10;
        depth_refinement_ctrls->s2_parent_to_current_th    = (uint8_t)~0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 10;
        depth_refinement_ctrls->e2_sub_to_current_th       = (uint8_t)~0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 0;
        depth_refinement_ctrls->cost_band_based_modulation = 1;
        depth_refinement_ctrls->max_cost_multiplier        = 400;
        depth_refinement_ctrls->max_band_cnt               = 4;
        depth_refinement_ctrls->decrement_per_band[0]      = MAX_SIGNED_VALUE;
        depth_refinement_ctrls->decrement_per_band[1]      = MAX_SIGNED_VALUE;
        depth_refinement_ctrls->decrement_per_band[2]      = 10;
        depth_refinement_ctrls->decrement_per_band[3]      = 5;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 25;
        depth_refinement_ctrls->split_rate_th              = 5;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 1;
        depth_refinement_ctrls->use_ref_info               = 1;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 0;
        break;
#if FTR_NIC_DREFI_NEW_LVL_DEFS
    case 9:
        depth_refinement_ctrls->mode                       = PD0_DEPTH_ADAPTIVE;
        depth_refinement_ctrls->s1_parent_to_current_th    = 10;
        depth_refinement_ctrls->s2_parent_to_current_th    = (uint8_t)~0;
        depth_refinement_ctrls->e1_sub_to_current_th       = 10;
        depth_refinement_ctrls->e2_sub_to_current_th       = (uint8_t)~0;
        depth_refinement_ctrls->parent_max_cost_th_mult    = 0;
        depth_refinement_ctrls->cost_band_based_modulation = 1;
        depth_refinement_ctrls->max_cost_multiplier        = 400;
        depth_refinement_ctrls->max_band_cnt               = 4;
        depth_refinement_ctrls->decrement_per_band[0]      = MAX_SIGNED_VALUE;
        depth_refinement_ctrls->decrement_per_band[1]      = MAX_SIGNED_VALUE;
        depth_refinement_ctrls->decrement_per_band[2]      = 10;
        depth_refinement_ctrls->decrement_per_band[3]      = 5;
        depth_refinement_ctrls->coeff_lvl_modulation       = 1;
        depth_refinement_ctrls->lower_depth_split_cost_th  = 100;
        depth_refinement_ctrls->split_rate_th              = 5;
        depth_refinement_ctrls->limit_max_min_to_pd0       = 1;
        depth_refinement_ctrls->use_ref_info               = 1;
        depth_refinement_ctrls->q_weight                   = 1;
        depth_refinement_ctrls->pd0_unavail_mode_depth     = 0;
        break;
    case 10:
        depth_refinement_ctrls->mode = PD0_DEPTH_PRED_PART_ONLY;
        break;
#else
    case 9:
        depth_refinement_ctrls->mode = PD0_DEPTH_PRED_PART_ONLY;
        break;
#endif
    }
}
#if TUNE_STILL_IMAGE
static void get_max_block_size_default(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    ctx->max_block_size = pcs->scs->super_block_size;
}

static void get_max_block_size_rtc(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    ctx->max_block_size = pcs->scs->super_block_size;
}

static void get_max_block_size_allintra(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EncMode enc_mode = pcs->enc_mode;
    // Base threshold
    uint16_t base_var_th_cap;

    if (enc_mode <= ENC_M7) {
        base_var_th_cap = (uint16_t)~0;
    } else {
        base_var_th_cap = 7500;
    }

    // QP-based scaling
    uint32_t q_weight, q_weight_denom;
    svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.cap_max_size_qp_based_th_scaling,
                                            &q_weight,
                                            &q_weight_denom,
                                            pcs->scs->static_config.qp);

    uint16_t var_th_cap = (base_var_th_cap == (uint16_t)~0)
        ? base_var_th_cap
        : DIVIDE_AND_ROUND(base_var_th_cap * q_weight, q_weight_denom);

    // Get variance
    uint16_t variance;
    if (pcs->scs->super_block_size == 64) {
        variance = pcs->ppcs->variance[ctx->sb_index][ME_TIER_ZERO_PU_64x64];
    } else {
        get_sb128_variance(pcs, ctx, &variance);
    }

    // Set max block size
    ctx->max_block_size = (variance <= var_th_cap) ? pcs->scs->super_block_size : pcs->scs->super_block_size >> 1;
}
#else
static void get_max_block_size(PictureControlSet* pcs, ModeDecisionContext* ctx, bool allintra) {
    EncMode enc_mode = pcs->enc_mode;
    if (allintra) {
        // Base threshold
        uint16_t base_var_th_cap;
        if (enc_mode <= ENC_M9) {
            base_var_th_cap = (uint16_t)~0;
        } else {
            base_var_th_cap = 7500;
        }

        // QP-based scaling
        uint32_t q_weight, q_weight_denom;
        svt_aom_get_qp_based_th_scaling_factors(pcs->scs->qp_based_th_scaling_ctrls.cap_max_size_qp_based_th_scaling,
                                                &q_weight,
                                                &q_weight_denom,
                                                pcs->scs->static_config.qp);

        uint16_t var_th_cap = (base_var_th_cap == (uint16_t)~0)
            ? base_var_th_cap
            : DIVIDE_AND_ROUND(base_var_th_cap * q_weight, q_weight_denom);

        // Get variance
        uint16_t variance;
        if (pcs->scs->super_block_size == 64) {
            variance = pcs->ppcs->variance[ctx->sb_index][ME_TIER_ZERO_PU_64x64];
        } else {
            get_sb128_variance(pcs, ctx, &variance);
        }

        // Set max block size
        ctx->max_block_size = (variance <= var_th_cap) ? pcs->scs->super_block_size : pcs->scs->super_block_size >> 1;
    } else {
        ctx->max_block_size = pcs->scs->super_block_size;
    }
}
#endif
/*
 * Generate per-SB MD settings (do not change per-PD)
 */
void svt_aom_sig_deriv_enc_dec_common(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EncMode    enc_mode          = pcs->enc_mode;
    const bool rtc_tune          = scs->static_config.rtc;
    const bool allintra          = scs->allintra;
    const bool is_not_last_layer = !pcs->ppcs->is_highest_layer;
    set_block_based_depth_refinement_controls(ctx, pcs->pic_block_based_depth_refinement_level);

    // pic_pred_depth_only shouldn't be changed after this point
    ctx->pred_depth_only = ctx->pic_pred_depth_only = (ctx->depth_refinement_ctrls.mode == PD0_DEPTH_PRED_PART_ONLY);
    set_lpd0_ctrls(ctx, pcs->pic_lpd0_lvl);

    B64Geom* b64_geom                             = &pcs->ppcs->b64_geom[ctx->sb_index];
    ctx->depth_removal_ctrls.disallow_below_64x64 = 0;
    ctx->depth_removal_ctrls.disallow_below_32x32 = 0;
    /*
    if disallow_below_16x16 is turned ON then enable_me_8x8 should be turned OFF for the same preset in order to save memory and cycles as that feature optimizes the me_candidate_array,
    me_mv_array and the total_me_candidate_index arrays when 8x8 blocks are not used

    if any check other than an I-SLICE check is used on disallow_below_16x16 then the enable_me_8x8 should be turned ON for the entire preset because without the 8x8 me data the non I-SLICE pictures
    that use 8x8 blocks will lose significant BD-Rate as the parent 16x16 me data will be used for the 8x8 blocks
    */
    ctx->depth_removal_ctrls.disallow_below_16x16 = 0;
    if (b64_geom->width % 32 != 0 || b64_geom->height % 32 != 0) {
        ctx->depth_removal_ctrls.disallow_below_64x64 = false;
    }
    if (b64_geom->width % 16 != 0 || b64_geom->height % 16 != 0) {
        ctx->depth_removal_ctrls.disallow_below_32x32 = false;
    }
    if (b64_geom->width % 8 != 0 || b64_geom->height % 8 != 0) {
        ctx->depth_removal_ctrls.disallow_below_16x16 = false;
    }
    // Must check disallow_8x8 on an SB level. If a preset wants 8x8 off, it may still be required at the
    // picture edge if the SB width/height is <=8 (to ensure that there is a conformant block to encode
    // for those dimensions). Use the SB width/height so that 8x8 can still be skipped for all complete
    // SBs and used only for the incomplete blocks that require 8x8 for conformance.
#if TUNE_STILL_IMAGE
    ctx->disallow_8x8 = allintra ? svt_aom_get_disallow_8x8_allintra()
        : rtc_tune               ? svt_aom_get_disallow_8x8_rtc(enc_mode, b64_geom->width, b64_geom->height)
                                 : svt_aom_get_disallow_8x8_default();
#else
    ctx->disallow_8x8 = svt_aom_get_disallow_8x8(enc_mode, allintra, rtc_tune, b64_geom->width, b64_geom->height);
#endif
    ctx->disallow_4x4 = pcs->pic_disallow_4x4;
    // Set max block-size
#if TUNE_STILL_IMAGE
    allintra       ? get_max_block_size_allintra(pcs, ctx)
        : rtc_tune ? get_max_block_size_rtc(pcs, ctx)
                   : get_max_block_size_default(pcs, ctx);
#else
    get_max_block_size(pcs, ctx, allintra);
#endif
    set_depth_removal_level_controls(pcs, ctx, pcs->pic_depth_removal_level);
    if (rtc_tune) {
        if ((!scs->use_flat_ipp && enc_mode <= ENC_M7) || (scs->use_flat_ipp && enc_mode <= ENC_M6)) {
            set_lpd1_ctrls(ctx, pcs->pic_lpd1_lvl);
        } else {
            int lpd1_lvl = pcs->pic_lpd1_lvl;
            if (pcs->slice_type != I_SLICE) {
                int me_8x8 = pcs->ppcs->me_8x8_cost_variance[ctx->sb_index];
                int th     = ((!scs->use_flat_ipp && enc_mode <= ENC_M10) || (scs->use_flat_ipp && enc_mode <= ENC_M11))
                        ? 3 * ctx->qp_index
                        : 3000;

                // when lpd1 is optimized, this lpd1_lvl == 0 check should be removed, leaving only the lpd1_lvl +=2 statement
                // this extra check has been added to help low-delay perform similarly to v1.7.0
                if (lpd1_lvl == 0) {
                    if (me_8x8 < th) {
                        lpd1_lvl += 3;
                    }
                } else {
                    if (me_8x8 < th) {
                        lpd1_lvl += 2;
                    }
                }
                // For SBs that are part of the cyclic refresh, use conservative settings
                if (ctx->sb_ptr->qindex != pcs->ppcs->frm_hdr.quantization_params.base_q_idx) {
                    lpd1_lvl -= 2;
                }
            }
            // Checking against 7, as 7 is the max level for lpd1_lvl. If max level for lpd1_lvl changes, the check should be updated
            lpd1_lvl = MAX(0, MIN(lpd1_lvl, 7));
            set_lpd1_ctrls(ctx, lpd1_lvl);
        }
    } else if (enc_mode <= ENC_M10) {
        set_lpd1_ctrls(ctx, pcs->pic_lpd1_lvl);
    } else {
        int lpd1_lvl = pcs->pic_lpd1_lvl;
        if (pcs->slice_type != I_SLICE) {
            int me_8x8 = pcs->ppcs->me_8x8_cost_variance[ctx->sb_index];
            int th     = enc_mode <= ENC_M8 ? 3 * ctx->qp_index : 3000;
            if (lpd1_lvl == 0) {
                if (me_8x8 < th) {
                    lpd1_lvl += 3;
                }
            } else {
                if (me_8x8 < th) {
                    lpd1_lvl += 2;
                }
            }
        }
        lpd1_lvl = MAX(0, MIN(lpd1_lvl, 7));
        set_lpd1_ctrls(ctx, lpd1_lvl);
    }

    if ((!rtc_tune && enc_mode <= ENC_M10) || (rtc_tune && enc_mode <= ENC_M9)) {
        ctx->pd1_lvl_refinement = 0;
    } else if (rtc_tune && scs->use_flat_ipp && enc_mode <= ENC_M10) {
        ctx->pd1_lvl_refinement = is_not_last_layer ? 0 : 2;
    } else {
        ctx->pd1_lvl_refinement = 2;
    }
    svt_aom_set_nsq_geom_ctrls(ctx, pcs->nsq_geom_level, NULL, NULL, NULL);

    if (scs->static_config.max_tx_size == 32) {
        // Ensure we allow at least 32x32 transforms
        ctx->depth_removal_ctrls.disallow_below_64x64 = false;
    }
}

static void set_depth_early_exit_ctrls(ModeDecisionContext* ctx, uint8_t early_exit_level) {
    DepthEarlyExitCtrls* ctrls = &ctx->depth_early_exit_ctrls;

    switch (early_exit_level) {
    case 0:
        ctrls->split_cost_th = 0;
        ctrls->early_exit_th = 0;
        break;
    case 1:
        ctrls->split_cost_th = 50;
        ctrls->early_exit_th = 0;
        break;
    case 2:
        ctrls->split_cost_th = 50;
        ctrls->early_exit_th = 900;
        break;

    default:
        assert(0);
        break;
    }
}

// Set signals used for light-pd0 path; only PD0 should call this function
// assumes NSQ OFF, no 4x4, no chroma, no TXT/TXS/RDOQ/SSSE, SB_64x64
void svt_aom_sig_deriv_enc_dec_light_pd0(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx) {
    const Pd0Level           pd0_level = ctx->lpd0_ctrls.pd0_level;
    PictureParentControlSet* ppcs      = pcs->ppcs;
    const uint8_t            is_islice = pcs->slice_type == I_SLICE;
    const bool               allintra  = scs->allintra;

    const uint8_t sc_class1         = ppcs->sc_class1;
    const bool    rtc_tune          = scs->static_config.rtc;
    const bool    is_not_last_layer = !ppcs->is_highest_layer;
    ctx->md_disallow_nsq_search     = 1;

    // Use coeff rate and slit flag rate only (i.e. no fast rate)
    ctx->shut_fast_rate = true;

    uint8_t depth_early_exit_lvl = 1;
    // When only the predicted depth is used, use safe early exit THs
    if (rtc_tune && pd0_level == VERY_LIGHT_PD0) {
        depth_early_exit_lvl = 0;
    } else if (pd0_level <= LPD0_LVL_0 || ctx->pic_pred_depth_only) {
        depth_early_exit_lvl = 1;
    } else {
        depth_early_exit_lvl = 2;
    }
    set_depth_early_exit_ctrls(ctx, depth_early_exit_lvl);

    uint8_t intra_level = 0;

    if (pcs->enc_mode <= ENC_M10 || (sc_class1 && rtc_tune)) {
        if (pcs->slice_type == I_SLICE || ppcs->transition_present == 1) {
            intra_level = 1;
        } else if (pd0_level <= LPD0_LVL_1) {
            uint64_t use_intra_pd0_th = compute_intra_pd0_th(scs, ctx);
            uint32_t fast_lambda = ctx->hbd_md ? ctx->fast_lambda_md[EB_10_BIT_MD] : ctx->fast_lambda_md[EB_8_BIT_MD];
            uint64_t cost_64x64  = RDCOST(fast_lambda, 0, ppcs->me_64x64_distortion[ctx->sb_index]);

            intra_level = (cost_64x64 < use_intra_pd0_th) ? 0 : 1;
        } else {
            intra_level = 0;
        }
    } else {
        if (pcs->slice_type == I_SLICE || ppcs->transition_present == 1) {
            intra_level = 8;
        } else {
            intra_level = 0;
        }
    }
    set_intra_ctrls(pcs, ctx, intra_level, 2);
#if FTR_VLPD0
    if (!scs->allintra && pd0_level == VERY_LIGHT_PD0) {
#else
    if (pd0_level == VERY_LIGHT_PD0) {
#endif
        // Modulate the inter-depth bias based on the QP and the temporal complexity of the SB
        // towards more split for low QPs or/and complex SBs,
        // and less split for high QPs or/and easy SBs (to compensate for the absence of the coeff rate)
        const uint32_t init_bias_tab[INPUT_SIZE_COUNT] = {1200, 1150, 1150, 1150, 1100, 1050, 1000};
        ctx->parent_cost_bias                          = init_bias_tab[pcs->ppcs->input_resolution] -
            pcs->ppcs->frm_hdr.quantization_params.base_q_idx;
        if (ppcs->me_8x8_cost_variance[ctx->sb_index] > 1000) {
            ctx->parent_cost_bias = ctx->parent_cost_bias + 150;
        } else if (ppcs->me_8x8_cost_variance[ctx->sb_index] > 500) {
            ctx->parent_cost_bias = ctx->parent_cost_bias + 50;
        } else if (ppcs->me_8x8_cost_variance[ctx->sb_index] > 250) {
            ctx->parent_cost_bias = ctx->parent_cost_bias - 50;
        } else {
            ctx->parent_cost_bias = ctx->parent_cost_bias - 150;
        }
    } else {
        ctx->parent_cost_bias = 1000;
    }

    if (allintra) {
        ctx->lpd0_use_src_samples = true;
    } else {
        ctx->lpd0_use_src_samples = false;
    }
#if !CLN_REMOVE_VAR_SUB_DEPTH
    uint8_t var_skip_sub_depth_lvl;
    if (allintra) {
        if (pcs->enc_mode <= ENC_M11) {
            var_skip_sub_depth_lvl = 0;
        } else if (scs->input_resolution <= INPUT_SIZE_480p_RANGE) {
            var_skip_sub_depth_lvl = 0;
        } else {
            var_skip_sub_depth_lvl = 1;
        }
    } else {
        var_skip_sub_depth_lvl = 0;
    }
    set_var_skip_sub_depth_ctrls(&ctx->var_skip_sub_depth_ctrls, var_skip_sub_depth_lvl);
#endif
    if (pd0_level == VERY_LIGHT_PD0) {
        return;
    }
    svt_aom_set_chroma_controls(ctx, 0 /*chroma off*/);

    // Using PF in LPD0 may cause some VQ issues
    set_pf_controls(ctx, 1);

    uint8_t subres_level;
    // Use b64_geom here because LPD0 is only enabled when 64x64 SB size is used
    B64Geom* b64_geom = &ppcs->b64_geom[ctx->sb_index];
    // LPD0 was designed assuming 4x4 blocks were disallowed. Since LPD0 is now used in some presets where 4x4 is on
    // check that subres is not used when 4x4 blocks are enabled.
    if (pd0_level <= LPD0_LVL_1 || !ctx->disallow_4x4 || !b64_geom->is_complete_b64) {
        subres_level = 0;
    } else {
        subres_level = 0;
        // The controls checks the deviation between: (1) the pred-to-src SAD of even rows and (2) the pred-to-src SAD of odd rows for each 64x64 to decide whether to use subres or not
        // then applies the result to the 64x64 block and to all children, therefore if incomplete 64x64 then shut subres
        // Use ME distortion and variance detector to enable subres
        uint64_t use_subres_th = compute_subres_th(scs, ctx);
        uint32_t fast_lambda   = ctx->hbd_md ? ctx->fast_lambda_md[EB_10_BIT_MD] : ctx->fast_lambda_md[EB_8_BIT_MD];
        uint64_t cost_64x64    = RDCOST(fast_lambda, 0, ppcs->me_64x64_distortion[ctx->sb_index]);

        if (pd0_level <= LPD0_LVL_2) {
            if (is_islice || ppcs->transition_present == 1) {
                subres_level = 1;
            } else {
                subres_level = (cost_64x64 < use_subres_th) ? 1 : 0;
            }
        } else if (pd0_level <= LPD0_LVL_3) {
            if (is_islice || ppcs->transition_present == 1) {
                subres_level = 1;
            } else if (is_not_last_layer) {
                subres_level = (cost_64x64 < use_subres_th) ? 1 : 0;
            } else {
                subres_level = 2;
            }
        } else {
            if (is_not_last_layer) {
                subres_level = ctx->disallow_8x8 ||
                        (ctx->depth_removal_ctrls.enabled &&
                         (ctx->depth_removal_ctrls.disallow_below_16x16 ||
                          ctx->depth_removal_ctrls.disallow_below_32x32 ||
                          ctx->depth_removal_ctrls.disallow_below_64x64))
                    ? 2
                    : 1;
            } else {
                subres_level = 2;
            }
        }
    }
    set_subres_controls(ctx, subres_level);
#if OPT_RATE_ESTIMATION
    uint8_t rate_est_level;
    if (pd0_level <= LPD0_LVL_2 && rtc_tune && pcs->enc_mode >= ENC_M12) {
        rate_est_level = 2;
    } else if (pd0_level <= LPD0_LVL_3) {
        rate_est_level = 4;
    } else {
        rate_est_level = 0;
    }
    // Don't use a more conservative level in LPD1 than the regular path would use
    if (rate_est_level) {
        rate_est_level = MAX(rate_est_level, pcs->rate_est_level);
    }

    set_rate_est_ctrls(ctx, rate_est_level);
#else
    if (pd0_level <= LPD0_LVL_2 && rtc_tune && pcs->enc_mode >= ENC_M12) {
        set_rate_est_ctrls(ctx, 2);
    } else if (pd0_level <= LPD0_LVL_3) {
        set_rate_est_ctrls(ctx, 4);
    } else {
        set_rate_est_ctrls(ctx, 0);
    }
#endif
    // set at pic-level b/c feature depends on some pic-level initializations
    ctx->approx_inter_rate = 1;
}

void svt_aom_sig_deriv_enc_dec_light_pd1(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    Pd1Level                 lpd1_level        = ctx->lpd1_ctrls.pd1_level;
    PictureParentControlSet* ppcs              = pcs->ppcs;
    const EbInputResolution  input_resolution  = ppcs->input_resolution;
    const uint8_t            is_islice         = pcs->slice_type == I_SLICE;
    const SliceType          slice_type        = pcs->slice_type;
    const bool               is_not_last_layer = !ppcs->is_highest_layer;
    // Get ref info, used to set some feature levels
    const uint32_t picture_qp           = ppcs->picture_qp;
    uint32_t       me_8x8_cost_variance = (uint32_t)~0;
    uint32_t       me_64x64_distortion  = (uint32_t)~0;
    uint8_t        l0_was_skip = 0, l1_was_skip = 0;
    uint8_t        l0_was_64x64_mvp = 0, l1_was_64x64_mvp = 0;
    const bool     rtc_tune     = pcs->scs->static_config.rtc;
    const EncMode  enc_mode     = pcs->enc_mode;
    const bool     use_flat_ipp = pcs->scs->use_flat_ipp;
    // the frame size of reference pics are different if enable reference scaling.
    // sb info can not be reused because super blocks are mismatched, so we set
    // the reference pic unavailable to avoid using wrong info
    const bool is_ref_l0_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_0, 0);
    const bool is_ref_l1_avail = svt_aom_is_ref_same_size(pcs, REF_LIST_1, 0);

    // REF info only available if frame is not an I_SLICE
    if (!is_islice && is_ref_l0_avail) {
        me_8x8_cost_variance          = ppcs->me_8x8_cost_variance[ctx->sb_index];
        me_64x64_distortion           = ppcs->me_64x64_distortion[ctx->sb_index];
        EbReferenceObject* ref_obj_l0 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
        l0_was_skip = ref_obj_l0->sb_skip[ctx->sb_index], l1_was_skip = 1;
        l0_was_64x64_mvp = ref_obj_l0->sb_64x64_mvp[ctx->sb_index], l1_was_64x64_mvp = 1;
        if (slice_type == B_SLICE && is_ref_l1_avail && pcs->ppcs->ref_list1_count_try) {
            EbReferenceObject* ref_obj_l1 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
            l1_was_skip                   = ref_obj_l1->sb_skip[ctx->sb_index];
            l1_was_64x64_mvp              = ref_obj_l1->sb_64x64_mvp[ctx->sb_index];
        }
    }
    uint8_t ref_skip_perc = pcs->ref_skip_percentage;

    // Set candidate reduction levels
    uint8_t cand_reduction_level = 0;
    if (is_islice) {
        cand_reduction_level = 0;
    } else {
        if (lpd1_level <= LPD1_LVL_0) {
            cand_reduction_level = 2;
        } else if (lpd1_level <= LPD1_LVL_2) {
            cand_reduction_level = 3;
        } else if (lpd1_level <= LPD1_LVL_3) {
            cand_reduction_level = 4;
        } else {
            cand_reduction_level = 5;
        }
    }
    if (ppcs->scs->rc_stat_gen_pass_mode) {
        cand_reduction_level = 6;
    }
    set_cand_reduction_ctrls(pcs,
                             ctx,
                             cand_reduction_level,
                             picture_qp,
                             me_8x8_cost_variance,
                             me_64x64_distortion,
                             l0_was_skip,
                             l1_was_skip,
                             ref_skip_perc);

    uint8_t rdoq_level = 0;
    if (pcs->rdoq_level) {
        if (rtc_tune) {
            if (lpd1_level <= LPD1_LVL_4) {
                rdoq_level = 1;
            } else {
                rdoq_level = 0;
            }
        } else {
            if (enc_mode <= ENC_M9) {
                if (lpd1_level <= LPD1_LVL_4) {
                    rdoq_level = 1;
                } else {
                    rdoq_level = 0;
                }
            } else {
                if (lpd1_level <= LPD1_LVL_0) {
                    rdoq_level = 4;
                } else if (lpd1_level <= LPD1_LVL_4) {
                    rdoq_level = 5;
                } else {
                    rdoq_level = 0;
                }
            }
        }
        // Don't use a more conservative level in LPD1 than the regular path would use
        if (rdoq_level) {
            rdoq_level = MAX(rdoq_level, pcs->rdoq_level);
        }
    }
    set_rdoq_controls(ctx, rdoq_level);
    uint8_t me_subpel_level = 0;
    if (lpd1_level <= LPD1_LVL_0) {
        if ((rtc_tune && !use_flat_ipp && enc_mode <= ENC_M10) || (rtc_tune && use_flat_ipp && enc_mode <= ENC_M11) ||
            (!rtc_tune && enc_mode <= ENC_M11)) {
            me_subpel_level = input_resolution <= INPUT_SIZE_480p_RANGE ? 6
                : input_resolution <= INPUT_SIZE_1080p_RANGE            ? 7
                                                                        : 10;
        } else {
            me_subpel_level = input_resolution <= INPUT_SIZE_1080p_RANGE ? 7 : 10;
        }
    } else {
        me_subpel_level = input_resolution <= INPUT_SIZE_480p_RANGE ? (is_not_last_layer ? 7 : 8)
            : input_resolution <= INPUT_SIZE_1080p_RANGE            ? 8
                                                                    : 10;
        if (((l0_was_skip && l1_was_skip && ref_skip_perc > 50) || (l0_was_64x64_mvp && l1_was_64x64_mvp)) &&
            me_8x8_cost_variance < (200 * picture_qp) && me_64x64_distortion < (200 * picture_qp)) {
            me_subpel_level = 0;
        }
    }
    md_subpel_me_controls(ctx, me_subpel_level);
    uint8_t lpd1_tx_level = 0;
    // rtc forced to use lvl 3 because using the more aggressive levels for only non-base pictures causes concentrated bit allocation for flat rtc
    if (lpd1_level <= LPD1_LVL_2 || rtc_tune) {
        lpd1_tx_level = 3;
    } else {
        lpd1_tx_level = 4;
        if (!rtc_tune &&
            (((l0_was_skip && l1_was_skip && ref_skip_perc > 35) && me_8x8_cost_variance < (800 * picture_qp) &&
              me_64x64_distortion < (800 * picture_qp)) ||
             (me_8x8_cost_variance < (100 * picture_qp) && me_64x64_distortion < (100 * picture_qp)))) {
            lpd1_tx_level = 6;
        }
    }
    set_lpd1_tx_ctrls(ctx, lpd1_tx_level);

    /* In modes below M11, only use level 1-3 for chroma detector, as more aggressive levels will cause
    blurring artifacts in certain clips.

    Do not test this signal in M10 and below during preset tuning.  This signal should be kept as an enc_mode check
    instead of and LPD1_LEVEL check to ensure that M10 and below do not use it.
    */
    if (pcs->enc_mode >= ENC_M10) {
        if (lpd1_level <= LPD1_LVL_4) {
            ctx->lpd1_tx_ctrls.chroma_detector_level = 4;
        } else {
            ctx->lpd1_tx_ctrls.chroma_detector_level = 0;
        }
    }

    /* In modes below M10, only skip non-NEAREST_NEAREST TX b/c skipping all inter TX will cause blocking artifacts
    in certain clips.  This signal is separated from the general lpd1_tx_ctrls (above) to avoid
    accidentally turning this on for modes below M13.

    Do not test this signal in M9 and below during preset tuning.  This signal should be kept as an enc_mode check
    instead of and LPD1_LEVEL check to ensure that M9 and below do not use it.
    */
    if (rtc_tune) {
        if (pcs->enc_mode <= ENC_M8) {
            ctx->lpd1_skip_inter_tx_level = 0;
        } else if (pcs->enc_mode <= ENC_M10) {
            if (lpd1_level <= LPD1_LVL_2) {
                ctx->lpd1_skip_inter_tx_level = 0;
            } else {
                ctx->lpd1_skip_inter_tx_level = 1;
                if (((l0_was_skip && l1_was_skip && ref_skip_perc > 35) && me_8x8_cost_variance < (800 * picture_qp) &&
                     me_64x64_distortion < (800 * picture_qp)) ||
                    (me_8x8_cost_variance < (100 * picture_qp) && me_64x64_distortion < (100 * picture_qp))) {
                    ctx->lpd1_skip_inter_tx_level = 2;
                }
            }
        } else {
            assert(pcs->enc_mode >= ENC_M8 && "Only enable this feature for M10+ in RA or M8+ for low delay");
            if (lpd1_level <= LPD1_LVL_2) {
                ctx->lpd1_skip_inter_tx_level = 0;
            } else if (lpd1_level <= LPD1_LVL_4) {
                ctx->lpd1_skip_inter_tx_level = 1;
            } else {
                ctx->lpd1_skip_inter_tx_level = 1;
                if (((l0_was_skip && l1_was_skip && ref_skip_perc > 35) && me_8x8_cost_variance < (800 * picture_qp) &&
                     me_64x64_distortion < (800 * picture_qp)) ||
                    (me_8x8_cost_variance < (100 * picture_qp) && me_64x64_distortion < (100 * picture_qp))) {
                    ctx->lpd1_skip_inter_tx_level = 2;
                }
            }
        }
    } else {
        if (pcs->enc_mode <= ENC_M9) {
            ctx->lpd1_skip_inter_tx_level = 0;
        } else {
            assert(pcs->enc_mode >= ENC_M10 && "Only enable this feature for M10+ in RA or M8+ for low delay");
            if (lpd1_level <= LPD1_LVL_2) {
                ctx->lpd1_skip_inter_tx_level = 0;
            } else {
                ctx->lpd1_skip_inter_tx_level = 1;
                if (((l0_was_skip && l1_was_skip && ref_skip_perc > 35) && me_8x8_cost_variance < (800 * picture_qp) &&
                     me_64x64_distortion < (800 * picture_qp)) ||
                    (me_8x8_cost_variance < (100 * picture_qp) && me_64x64_distortion < (100 * picture_qp))) {
                    ctx->lpd1_skip_inter_tx_level = 2;
                }
            }
        }
    }
    ctx->lpd1_bypass_tx_th = 0;
    if (rtc_tune) {
        if (lpd1_level <= LPD1_LVL_0) {
            ctx->lpd1_bypass_tx_th = 100;
        } else {
            ctx->lpd1_bypass_tx_th = 200;
        }
    }

#if OPT_RATE_ESTIMATION
    uint8_t rate_est_level;
    if (lpd1_level <= LPD1_LVL_0) {
        rate_est_level = 4;
    } else {
        rate_est_level = 0;
    }
    // Don't use a more conservative level in LPD1 than the regular path would use
    if (rate_est_level) {
        rate_est_level = MAX(rate_est_level, pcs->rate_est_level);
    }
#else
    uint8_t rate_est_level = 0;
    if (lpd1_level <= LPD1_LVL_0) {
        rate_est_level = 4;
    } else {
        rate_est_level = 0;
    }
#endif
    set_rate_est_ctrls(ctx, rate_est_level);

    // If want to turn off approximating inter rate, must ensure that the approximation is also disabled
    // at the pic level (pcs->approx_inter_rate)
    // If the pic level signal is set more aggressive than the default lpd1 setting (lvl 1), use the pic level.
    ctx->approx_inter_rate = MAX(1, pcs->approx_inter_rate);

    uint8_t pf_level = 1;
    set_pf_controls(ctx, pf_level);

    uint8_t intra_level = 0;
    if (pcs->intra_level) {
        if (lpd1_level <= LPD1_LVL_2) {
            intra_level = 6;
        } else {
#if OPT_PER_BLK_INTRA
            intra_level = MAX_INTRA_LEVEL - 1;
        }
#else
            intra_level = 8;
        }
#endif

        // Don't use a more conservative level in LPD1 than the regular path would use
        if (intra_level) {
            intra_level = MAX(intra_level, pcs->intra_level);
        }
    }
    set_intra_ctrls(pcs, ctx, intra_level, 2);
    /* Set signals that have assumed values in the light-PD1 path (but need to be initialized as they may be checked) */

    // Use coeff rate and slit flag rate only (i.e. no fast rate)
    ctx->shut_fast_rate   = false;
    ctx->uv_ctrls.enabled = 1;
    ctx->uv_ctrls.uv_mode = CHROMA_MODE_1;
    set_cfl_ctrls(ctx, 0);
    ctx->md_disallow_nsq_search                     = 1;
    ctx->new_nearest_injection                      = 1;
    ctx->blk_skip_decision                          = true;
    ctx->rate_est_ctrls.update_skip_ctx_dc_sign_ctx = 0;
    ctx->rate_est_ctrls.update_skip_coeff_ctx       = 0;
    ctx->subres_ctrls.odd_to_even_deviation_th      = 0;
    set_inter_intra_ctrls(ctx, 0);
}
#if TUNE_STILL_IMAGE
void svt_aom_sig_deriv_enc_dec_default(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EncMode                  enc_mode             = pcs->enc_mode;
    uint8_t                  pd_pass              = ctx->pd_pass;
    PictureParentControlSet* ppcs                 = pcs->ppcs;
    const uint8_t            sc_class1            = ppcs->sc_class1;
    const uint32_t           picture_qp           = ppcs->picture_qp;
    uint32_t                 me_8x8_cost_variance = (uint32_t)~0;
    uint32_t                 me_64x64_distortion  = (uint32_t)~0;
    uint8_t                  l0_was_skip = 0, l1_was_skip = 0;
    uint8_t                  ref_skip_perc = pcs->ref_skip_percentage;

    set_nsq_search_ctrls(pcs, ctx, pcs->nsq_search_level);
    svt_aom_set_nic_controls(ctx, ctx->pd_pass == PD_PASS_0 ? 11 : pcs->nic_level);
    set_cand_reduction_ctrls(pcs,
                             ctx,
                             pd_pass == PD_PASS_0 ? 0 : pcs->cand_reduction_level,
                             picture_qp,
                             me_8x8_cost_variance,
                             me_64x64_distortion,
                             l0_was_skip,
                             l1_was_skip,
                             ref_skip_perc);

    uint8_t txt_level = pcs->txt_level;
    svt_aom_set_txt_controls(ctx, pd_pass == PD_PASS_0 ? 0 : txt_level);
    set_tx_shortcut_ctrls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->tx_shortcut_level);

    set_interpolation_search_level_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->interpolation_search_level);

    svt_aom_set_chroma_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->chroma_level);

    set_cfl_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->cfl_level);
    if (pd_pass == PD_PASS_0) {
        ctx->md_disallow_nsq_search = 1;
    } else {
        // Update nsq settings based on the sb_class
        ctx->md_disallow_nsq_search = !ctx->nsq_geom_ctrls.enabled || !ctx->nsq_search_ctrls.enabled;
    }
    if (pd_pass == PD_PASS_0) {
        ctx->global_mv_injection = 0;
    } else {
        ctx->global_mv_injection = ppcs->gm_ctrls.enabled;
    }
    if (pd_pass == PD_PASS_0) {
        ctx->new_nearest_injection = 0;
    } else {
        ctx->new_nearest_injection = 1;
    }
    ctx->new_nearest_near_comb_injection = pd_pass == PD_PASS_0 ? 0 : pcs->new_nearest_near_comb_injection;

    //set Warped-Motion controls from Picture level.
    svt_aom_set_wm_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->wm_level);

    ctx->unipred3x3_injection = pd_pass == PD_PASS_0 ? 0 : pcs->unipred3x3_injection;
    svt_aom_set_bipred3x3_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->bipred3x3_injection);
    set_inter_comp_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->inter_compound_mode);
    svt_aom_set_dist_based_ref_pruning_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->dist_based_ref_pruning);
    set_spatial_sse_full_loop_level(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->spatial_sse_full_loop_level);
    if (ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
        ctx->blk_skip_decision = true;
    } else {
        ctx->blk_skip_decision = false;
    }

    uint8_t rdoq_level = 0;
    if (pd_pass == PD_PASS_0) {
        if (enc_mode <= ENC_M6) {
            rdoq_level = 1;
        } else {
            rdoq_level = 0;
        }
    } else {
        rdoq_level = pcs->rdoq_level;
    }

    set_rdoq_controls(ctx, rdoq_level);
    // There are only redundant blocks when HVA_HVB shapes are used
    if (pd_pass == PD_PASS_0 || !ctx->nsq_geom_ctrls.allow_HVA_HVB) {
        ctx->redundant_blk = false;
    } else {
        ctx->redundant_blk = true;
    }
    uint8_t depth_early_exit_lvl = 0;
    if (pd_pass == PD_PASS_0) {
        depth_early_exit_lvl = 1;
    } else if (enc_mode <= ENC_M6) {
        depth_early_exit_lvl = 1;
    } else {
        depth_early_exit_lvl = 2;
    }
    set_depth_early_exit_ctrls(ctx, depth_early_exit_lvl);
    set_obmc_controls(ctx, pd_pass == PD_PASS_0 ? 0 : ppcs->pic_obmc_level);
    set_inter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->inter_intra_level);
    set_txs_controls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->txs_level);
    set_filter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->pic_filter_intra_level);
    // Set md_allow_intrabc @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_allow_intrabc = 0;
    } else {
        ctx->md_allow_intrabc = pcs->ppcs->frm_hdr.allow_intrabc;
    }

    // Set md_palette_level @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_palette_level = 0;
    } else {
        ctx->md_palette_level = pcs->ppcs->palette_level;
    }

    uint8_t pf_level = 1;
    set_pf_controls(ctx, pf_level);
    md_sq_motion_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_sq_mv_search_level);

    md_nsq_motion_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_nsq_mv_search_level);
    svt_aom_md_pme_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_pme_level);
    uint8_t me_subpel_level = 0;
    if (pd_pass == PD_PASS_0) {
        if (enc_mode <= ENC_M10) {
            me_subpel_level = 4;
        } else {
            me_subpel_level = 0;
        }
    } else {
        if (enc_mode <= ENC_M2) {
            me_subpel_level = 1;
        } else if (enc_mode <= ENC_M8) {
            me_subpel_level = 4;
        } else if (enc_mode <= ENC_M11) {
            me_subpel_level = 5;
        } else {
            me_subpel_level = 6;
        }
    }
    md_subpel_me_controls(ctx, me_subpel_level);

    uint8_t pme_subpel_level = 0;
    if (pd_pass == PD_PASS_0) {
        pme_subpel_level = 0;
    } else if (enc_mode <= ENC_MR) {
        pme_subpel_level = 1;
    } else {
        pme_subpel_level = 2;
    }
    md_subpel_pme_controls(ctx, pme_subpel_level);
#if OPT_RATE_ESTIMATION
    uint8_t rate_est_level;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = pcs->rate_est_level ? MAX(2, pcs->rate_est_level) : 0;
    } else {
        rate_est_level = pcs->rate_est_level;
    }
#else
    uint8_t rate_est_level = 0;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = 2;
    } else {
        rate_est_level = 1;
    }
#endif
    set_rate_est_ctrls(ctx, rate_est_level);

    // set at pic-level b/c feature depends on some pic-level initializations
    ctx->approx_inter_rate = pcs->approx_inter_rate;
    // Use coeff rate and slit flag rate only (i.e. no fast rate)
    if (pd_pass == PD_PASS_0) {
        ctx->shut_fast_rate = true;
    } else {
        ctx->shut_fast_rate = false;
    }

    // intra_level must be greater than 0 for I_SLICE
    uint8_t intra_level                = 0;
    uint8_t dist_based_ang_intra_level = 0;
    if (pd_pass == PD_PASS_0) {
#if OPT_PER_BLK_INTRA
        intra_level                = MAX_INTRA_LEVEL - 1;
        dist_based_ang_intra_level = 0;
#else
        intra_level                = 8;
        dist_based_ang_intra_level = 2;
#endif
    } else {
        intra_level                = pcs->intra_level;
        dist_based_ang_intra_level = pcs->dist_based_ang_intra_level;
    }
    set_intra_ctrls(pcs, ctx, intra_level, dist_based_ang_intra_level);

    // Use Hadamard at MDS0
    ctx->mds0_use_hadamard_sb = false;

    set_mds0_controls(ctx, pcs->mds0_level);
    set_subres_controls(ctx, 0);
    if (pd_pass == PD_PASS_0) {
        ctx->parent_cost_bias = 1000;
    } else {
        ctx->parent_cost_bias = 995;
    }
    uint8_t skip_sub_depth_lvl;
    if (pd_pass == PD_PASS_0) {
        skip_sub_depth_lvl = 0;
    } else if (sc_class1) {
        if (enc_mode <= ENC_M6) {
            skip_sub_depth_lvl = 0;
        } else {
            skip_sub_depth_lvl = 1;
        }
    } else if (enc_mode <= ENC_M1) {
        skip_sub_depth_lvl = 1;
    } else {
        skip_sub_depth_lvl = 2;
    }

    set_skip_sub_depth_ctrls(&ctx->skip_sub_depth_ctrls, skip_sub_depth_lvl);
    ctx->tune_ssim_level = SSIM_LVL_0;
}

void svt_aom_sig_deriv_enc_dec_rtc(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EncMode                  enc_mode             = pcs->enc_mode;
    uint8_t                  pd_pass              = ctx->pd_pass;
    PictureParentControlSet* ppcs                 = pcs->ppcs;
    const uint8_t            sc_class1            = ppcs->sc_class1;
    const uint32_t           picture_qp           = ppcs->picture_qp;
    uint32_t                 me_8x8_cost_variance = (uint32_t)~0;
    uint32_t                 me_64x64_distortion  = (uint32_t)~0;
    uint8_t                  l0_was_skip = 0, l1_was_skip = 0;
    uint8_t                  ref_skip_perc = pcs->ref_skip_percentage;
    const bool               flat_rtc      = pcs->scs->use_flat_ipp;

    set_nsq_search_ctrls(pcs, ctx, pcs->nsq_search_level);
    svt_aom_set_nic_controls(ctx, ctx->pd_pass == PD_PASS_0 ? 11 : pcs->nic_level);
    set_cand_reduction_ctrls(pcs,
                             ctx,
                             pd_pass == PD_PASS_0 ? 0 : pcs->cand_reduction_level,
                             picture_qp,
                             me_8x8_cost_variance,
                             me_64x64_distortion,
                             l0_was_skip,
                             l1_was_skip,
                             ref_skip_perc);

    uint8_t txt_level = pcs->txt_level;
    svt_aom_set_txt_controls(ctx, pd_pass == PD_PASS_0 ? 0 : txt_level);
    set_tx_shortcut_ctrls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->tx_shortcut_level);

    set_interpolation_search_level_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->interpolation_search_level);

    svt_aom_set_chroma_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->chroma_level);

    set_cfl_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->cfl_level);
    if (pd_pass == PD_PASS_0) {
        ctx->md_disallow_nsq_search = 1;
    } else {
        // Update nsq settings based on the sb_class
        ctx->md_disallow_nsq_search = !ctx->nsq_geom_ctrls.enabled || !ctx->nsq_search_ctrls.enabled;
    }
    if (pd_pass == PD_PASS_0) {
        ctx->global_mv_injection = 0;
    } else {
        ctx->global_mv_injection = ppcs->gm_ctrls.enabled;
    }
    if (pd_pass == PD_PASS_0) {
        ctx->new_nearest_injection = 0;
    } else {
        ctx->new_nearest_injection = 1;
    }
    ctx->new_nearest_near_comb_injection = pd_pass == PD_PASS_0 ? 0 : pcs->new_nearest_near_comb_injection;

    //set Warped-Motion controls from Picture level.
    svt_aom_set_wm_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->wm_level);

    ctx->unipred3x3_injection = pd_pass == PD_PASS_0 ? 0 : pcs->unipred3x3_injection;
    svt_aom_set_bipred3x3_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->bipred3x3_injection);
    set_inter_comp_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->inter_compound_mode);
    svt_aom_set_dist_based_ref_pruning_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->dist_based_ref_pruning);
    set_spatial_sse_full_loop_level(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->spatial_sse_full_loop_level);
    if (ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
        ctx->blk_skip_decision = true;
    } else {
        ctx->blk_skip_decision = false;
    }

    uint8_t rdoq_level = 0;
    if (pd_pass == PD_PASS_0) {
        if (enc_mode <= ENC_M7) {
            rdoq_level = 1;
        } else {
            rdoq_level = 0;
        }
    } else {
        rdoq_level = pcs->rdoq_level;
    }

    set_rdoq_controls(ctx, rdoq_level);
    // There are only redundant blocks when HVA_HVB shapes are used
    if (pd_pass == PD_PASS_0 || !ctx->nsq_geom_ctrls.allow_HVA_HVB) {
        ctx->redundant_blk = false;
    } else {
        ctx->redundant_blk = true;
    }
    uint8_t depth_early_exit_lvl = 0;
    if (pd_pass == PD_PASS_0) {
        depth_early_exit_lvl = 1;
    } else if (enc_mode <= ENC_M6) {
        depth_early_exit_lvl = 1;
    } else {
        depth_early_exit_lvl = 2;
    }
    set_depth_early_exit_ctrls(ctx, depth_early_exit_lvl);
    set_obmc_controls(ctx, pd_pass == PD_PASS_0 ? 0 : ppcs->pic_obmc_level);
    set_inter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->inter_intra_level);
    set_txs_controls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->txs_level);
    set_filter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->pic_filter_intra_level);
    // Set md_allow_intrabc @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_allow_intrabc = 0;
    } else {
        ctx->md_allow_intrabc = pcs->ppcs->frm_hdr.allow_intrabc;
    }

    // Set md_palette_level @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_palette_level = 0;
    } else {
        ctx->md_palette_level = pcs->ppcs->palette_level;
    }

    uint8_t pf_level = 1;
    set_pf_controls(ctx, pf_level);
    md_sq_motion_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_sq_mv_search_level);

    md_nsq_motion_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_nsq_mv_search_level);
    svt_aom_md_pme_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_pme_level);
    uint8_t me_subpel_level = 0;
    if (pd_pass == PD_PASS_0) {
        if (enc_mode <= ENC_M10) {
            me_subpel_level = 4;
        } else {
            me_subpel_level = 0;
        }
    } else {
        if ((!flat_rtc && enc_mode <= ENC_M9) || (flat_rtc && enc_mode <= ENC_M8)) {
            me_subpel_level = 4;
        } else {
            me_subpel_level = 6;
        }
    }
    md_subpel_me_controls(ctx, me_subpel_level);

    uint8_t pme_subpel_level = 0;
    if (pd_pass == PD_PASS_0) {
        pme_subpel_level = 0;
    } else if (sc_class1) {
        if (enc_mode <= ENC_M7) {
            pme_subpel_level = 2;
        } else {
            pme_subpel_level = 0;
        }
    } else if (enc_mode <= ENC_MR) {
        pme_subpel_level = 1;
    } else {
        pme_subpel_level = 2;
    }
    md_subpel_pme_controls(ctx, pme_subpel_level);
#if OPT_RATE_ESTIMATION
    uint8_t rate_est_level;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = pcs->rate_est_level ? MAX(2, pcs->rate_est_level) : 0;
    } else {
        rate_est_level = pcs->rate_est_level;
    }
#else
    uint8_t rate_est_level = 0;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = 2;
    } else {
        rate_est_level = 1;
    }
#endif
    set_rate_est_ctrls(ctx, rate_est_level);

    // set at pic-level b/c feature depends on some pic-level initializations
    ctx->approx_inter_rate = pcs->approx_inter_rate;
    // Use coeff rate and slit flag rate only (i.e. no fast rate)
    if (pd_pass == PD_PASS_0) {
        ctx->shut_fast_rate = true;
    } else {
        ctx->shut_fast_rate = false;
    }

    // intra_level must be greater than 0 for I_SLICE
    uint8_t intra_level                = 0;
    uint8_t dist_based_ang_intra_level = 0;
    if (pd_pass == PD_PASS_0) {
#if OPT_PER_BLK_INTRA
        intra_level                = MAX_INTRA_LEVEL - 1;
        dist_based_ang_intra_level = 0;
#else
        intra_level                = 8;
        dist_based_ang_intra_level = 2;
#endif
    } else {
        intra_level                = pcs->intra_level;
        dist_based_ang_intra_level = pcs->dist_based_ang_intra_level;
    }
    set_intra_ctrls(pcs, ctx, intra_level, dist_based_ang_intra_level);

    // Use Hadamard at MDS0
    ctx->mds0_use_hadamard_sb = false;

    set_mds0_controls(ctx, pcs->mds0_level);
    set_subres_controls(ctx, 0);
    if (pd_pass == PD_PASS_0) {
        ctx->parent_cost_bias = 1000;
    } else {
        ctx->parent_cost_bias = 995;
    }
    uint8_t skip_sub_depth_lvl;
    if (pd_pass == PD_PASS_0) {
        skip_sub_depth_lvl = 0;
    } else if (sc_class1) {
        if (enc_mode <= ENC_M6) {
            skip_sub_depth_lvl = 0;
        } else {
            skip_sub_depth_lvl = 1;
        }
    } else if (enc_mode <= ENC_M1) {
        skip_sub_depth_lvl = 1;
    } else {
        skip_sub_depth_lvl = 2;
    }

    set_skip_sub_depth_ctrls(&ctx->skip_sub_depth_ctrls, skip_sub_depth_lvl);
    ctx->tune_ssim_level = SSIM_LVL_0;
}

void svt_aom_sig_deriv_enc_dec_allintra(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EncMode        enc_mode             = pcs->enc_mode;
    uint8_t        pd_pass              = ctx->pd_pass;
    const uint32_t picture_qp           = pcs->ppcs->picture_qp;
    uint32_t       me_8x8_cost_variance = (uint32_t)~0;
    uint32_t       me_64x64_distortion  = (uint32_t)~0;
    uint8_t        l0_was_skip = 0, l1_was_skip = 0;
    uint8_t        ref_skip_perc = pcs->ref_skip_percentage;

    set_nsq_search_ctrls(pcs, ctx, pcs->nsq_search_level);
    svt_aom_set_nic_controls(ctx, ctx->pd_pass == PD_PASS_0 ? 11 : pcs->nic_level);
    set_cand_reduction_ctrls(pcs,
                             ctx,
                             pd_pass == PD_PASS_0 ? 0 : pcs->cand_reduction_level,
                             picture_qp,
                             me_8x8_cost_variance,
                             me_64x64_distortion,
                             l0_was_skip,
                             l1_was_skip,
                             ref_skip_perc);

    uint8_t txt_level = pcs->txt_level;
    svt_aom_set_txt_controls(ctx, pd_pass == PD_PASS_0 ? 0 : txt_level);
    set_tx_shortcut_ctrls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->tx_shortcut_level);

    set_interpolation_search_level_ctrls(ctx, 0);

    svt_aom_set_chroma_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->chroma_level);

    set_cfl_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->cfl_level);
    if (pd_pass == PD_PASS_0) {
        ctx->md_disallow_nsq_search = 1;
    } else {
        // Update nsq settings based on the sb_class
        ctx->md_disallow_nsq_search = !ctx->nsq_geom_ctrls.enabled || !ctx->nsq_search_ctrls.enabled;
    }

    ctx->global_mv_injection             = 0;
    ctx->new_nearest_injection           = 0;
    ctx->new_nearest_near_comb_injection = 0;
    svt_aom_set_wm_controls(ctx, 0);
    ctx->unipred3x3_injection = 0;
    svt_aom_set_bipred3x3_controls(ctx, 0);
    set_inter_comp_controls(ctx, 0);
    svt_aom_set_dist_based_ref_pruning_controls(ctx, 0);

    set_spatial_sse_full_loop_level(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->spatial_sse_full_loop_level);
    if (ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
        ctx->blk_skip_decision = true;
    } else {
        ctx->blk_skip_decision = false;
    }

    uint8_t rdoq_level = 0;
    if (pd_pass == PD_PASS_0) {
        if (enc_mode <= ENC_M8) {
            rdoq_level = 1;
        } else {
            rdoq_level = 0;
        }
    } else {
        rdoq_level = pcs->rdoq_level;
    }

    set_rdoq_controls(ctx, rdoq_level);
    // There are only redundant blocks when HVA_HVB shapes are used
    if (pd_pass == PD_PASS_0 || !ctx->nsq_geom_ctrls.allow_HVA_HVB) {
        ctx->redundant_blk = false;
    } else {
        ctx->redundant_blk = true;
    }
    uint8_t depth_early_exit_lvl = 0;
    if (pd_pass == PD_PASS_0) {
        depth_early_exit_lvl = 1;
    }

    else if (enc_mode <= ENC_M7) {
        depth_early_exit_lvl = 1;
    } else {
        depth_early_exit_lvl = 2;
    }

    set_depth_early_exit_ctrls(ctx, depth_early_exit_lvl);
    set_obmc_controls(ctx, 0);
    set_inter_intra_ctrls(ctx, 0);
    set_txs_controls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->txs_level);
    set_filter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->pic_filter_intra_level);
    // Set md_allow_intrabc @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_allow_intrabc = 0;
    } else {
        ctx->md_allow_intrabc = pcs->ppcs->frm_hdr.allow_intrabc;
    }

    // Set md_palette_level @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_palette_level = 0;
    } else {
        ctx->md_palette_level = pcs->ppcs->palette_level;
    }

    uint8_t pf_level = 1;
    set_pf_controls(ctx, pf_level);

    md_sq_motion_search_controls(ctx, 0);
    md_nsq_motion_search_controls(ctx, 0);
    svt_aom_md_pme_search_controls(ctx, 0);
    md_subpel_me_controls(ctx, 0);
    md_subpel_pme_controls(ctx, 0);

#if OPT_RATE_ESTIMATION
    uint8_t rate_est_level;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = pcs->rate_est_level ? MAX(2, pcs->rate_est_level) : 0;
    } else {
        rate_est_level = pcs->rate_est_level;
    }
#else
    uint8_t rate_est_level = 0;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = 2;
    } else {
        rate_est_level = 1;
    }
#endif
    set_rate_est_ctrls(ctx, rate_est_level);

    // set at pic-level b/c feature depends on some pic-level initializations
    ctx->approx_inter_rate = pcs->approx_inter_rate;
    // Use coeff rate and slit flag rate only (i.e. no fast rate)
    if (pd_pass == PD_PASS_0) {
        ctx->shut_fast_rate = true;
    } else {
        ctx->shut_fast_rate = false;
    }

    // intra_level must be greater than 0 for I_SLICE
    uint8_t intra_level                = 0;
    uint8_t dist_based_ang_intra_level = 0;
    if (pd_pass == PD_PASS_0) {
        intra_level                = MAX_INTRA_LEVEL - 1;
        dist_based_ang_intra_level = 0;
    } else {
        intra_level                = pcs->intra_level;
        dist_based_ang_intra_level = pcs->dist_based_ang_intra_level;
    }
    set_intra_ctrls(pcs, ctx, intra_level, dist_based_ang_intra_level);
    // Use Hadamard at MDS0
    ctx->mds0_use_hadamard_sb = false;
    if (pd_pass == PD_PASS_0) {
        ctx->mds0_use_hadamard_sb = false;
    } else {
        ctx->mds0_use_hadamard_sb = true;
    }

    set_mds0_controls(ctx, pcs->mds0_level);
    set_subres_controls(ctx, 0);
    if (pd_pass == PD_PASS_0) {
        ctx->parent_cost_bias = 1000;
    } else {
        ctx->parent_cost_bias = 995;
    }
    uint8_t skip_sub_depth_lvl;
    if (pd_pass == PD_PASS_0) {
        skip_sub_depth_lvl = 0;
    }

    else if (enc_mode <= ENC_M7) {
        skip_sub_depth_lvl = 1;
    } else {
        skip_sub_depth_lvl = 2;
    }

    set_skip_sub_depth_ctrls(&ctx->skip_sub_depth_ctrls, skip_sub_depth_lvl);
    ctx->tune_ssim_level = SSIM_LVL_0;
}
#else
void svt_aom_sig_deriv_enc_dec(SequenceControlSet* scs, PictureControlSet* pcs, ModeDecisionContext* ctx) {
    EncMode                  enc_mode             = pcs->enc_mode;
    uint8_t                  pd_pass              = ctx->pd_pass;
    PictureParentControlSet* ppcs                 = pcs->ppcs;
    const uint8_t            sc_class1            = ppcs->sc_class1;
    const uint32_t           picture_qp           = ppcs->picture_qp;
    uint32_t                 me_8x8_cost_variance = (uint32_t)~0;
    uint32_t                 me_64x64_distortion  = (uint32_t)~0;
    uint8_t                  l0_was_skip = 0, l1_was_skip = 0;
    uint8_t                  ref_skip_perc = pcs->ref_skip_percentage;
    const bool               rtc_tune      = scs->static_config.rtc;
    const bool               allintra      = scs->allintra;
    set_nsq_search_ctrls(pcs, ctx, pcs->nsq_search_level);
    svt_aom_set_nic_controls(ctx, ctx->pd_pass == PD_PASS_0 ? 11 : pcs->nic_level);
    set_cand_reduction_ctrls(pcs,
                             ctx,
                             pd_pass == PD_PASS_0 ? 0 : pcs->cand_reduction_level,
                             picture_qp,
                             me_8x8_cost_variance,
                             me_64x64_distortion,
                             l0_was_skip,
                             l1_was_skip,
                             ref_skip_perc);

    uint8_t txt_level = pcs->txt_level;
    svt_aom_set_txt_controls(ctx, pd_pass == PD_PASS_0 ? 0 : txt_level);
    set_tx_shortcut_ctrls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->tx_shortcut_level);

    set_interpolation_search_level_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->interpolation_search_level);

    svt_aom_set_chroma_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->chroma_level);

    set_cfl_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->cfl_level);
    if (pd_pass == PD_PASS_0) {
        ctx->md_disallow_nsq_search = 1;
    } else {
        // Update nsq settings based on the sb_class
        ctx->md_disallow_nsq_search = !ctx->nsq_geom_ctrls.enabled || !ctx->nsq_search_ctrls.enabled;
    }
    if (pd_pass == PD_PASS_0) {
        ctx->global_mv_injection = 0;
    } else {
        ctx->global_mv_injection = ppcs->gm_ctrls.enabled;
    }
    if (pd_pass == PD_PASS_0) {
        ctx->new_nearest_injection = 0;
    } else {
        ctx->new_nearest_injection = 1;
    }
    ctx->new_nearest_near_comb_injection = pd_pass == PD_PASS_0 ? 0 : pcs->new_nearest_near_comb_injection;

    //set Warped-Motion controls from Picture level.
    svt_aom_set_wm_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->wm_level);

    ctx->unipred3x3_injection = pd_pass == PD_PASS_0 ? 0 : pcs->unipred3x3_injection;
    svt_aom_set_bipred3x3_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->bipred3x3_injection);
    set_inter_comp_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->inter_compound_mode);
    svt_aom_set_dist_based_ref_pruning_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->dist_based_ref_pruning);
    set_spatial_sse_full_loop_level(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->spatial_sse_full_loop_level);
    if (ctx->uv_ctrls.uv_mode <= CHROMA_MODE_1) {
        ctx->blk_skip_decision = true;
    } else {
        ctx->blk_skip_decision = false;
    }

    uint8_t rdoq_level = 0;
    if (pd_pass == PD_PASS_0) {
        if ((!rtc_tune && enc_mode <= ENC_M6) || (rtc_tune && enc_mode <= ENC_M7) ||
            (allintra && enc_mode <= ENC_M11)) {
            rdoq_level = 1;
        } else {
            rdoq_level = 0;
        }
    } else {
        rdoq_level = pcs->rdoq_level;
    }

    set_rdoq_controls(ctx, rdoq_level);
    // There are only redundant blocks when HVA_HVB shapes are used
    if (pd_pass == PD_PASS_0 || !ctx->nsq_geom_ctrls.allow_HVA_HVB) {
        ctx->redundant_blk = false;
    } else {
        ctx->redundant_blk = true;
    }
    uint8_t depth_early_exit_lvl = 0;
    if (pd_pass == PD_PASS_0) {
        depth_early_exit_lvl = 1;
    } else if (allintra) {
        if (enc_mode <= ENC_M9) {
            depth_early_exit_lvl = 1;
        } else {
            depth_early_exit_lvl = 2;
        }
    } else if (enc_mode <= ENC_M6) {
        depth_early_exit_lvl = 1;
    } else {
        depth_early_exit_lvl = 2;
    }
    set_depth_early_exit_ctrls(ctx, depth_early_exit_lvl);
    set_obmc_controls(ctx, pd_pass == PD_PASS_0 ? 0 : ppcs->pic_obmc_level);
    set_inter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->inter_intra_level);
    set_txs_controls(pcs, ctx, pd_pass == PD_PASS_0 ? 0 : pcs->txs_level);
    set_filter_intra_ctrls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->pic_filter_intra_level);
    // Set md_allow_intrabc @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_allow_intrabc = 0;
    } else {
        ctx->md_allow_intrabc = pcs->ppcs->frm_hdr.allow_intrabc;
    }

    // Set md_palette_level @ MD
    if (pd_pass == PD_PASS_0) {
        ctx->md_palette_level = 0;
    } else {
        ctx->md_palette_level = pcs->ppcs->palette_level;
    }

    uint8_t pf_level = 1;
    set_pf_controls(ctx, pf_level);
    md_sq_motion_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_sq_mv_search_level);

    md_nsq_motion_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_nsq_mv_search_level);
    svt_aom_md_pme_search_controls(ctx, pd_pass == PD_PASS_0 ? 0 : pcs->md_pme_level);
    uint8_t me_subpel_level = 0;
    if (pd_pass == PD_PASS_0) {
        if (enc_mode <= ENC_M10) {
            me_subpel_level = 4;
        } else {
            me_subpel_level = 0;
        }
    } else if (rtc_tune) {
        if ((!scs->use_flat_ipp && enc_mode <= ENC_M9) || (scs->use_flat_ipp && enc_mode <= ENC_M8)) {
            me_subpel_level = 4;
        } else {
            me_subpel_level = 6;
        }
    } else {
        if (enc_mode <= ENC_M2) {
            me_subpel_level = 1;
        } else if (enc_mode <= ENC_M8) {
            me_subpel_level = 4;
        } else if (enc_mode <= ENC_M11) {
            me_subpel_level = 5;
        } else {
            me_subpel_level = 6;
        }
    }
    md_subpel_me_controls(ctx, me_subpel_level);

    uint8_t pme_subpel_level = 0;
    if (pd_pass == PD_PASS_0) {
        pme_subpel_level = 0;
    } else if (rtc_tune && sc_class1) {
        if (enc_mode <= ENC_M7) {
            pme_subpel_level = 2;
        } else {
            pme_subpel_level = 0;
        }
    } else if (enc_mode <= ENC_MR) {
        pme_subpel_level = 1;
    } else {
        pme_subpel_level = 2;
    }
    md_subpel_pme_controls(ctx, pme_subpel_level);
    uint8_t rate_est_level = 0;
    if (pd_pass == PD_PASS_0) {
        rate_est_level = 2;
    } else {
        rate_est_level = 1;
    }
    set_rate_est_ctrls(ctx, rate_est_level);

    // set at pic-level b/c feature depends on some pic-level initializations
    ctx->approx_inter_rate = pcs->approx_inter_rate;
    // Use coeff rate and slit flag rate only (i.e. no fast rate)
    if (pd_pass == PD_PASS_0) {
        ctx->shut_fast_rate = true;
    } else {
        ctx->shut_fast_rate = false;
    }

    // intra_level must be greater than 0 for I_SLICE
    uint8_t intra_level                = 0;
    uint8_t dist_based_ang_intra_level = 0;
    if (pd_pass == PD_PASS_0) {
        intra_level                = 8;
        dist_based_ang_intra_level = 2;
    } else {
        intra_level                = pcs->intra_level;
        dist_based_ang_intra_level = pcs->dist_based_ang_intra_level;
    }
    set_intra_ctrls(pcs, ctx, intra_level, dist_based_ang_intra_level);
    // Use Hadamard at MDS0
    ctx->mds0_use_hadamard = false;
    if (allintra) {
        if (pd_pass == PD_PASS_0) {
            ctx->mds0_use_hadamard = false;
        } else if (enc_mode <= ENC_M11) {
            ctx->mds0_use_hadamard = ctx->intra_ctrls.intra_mode_end > DC_PRED;
        } else {
            ctx->mds0_use_hadamard = false;
        }
    } else {
        ctx->mds0_use_hadamard = false;
    }
    set_mds0_controls(ctx, pcs->mds0_level);
    set_subres_controls(ctx, 0);
    if (pd_pass == PD_PASS_0) {
        ctx->parent_cost_bias = 1000;
    } else {
        ctx->parent_cost_bias = 995;
    }
    uint8_t skip_sub_depth_lvl;
    if (pd_pass == PD_PASS_0) {
        skip_sub_depth_lvl = 0;
    } else if (allintra) {
        if (enc_mode <= ENC_M9) {
            skip_sub_depth_lvl = 1;
        } else {
            skip_sub_depth_lvl = 2;
        }
    } else if (sc_class1) {
        if (enc_mode <= ENC_M6) {
            skip_sub_depth_lvl = 0;
        } else {
            skip_sub_depth_lvl = 1;
        }
    } else if (enc_mode <= ENC_M1) {
        skip_sub_depth_lvl = 1;
    } else {
        skip_sub_depth_lvl = 2;
    }

    set_skip_sub_depth_ctrls(&ctx->skip_sub_depth_ctrls, skip_sub_depth_lvl);
    ctx->tune_ssim_level = SSIM_LVL_0;
}
#endif
/*
* return the 4x4 level
Used by svt_aom_sig_deriv_enc_dec and memory allocation
*/
bool svt_aom_get_disallow_4x4_default(EncMode enc_mode) {
    if (enc_mode <= ENC_M2) {
        return false;
    } else {
        return true;
    }
}

bool svt_aom_get_disallow_4x4_rtc(EncMode enc_mode) {
    if (enc_mode <= ENC_M2) {
        return false;
    } else {
        return true;
    }
}

bool svt_aom_get_disallow_4x4_allintra(EncMode enc_mode) {
    if (enc_mode <= ENC_M3) {
        return false;
    } else {
        return true;
    }
}
#if TUNE_STILL_IMAGE
/*
* return the 8x8 level
Used by svt_aom_sig_deriv_enc_dec and memory allocation

The aligned width/height can be either the picture width/height or the SB width/height. For memory
allocation, we should use the picture width/height.
*/
bool svt_aom_get_disallow_8x8_default() {
    return false;
}

bool svt_aom_get_disallow_8x8_rtc(EncMode enc_mode, const uint16_t aligned_width, const uint16_t aligned_height) {
    if (dimensions_require_8x8(aligned_width, aligned_height)) {
        return false;
    }

    if (enc_mode <= ENC_M10) {
        return false;
    } else {
        return true;
    }
}

bool svt_aom_get_disallow_8x8_allintra() {
    return false;
}
#else
/*
* return the 4x4 level
Used by svt_aom_sig_deriv_enc_dec and memory allocation
*/
bool svt_aom_get_disallow_4x4(EncMode enc_mode) {
    if (enc_mode <= ENC_M2) {
        return false;
    } else {
        return true;
    }
}

/*
* return the 8x8 level
Used by svt_aom_sig_deriv_enc_dec and memory allocation

The aligned width/height can be either the picture width/height or the SB width/height. For memory
allocation, we should use the picture width/height.
*/
bool svt_aom_get_disallow_8x8(EncMode enc_mode, bool allintra, bool rtc_tune, const uint16_t aligned_width,
                              const uint16_t aligned_height) {
    if (dimensions_require_8x8(aligned_width, aligned_height)) {
        return false;
    }
    if (allintra) {
        return false;
    } else if (rtc_tune) {
        if (enc_mode <= ENC_M10) {
            return false;
        } else {
            return true;
        }
    } else {
        return false;
    }
}
#endif
#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_nsq_geom_level_default(EncMode enc_mode, InputCoeffLvl coeff_lvl) {
    uint8_t nsq_geom_level;
    if (enc_mode <= ENC_M0) {
        if (coeff_lvl == HIGH_LVL) {
            nsq_geom_level = 2;
        } else { // regular or low
            nsq_geom_level = 1;
        }
    } else if (enc_mode <= ENC_M5) {
        if (coeff_lvl == HIGH_LVL) {
            nsq_geom_level = 3;
        } else { // regular or low
            nsq_geom_level = 2;
        }
    } else {
        nsq_geom_level = 3;
    }
    return nsq_geom_level;
}

uint8_t svt_aom_get_nsq_geom_level_rtc(EncMode enc_mode) {
    uint8_t nsq_geom_level;
    if (enc_mode <= ENC_M11) {
        nsq_geom_level = 3;
    } else {
        nsq_geom_level = 4;
    }

    return nsq_geom_level;
}

uint8_t svt_aom_get_nsq_geom_level_allintra(EncMode enc_mode) {
    uint8_t nsq_geom_level;
    if (enc_mode <= ENC_M3) {
        nsq_geom_level = 2;
    } else if (enc_mode <= ENC_M5) {
        nsq_geom_level = 3;
    } else {
        nsq_geom_level = 0;
    }
    return nsq_geom_level;
}

uint8_t svt_aom_get_nsq_search_level_default(PictureControlSet* pcs, EncMode enc_mode, InputCoeffLvl coeff_lvl,
                                             uint32_t qp) {
    int nsq_search_level;

    if (enc_mode <= ENC_M0) {
        const uint8_t is_base = pcs->ppcs->temporal_layer_index == 0;
        nsq_search_level      = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M2) {
        nsq_search_level = 7;
    } else if (enc_mode <= ENC_M3) {
        nsq_search_level = 9;
    } else if (enc_mode <= ENC_M4) {
        nsq_search_level = 12;
    } else if (enc_mode <= ENC_M6) {
        nsq_search_level = 15;
    } else if (enc_mode <= ENC_M7) {
        nsq_search_level = 18;
    } else {
        nsq_search_level = 19;
    }

    // If NSQ search is off, don't apply offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

#define NSQ_MODULATION_MIN_LEVEL 8
    if (nsq_search_level > NSQ_MODULATION_MIN_LEVEL) {
        if (pcs->ppcs->r0_gen) {
            double r0_tab[MAX_TEMPORAL_LAYERS] = {0.10, 0.15, 0.20, 0.25, 0.25, 0.25};
            double r0_th                       = pcs->slice_type == I_SLICE ? 0.05 : r0_tab[pcs->temporal_layer_index];
            if (pcs->ppcs->r0 < r0_th) {
                nsq_search_level = MIN(nsq_search_level, MAX(NSQ_MODULATION_MIN_LEVEL, MAX(nsq_search_level - 4, 1)));
            }
        }
    }

    if (nsq_search_level > NSQ_MODULATION_MIN_LEVEL) {
        if (pcs->ppcs->sc_class3) {
            nsq_search_level = MIN(nsq_search_level, MAX(NSQ_MODULATION_MIN_LEVEL, MAX(nsq_search_level - 2, 1)));
        }
    }

    // offset level based on coeff_lvl
    if (coeff_lvl == HIGH_LVL) {
        nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
    } else if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
        nsq_search_level = MAX(nsq_search_level - 3, 1);
    }

    // If NSQ search is off, don't apply QP offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

    const uint8_t seq_qp_mod = pcs->scs->seq_qp_mod;
    // offset level based on sequence QP
    if (seq_qp_mod) {
        if (enc_mode <= ENC_M6) {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 45) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        } else {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        }
    }
    return nsq_search_level;
}

uint8_t svt_aom_get_nsq_search_level_rtc(PictureControlSet* pcs, EncMode enc_mode, InputCoeffLvl coeff_lvl,
                                         uint32_t qp) {
    int nsq_search_level;

    if (enc_mode <= ENC_M0) {
        const uint8_t is_base = pcs->ppcs->temporal_layer_index == 0;
        nsq_search_level      = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M2) {
        nsq_search_level = 7;
    } else if (enc_mode <= ENC_M3) {
        nsq_search_level = 9;
    } else if (enc_mode <= ENC_M4) {
        nsq_search_level = 12;
    } else if (enc_mode <= ENC_M6) {
        nsq_search_level = 15;
    } else {
        nsq_search_level = 19;
    }

    // If NSQ search is off, don't apply offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

#define NSQ_MODULATION_MIN_LEVEL 8
    if (nsq_search_level > NSQ_MODULATION_MIN_LEVEL) {
        if (pcs->ppcs->r0_gen) {
            double r0_tab[MAX_TEMPORAL_LAYERS] = {0.10, 0.15, 0.20, 0.25, 0.25, 0.25};
            double r0_th                       = pcs->slice_type == I_SLICE ? 0.05 : r0_tab[pcs->temporal_layer_index];
            if (pcs->ppcs->r0 < r0_th) {
                nsq_search_level = MIN(nsq_search_level, MAX(NSQ_MODULATION_MIN_LEVEL, MAX(nsq_search_level - 4, 1)));
            }
        }
    }

    if (nsq_search_level > NSQ_MODULATION_MIN_LEVEL) {
        if (pcs->ppcs->sc_class3) {
            nsq_search_level = MIN(nsq_search_level, MAX(NSQ_MODULATION_MIN_LEVEL, MAX(nsq_search_level - 2, 1)));
        }
    }

    // offset level based on coeff_lvl
    if (coeff_lvl == HIGH_LVL) {
        nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
    } else if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
        nsq_search_level = MAX(nsq_search_level - 3, 1);
    }

    // If NSQ search is off, don't apply QP offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

    const uint8_t seq_qp_mod = pcs->scs->seq_qp_mod;
    // offset level based on sequence QP
    if (seq_qp_mod) {
        if (enc_mode <= ENC_M6) {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 45) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        } else {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        }
    }
    return nsq_search_level;
}

uint8_t svt_aom_get_nsq_search_level_allintra(PictureControlSet* pcs, EncMode enc_mode, uint32_t qp) {
    int nsq_search_level;

    if (enc_mode <= ENC_M0) {
        nsq_search_level = 3;
    } else if (enc_mode <= ENC_M1) {
        nsq_search_level = 10;
    } else if (enc_mode <= ENC_M2) {
        nsq_search_level = 14;
    } else if (enc_mode <= ENC_M3) {
        nsq_search_level = 16;
    } else if (enc_mode <= ENC_M4) {
        nsq_search_level = 17;
    } else {
        nsq_search_level = 0;
    }

    // If NSQ search is off, don't apply offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

    const uint8_t seq_qp_mod = pcs->scs->seq_qp_mod;
    // offset level based on sequence QP
    if (seq_qp_mod) {
        if (enc_mode <= ENC_M5) {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 45) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        } else {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        }
    }
    return nsq_search_level;
}
#else
uint8_t svt_aom_get_nsq_geom_level(bool allintra, ResolutionRange input_resolution, EncMode enc_mode,
                                   InputCoeffLvl coeff_lvl, bool rtc) {
    uint8_t nsq_geom_level;
    if (allintra) {
        if (input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_MR) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 2;
                } else { // regular or low
                    nsq_geom_level = 1;
                }
            } else if (enc_mode <= ENC_M3) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 3;
                } else { // regular or low
                    nsq_geom_level = 2;
                }
            } else if (enc_mode <= ENC_M7) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 4;
                } else { // regular or low
                    nsq_geom_level = 3;
                }
            } else {
                nsq_geom_level = 0;
            }
        } else {
            if (enc_mode <= ENC_M1) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 2;
                } else { // regular or low
                    nsq_geom_level = 1;
                }
            } else if (enc_mode <= ENC_M9) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 3;
                } else { // regular or low
                    nsq_geom_level = 2;
                }
            } else {
                nsq_geom_level = 0;
            }
        }

    } else {
        if (rtc) {
            if (enc_mode <= ENC_M11) {
                nsq_geom_level = 3;
            } else {
                nsq_geom_level = 4;
            }
        } else {
            if (enc_mode <= ENC_M0) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 2;
                } else { // regular or low
                    nsq_geom_level = 1;
                }
            } else if (enc_mode <= ENC_M5) {
                if (coeff_lvl == HIGH_LVL) {
                    nsq_geom_level = 3;
                } else { // regular or low
                    nsq_geom_level = 2;
                }
            } else {
                nsq_geom_level = 3;
            }
        }
    }
    return nsq_geom_level;
}

uint8_t svt_aom_get_nsq_search_level(PictureControlSet* pcs, EncMode enc_mode, InputCoeffLvl coeff_lvl, uint32_t qp) {
    int        nsq_search_level;
    const bool rtc_tune = pcs->scs->static_config.rtc;
    if (pcs->scs->allintra) {
        if (pcs->scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_MR) {
                nsq_search_level = 3;
            } else if (enc_mode <= ENC_M0) {
                nsq_search_level = 10;
            } else if (enc_mode <= ENC_M3) {
                nsq_search_level = 15;
            } else if (enc_mode <= ENC_M5) {
                nsq_search_level = 16;
            } else if (enc_mode <= ENC_M9) {
                nsq_search_level = 17;
            } else {
                nsq_search_level = 0;
            }
        } else {
            if (enc_mode <= ENC_MR) {
                nsq_search_level = 3;
            } else if (enc_mode <= ENC_M0) {
                nsq_search_level = 4;
            } else if (enc_mode <= ENC_M2) {
                nsq_search_level = 7;
            } else if (enc_mode <= ENC_M3) {
                nsq_search_level = 12;
            } else if (enc_mode <= ENC_M6) {
                nsq_search_level = 13;
            } else if (enc_mode <= ENC_M7) {
                nsq_search_level = 15;
            } else if (enc_mode <= ENC_M9) {
                nsq_search_level = 17;
            } else {
                nsq_search_level = 0;
            }
        }
    } else if (enc_mode <= ENC_M0) {
        const uint8_t is_base = pcs->ppcs->temporal_layer_index == 0;
        nsq_search_level      = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M2) {
        nsq_search_level = 7;
    } else if (enc_mode <= ENC_M3) {
        nsq_search_level = 9;
    } else if (enc_mode <= ENC_M4) {
        nsq_search_level = 12;
    } else if (enc_mode <= ENC_M6) {
        nsq_search_level = 15;
    } else if (!rtc_tune && enc_mode <= ENC_M7) {
        nsq_search_level = 18;
    } else {
        nsq_search_level = 19;
    }

    // If NSQ search is off, don't apply offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

#define NSQ_MODULATION_MIN_LEVEL 8
    if (nsq_search_level > NSQ_MODULATION_MIN_LEVEL) {
        if (pcs->ppcs->r0_gen) {
            double r0_tab[MAX_TEMPORAL_LAYERS] = {0.10, 0.15, 0.20, 0.25, 0.25, 0.25};
            double r0_th                       = pcs->slice_type == I_SLICE ? 0.05 : r0_tab[pcs->temporal_layer_index];
            if (pcs->ppcs->r0 < r0_th) {
                nsq_search_level = MIN(nsq_search_level, MAX(NSQ_MODULATION_MIN_LEVEL, MAX(nsq_search_level - 4, 1)));
            }
        }
    }

    if (nsq_search_level > NSQ_MODULATION_MIN_LEVEL) {
        if (pcs->ppcs->sc_class3) {
            nsq_search_level = MIN(nsq_search_level, MAX(NSQ_MODULATION_MIN_LEVEL, MAX(nsq_search_level - 2, 1)));
        }
    }

    // offset level based on coeff_lvl
    if (coeff_lvl == HIGH_LVL) {
        nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
    } else if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
        nsq_search_level = MAX(nsq_search_level - 3, 1);
    }

    // If NSQ search is off, don't apply QP offsets
    if (nsq_search_level == 0) {
        return nsq_search_level;
    }

    const uint8_t seq_qp_mod = pcs->scs->seq_qp_mod;
    // offset level based on sequence QP
    if (seq_qp_mod) {
        if (enc_mode <= ENC_M6) {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 45) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        } else {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39) {
                nsq_search_level = nsq_search_level + 3 > 19 ? 0 : nsq_search_level + 3;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43) {
                nsq_search_level = nsq_search_level + 2 > 19 ? 0 : nsq_search_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48) {
                nsq_search_level = nsq_search_level + 1 > 19 ? 0 : nsq_search_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56) {
                nsq_search_level = MAX(nsq_search_level - 1, 1);
            }
        }
    }
    return nsq_search_level;
}
#endif
/*
* return by-pass encdec
*/
#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_bypass_encdec_default(EncMode enc_mode, uint8_t encoder_bit_depth) {
    uint8_t bypass_encdec = 1;
    if (encoder_bit_depth == EB_EIGHT_BIT) {
        // 8bit settings
        if (enc_mode <= ENC_M2) {
            bypass_encdec = 0;
        } else {
            bypass_encdec = 1;
        }
    } else {
        // 10bit settings
        if (enc_mode <= ENC_M7) {
            bypass_encdec = 0;
        } else {
            bypass_encdec = 1;
        }
    }
    return bypass_encdec;
}

uint8_t svt_aom_get_bypass_encdec_rtc(EncMode enc_mode, uint8_t encoder_bit_depth) {
    uint8_t bypass_encdec = 1;
    if (encoder_bit_depth == EB_EIGHT_BIT) {
        // 8bit settings
        if (enc_mode <= ENC_M2) {
            bypass_encdec = 0;
        } else {
            bypass_encdec = 1;
        }
    } else {
        // 10bit settings
        if (enc_mode <= ENC_M7) {
            bypass_encdec = 0;
        } else {
            bypass_encdec = 1;
        }
    }
    return bypass_encdec;
}

uint8_t svt_aom_get_bypass_encdec_allintra(EncMode enc_mode) {
    uint8_t bypass_encdec = 1;
    if (enc_mode <= ENC_M3) {
        bypass_encdec = 0;
    } else {
        bypass_encdec = 1;
    }
    return bypass_encdec;
}
#else
uint8_t svt_aom_get_bypass_encdec(EncMode enc_mode, uint8_t encoder_bit_depth) {
    uint8_t bypass_encdec = 1;
    if (encoder_bit_depth == EB_EIGHT_BIT) {
        // 8bit settings
        if (enc_mode <= ENC_M2) {
            bypass_encdec = 0;
        } else {
            bypass_encdec = 1;
        }
    } else {
        // 10bit settings
        if (enc_mode <= ENC_M7) {
            bypass_encdec = 0;
        } else {
            bypass_encdec = 1;
        }
    }
    return bypass_encdec;
}
#endif

static void set_cdf_controls(PictureControlSet* pcs, uint8_t update_cdf_level) {
    CdfControls* ctrl = &pcs->cdf_ctrl;
    switch (update_cdf_level) {
    case 0:
        ctrl->update_mv   = 0;
        ctrl->update_se   = 0;
        ctrl->update_coef = 0;
        break;
    case 1:
        ctrl->update_mv   = 1;
        ctrl->update_se   = 1;
        ctrl->update_coef = 1;
        break;
    case 2:
        ctrl->update_mv   = 0;
        ctrl->update_se   = 1;
        ctrl->update_coef = 1;
        break;
    case 3:
        ctrl->update_mv   = 0;
        ctrl->update_se   = 1;
        ctrl->update_coef = 0;
        break;
    default:
        assert(0);
        break;
    }

    ctrl->update_mv = pcs->slice_type == I_SLICE ? 0 : ctrl->update_mv;
    ctrl->enabled   = ctrl->update_coef | ctrl->update_mv | ctrl->update_se;
}

/******************************************************
* Derive Mode Decision Config Settings for OQ
Input   : encoder mode and tune
Output  : EncDec Kernel signal(s)
******************************************************/
static EbErrorType rtime_alloc_ec_ctx_array(PictureControlSet* pcs, uint16_t all_sb) {
    EB_MALLOC_ARRAY(pcs->ec_ctx_array, all_sb);
    return EB_ErrorNone;
}

#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_update_cdf_level_default(EncMode enc_mode, SliceType is_islice, uint8_t is_base,
                                             uint8_t sc_class1) {
    uint8_t update_cdf_level;
    if (enc_mode <= ENC_M0) {
        update_cdf_level = 1;
    } else if ((!sc_class1 && enc_mode <= ENC_M3) || (sc_class1 && enc_mode <= ENC_M4)) {
        update_cdf_level = is_base ? 1 : 2;
    } else if (enc_mode <= ENC_M8) {
        update_cdf_level = is_islice ? 1 : 0;
    } else {
        update_cdf_level = 0;
    }
    return update_cdf_level;
}

uint8_t svt_aom_get_update_cdf_level_rtc(EncMode enc_mode, SliceType is_islice, uint8_t is_base, uint8_t sc_class1) {
    uint8_t update_cdf_level;
    if (enc_mode <= ENC_M0) {
        update_cdf_level = 1;
    } else if ((!sc_class1 && enc_mode <= ENC_M3) || (sc_class1 && enc_mode <= ENC_M4)) {
        update_cdf_level = is_base ? 1 : 2;
    } else if (enc_mode <= ENC_M8) {
        update_cdf_level = is_islice ? 1 : 0;
    } else {
        update_cdf_level = 0;
    }
    return update_cdf_level;
}

uint8_t svt_aom_get_update_cdf_level_allintra(EncMode enc_mode) {
    uint8_t update_cdf_level;
    if (enc_mode <= ENC_M3) {
        update_cdf_level = 1;
    } else if (enc_mode <= ENC_M6) {
        update_cdf_level = 2;
    } else {
        update_cdf_level = 0;
    }

    return update_cdf_level;
}
#else
uint8_t svt_aom_get_update_cdf_level(EncMode enc_mode, SliceType is_islice, uint8_t is_base, uint8_t sc_class1,
                                     const EbInputResolution input_resolution, bool allintra) {
    uint8_t update_cdf_level = 0;
    if (allintra) {
        if (input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_M3) {
                update_cdf_level = 1;
            } else if (enc_mode <= ENC_M8) {
                update_cdf_level = 2;
            } else {
                update_cdf_level = 0;
            }
        } else {
            if (enc_mode <= ENC_M3) {
                update_cdf_level = 1;
            } else if (enc_mode <= ENC_M5) {
                update_cdf_level = 2;
            } else if (enc_mode <= ENC_M9) {
                update_cdf_level = 3;
            } else {
                update_cdf_level = 0;
            }
        }
    } else {
        if (enc_mode <= ENC_M0) {
            update_cdf_level = 1;
        } else if ((!sc_class1 && enc_mode <= ENC_M3) || (sc_class1 && enc_mode <= ENC_M4)) {
            update_cdf_level = is_base ? 1 : 2;
        } else if (enc_mode <= ENC_M8) {
            update_cdf_level = is_islice ? 1 : 0;
        } else {
            update_cdf_level = 0;
        }
    }
    return update_cdf_level;
}
#endif
#if TUNE_STILL_IMAGE
uint8_t svt_aom_get_chroma_level_default(EncMode enc_mode, const uint8_t is_islice) {
    uint8_t chroma_level;

    if (enc_mode <= ENC_MR) {
        chroma_level = 1;
    } else if (enc_mode <= ENC_M0) {
        chroma_level = is_islice ? 1 : 4;
    } else if (enc_mode <= ENC_M5) {
        chroma_level = 4;
    } else {
        chroma_level = 5;
    }

    return chroma_level;
}

uint8_t svt_aom_get_chroma_level_rtc(EncMode enc_mode, const uint8_t is_islice) {
    uint8_t chroma_level;

    if (enc_mode <= ENC_MR) {
        chroma_level = 1;
    } else if (enc_mode <= ENC_M0) {
        chroma_level = is_islice ? 1 : 4;
    } else if (enc_mode <= ENC_M5) {
        chroma_level = 4;
    } else {
        chroma_level = 5;
    }

    return chroma_level;
}

uint8_t svt_aom_get_chroma_level_allintra(EncMode enc_mode) {
    uint8_t chroma_level;

    if (enc_mode <= ENC_M0) {
        chroma_level = 1;
    } else if (enc_mode <= ENC_M1) {
        chroma_level = 2;
    } else if (enc_mode <= ENC_M6) {
        chroma_level = 4;
    } else {
        chroma_level = 5;
    }

    return chroma_level;
}
#else
uint8_t svt_aom_get_chroma_level(EncMode enc_mode, const uint8_t is_islice, bool allintra) {
    uint8_t chroma_level = 0;
    if (allintra) {
        if (enc_mode <= ENC_MR) {
            chroma_level = 1;
        } else if (enc_mode <= ENC_M1) {
            chroma_level = 2;
        } else if (enc_mode <= ENC_M7) {
            chroma_level = 4;
        } else {
            chroma_level = 5;
        }
    } else { //DNC
        if (enc_mode <= ENC_MR) {
            chroma_level = 1;
        } else if (enc_mode <= ENC_M0) {
            chroma_level = is_islice ? 1 : 4;
        } else if (enc_mode <= ENC_M5) {
            chroma_level = 4;
        } else {
            chroma_level = 5;
        }
    }
    return chroma_level;
}
#endif

/*
set lpd0_level
*/
#if TUNE_STILL_IMAGE
static void set_pic_lpd0_lvl_default(PictureControlSet* pcs, EncMode enc_mode) {
    PictureParentControlSet* ppcs               = pcs->ppcs;
    const uint8_t            is_base            = ppcs->temporal_layer_index == 0;
    const uint8_t            is_islice          = pcs->slice_type == I_SLICE;
    const bool               transition_present = (ppcs->transition_present == 1);
    InputCoeffLvl            coeff_lvl          = pcs->coeff_lvl;
    const EbInputResolution  input_resolution   = ppcs->input_resolution;
    uint8_t                  ldp0_lvl_offset[4] = {2, 2, 1, 0};
    uint8_t                  qp_band_idx        = 0;
    const uint8_t            seq_qp_mod         = pcs->scs->seq_qp_mod;

    if (pcs->scs->static_config.qp <= 27) {
        qp_band_idx = 0;
    } else if (pcs->scs->static_config.qp <= 39) {
        qp_band_idx = 1;
    } else if (pcs->scs->static_config.qp <= 43) {
        qp_band_idx = 2;
    } else {
        qp_band_idx = 3;
    }

    if (enc_mode <= ENC_M3) {
        pcs->pic_lpd0_lvl = 0;
    } else if (enc_mode <= ENC_M4) {
        pcs->pic_lpd0_lvl = 1;
    } else if (enc_mode <= ENC_M7) {
        if (input_resolution <= INPUT_SIZE_360p_RANGE) {
            pcs->pic_lpd0_lvl = 3;
        } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
            pcs->pic_lpd0_lvl = (is_base || transition_present) ? 3 : 5;
        } else {
            if (coeff_lvl == HIGH_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? 7 : 8;
            } else if (coeff_lvl == NORMAL_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? 4 : 6;
            } else {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? 3 : 5;
            }
        }
    } else if (enc_mode <= ENC_M8) {
        if (input_resolution <= INPUT_SIZE_360p_RANGE) {
            // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
            const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
            pcs->pic_lpd0_lvl   = MIN(MAX_LDP0_LVL, 3 + qp_offset);
        } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
            // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
            const int qp_offset = (seq_qp_mod <= 1) ? 0 : MAX((int)((int)ldp0_lvl_offset[qp_band_idx] - 1), 0);
            pcs->pic_lpd0_lvl   = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                  : MIN(MAX_LDP0_LVL, 5 + qp_offset);
        } else {
            // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
            const int qp_offset = (seq_qp_mod <= 1) ? 0 : MAX((int)((int)ldp0_lvl_offset[qp_band_idx] - 1), 0);
            if (coeff_lvl == HIGH_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 7 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 8 + qp_offset);
            } else if (coeff_lvl == NORMAL_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 7 + qp_offset);
            } else {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 5 + qp_offset);
            }
        }
    } else if (enc_mode <= ENC_M10) {
        if (input_resolution <= INPUT_SIZE_360p_RANGE) {
            // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
            const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
            if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 5 + qp_offset);
            } else if (coeff_lvl == NORMAL_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 4 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 6 + qp_offset);
            } else {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 7 + qp_offset);
            }
        } else {
            // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
            const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
            if (coeff_lvl == HIGH_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 7 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 8 + qp_offset);
            } else if (coeff_lvl == NORMAL_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 7 + qp_offset);
            } else {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 5 + qp_offset);
            }
        }
    } else {
        if (input_resolution <= INPUT_SIZE_360p_RANGE) {
            // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
            const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
            if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 5 + qp_offset);
            } else if (coeff_lvl == NORMAL_LVL) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 4 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 6 + qp_offset);
            } else {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                    : MIN(MAX_LDP0_LVL, 7 + qp_offset);
            }
        } else {
            if (coeff_lvl == HIGH_LVL) {
                pcs->pic_lpd0_lvl = 7;
            } else {
                pcs->pic_lpd0_lvl = (is_islice || transition_present) ? 6 : 7;
            }
        }
    }

    if (pcs->scs->super_block_size == 128) {
        pcs->pic_lpd0_lvl = 0;
    }
}

static void set_pic_lpd0_lvl_rtc(PictureControlSet* pcs, EncMode enc_mode) {
    PictureParentControlSet* ppcs               = pcs->ppcs;
    const uint8_t            is_base            = ppcs->temporal_layer_index == 0;
    const uint8_t            is_islice          = pcs->slice_type == I_SLICE;
    const bool               transition_present = (ppcs->transition_present == 1);
    const uint8_t            sc_class1          = ppcs->sc_class1;
    const EbInputResolution  input_resolution   = ppcs->input_resolution;

    if (sc_class1) {
        if (enc_mode <= ENC_M9) {
            pcs->pic_lpd0_lvl = is_base ? 5 : 7;
        } else {
            pcs->pic_lpd0_lvl = is_islice ? 5 : 7;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd0_lvl = is_islice ? 0 : 1;
            } else {
                pcs->pic_lpd0_lvl = is_base ? 3 : 4;
            }
        } else if (enc_mode <= ENC_M8) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd0_lvl = is_islice ? 1 : 3;
            } else {
                pcs->pic_lpd0_lvl = is_base ? 3 : 5;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->pic_lpd0_lvl = is_base ? 5 : 7;
        } else {
            pcs->pic_lpd0_lvl = (is_islice || transition_present) ? 6 : 7;
        }
    }

    if (pcs->scs->super_block_size == 128) {
        pcs->pic_lpd0_lvl = 0;
    }
}

static void set_pic_lpd0_lvl_allintra(PictureControlSet* pcs, EncMode enc_mode) {
    if (enc_mode <= ENC_M1) {
        pcs->pic_lpd0_lvl = 0;
    } else if (enc_mode <= ENC_M8) {
        pcs->pic_lpd0_lvl = 1;
    } else {
        pcs->pic_lpd0_lvl = 7;
    }
    if (pcs->scs->super_block_size == 128) {
        pcs->pic_lpd0_lvl = 0;
    }
}
#else
static void set_pic_lpd0_lvl(PictureControlSet* pcs, EncMode enc_mode) {
    PictureParentControlSet* ppcs = pcs->ppcs;

    const uint8_t           is_base            = ppcs->temporal_layer_index == 0;
    const uint8_t           is_islice          = pcs->slice_type == I_SLICE;
    const bool              transition_present = (ppcs->transition_present == 1);
    const uint8_t           sc_class1          = ppcs->sc_class1;
    const bool              rtc_tune           = pcs->scs->static_config.rtc;
    InputCoeffLvl           coeff_lvl          = pcs->coeff_lvl;
    const EbInputResolution input_resolution   = ppcs->input_resolution;
    uint8_t                 ldp0_lvl_offset[4] = {2, 2, 1, 0};
    uint8_t                 qp_band_idx        = 0;
    const uint8_t           seq_qp_mod         = pcs->scs->seq_qp_mod;

    if (pcs->scs->static_config.qp <= 27) {
        qp_band_idx = 0;
    } else if (pcs->scs->static_config.qp <= 39) {
        qp_band_idx = 1;
    } else if (pcs->scs->static_config.qp <= 43) {
        qp_band_idx = 2;
    } else {
        qp_band_idx = 3;
    }
    if (pcs->scs->allintra) {
        if (enc_mode <= ENC_M0) {
            pcs->pic_lpd0_lvl = 0;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_lpd0_lvl = 1;
        } else if (enc_mode <= ENC_M11) {
            pcs->pic_lpd0_lvl = 3;
        } else {
            pcs->pic_lpd0_lvl = 6;
        }
    } else if (rtc_tune) {
        if (sc_class1) {
            if (enc_mode <= ENC_M9) {
                pcs->pic_lpd0_lvl = is_base ? 5 : 7;
            } else {
                pcs->pic_lpd0_lvl = is_islice ? 5 : 7;
            }
        } else {
            if (enc_mode <= ENC_M7) {
                if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                    pcs->pic_lpd0_lvl = is_islice ? 0 : 1;
                } else {
                    pcs->pic_lpd0_lvl = is_base ? 3 : 4;
                }
            } else if (enc_mode <= ENC_M8) {
                if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                    pcs->pic_lpd0_lvl = is_islice ? 1 : 3;
                } else {
                    pcs->pic_lpd0_lvl = is_base ? 3 : 5;
                }
            } else if (enc_mode <= ENC_M11) {
                pcs->pic_lpd0_lvl = is_base ? 5 : 7;
            } else {
                pcs->pic_lpd0_lvl = (is_islice || transition_present) ? 6 : 7;
            }
        }
    } else {
        if (enc_mode <= ENC_M3) {
            pcs->pic_lpd0_lvl = 0;
        } else if (enc_mode <= ENC_M4) {
            pcs->pic_lpd0_lvl = 1;
        } else if (enc_mode <= ENC_M7) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd0_lvl = 3;
            } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                pcs->pic_lpd0_lvl = (is_base || transition_present) ? 3 : 5;
            } else {
                if (coeff_lvl == HIGH_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? 7 : 8;
                } else if (coeff_lvl == NORMAL_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? 4 : 6;
                } else {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? 3 : 5;
                }
            }
        } else if (enc_mode <= ENC_M8) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
                const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
                pcs->pic_lpd0_lvl   = MIN(MAX_LDP0_LVL, 3 + qp_offset);
            } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
                const int qp_offset = (seq_qp_mod <= 1) ? 0 : MAX((int)((int)ldp0_lvl_offset[qp_band_idx] - 1), 0);
                pcs->pic_lpd0_lvl   = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                      : MIN(MAX_LDP0_LVL, 5 + qp_offset);
            } else {
                // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
                const int qp_offset = (seq_qp_mod <= 1) ? 0 : MAX((int)((int)ldp0_lvl_offset[qp_band_idx] - 1), 0);
                if (coeff_lvl == HIGH_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 7 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 8 + qp_offset);
                } else if (coeff_lvl == NORMAL_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 7 + qp_offset);
                } else {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 5 + qp_offset);
                }
            }
        } else if (enc_mode <= ENC_M10) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
                const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
                if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 5 + qp_offset);
                } else if (coeff_lvl == NORMAL_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 4 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 6 + qp_offset);
                } else {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 7 + qp_offset);
                }
            } else {
                // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
                const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
                if (coeff_lvl == HIGH_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 7 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 8 + qp_offset);
                } else if (coeff_lvl == NORMAL_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 7 + qp_offset);
                } else {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 5 + qp_offset);
                }
            }
        } else {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                // For seq_qp_mode 3, there is no conservative offset to disallow because the qp offset is limited to at least 0
                const int qp_offset = (seq_qp_mod <= 1) ? 0 : (int)ldp0_lvl_offset[qp_band_idx];
                if (coeff_lvl == VLOW_LVL || coeff_lvl == LOW_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 3 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 5 + qp_offset);
                } else if (coeff_lvl == NORMAL_LVL) {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 4 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 6 + qp_offset);
                } else {
                    pcs->pic_lpd0_lvl = (is_base || transition_present) ? MIN(MAX_LDP0_LVL, 5 + qp_offset)
                                                                        : MIN(MAX_LDP0_LVL, 7 + qp_offset);
                }
            } else {
                if (coeff_lvl == HIGH_LVL) {
                    pcs->pic_lpd0_lvl = 7;
                } else {
                    pcs->pic_lpd0_lvl = (is_islice || transition_present) ? 6 : 7;
                }
            }
        }
    }
    if (pcs->scs->super_block_size == 128) {
        pcs->pic_lpd0_lvl = 0;
    }
}
#endif

uint8_t get_inter_compound_level(EncMode enc_mode) {
    uint8_t comp_level;
    if (enc_mode <= ENC_M0) {
        comp_level = 3;
    } else if (enc_mode <= ENC_M2) {
        comp_level = 4;
    } else {
        comp_level = 0;
    }

    return comp_level;
}

#if TUNE_STILL_IMAGE
uint8_t get_filter_intra_level_default(EncMode enc_mode) {
    uint8_t filter_intra_level;
    if (enc_mode <= ENC_M1) {
        filter_intra_level = 1;
    } else if (enc_mode <= ENC_M5) {
        filter_intra_level = 2;
    } else {
        filter_intra_level = 0;
    }

    return filter_intra_level;
}

uint8_t get_filter_intra_level_rtc(EncMode enc_mode) {
    uint8_t filter_intra_level;
    if (enc_mode <= ENC_M1) {
        filter_intra_level = 1;
    } else if (enc_mode <= ENC_M5) {
        filter_intra_level = 2;
    } else {
        filter_intra_level = 0;
    }

    return filter_intra_level;
}

uint8_t get_filter_intra_level_allintra(EncMode enc_mode) {
    uint8_t filter_intra_level;
    if (enc_mode <= ENC_M0) {
        filter_intra_level = 1;
    } else if (enc_mode <= ENC_M6) {
        filter_intra_level = 2;
    } else {
        filter_intra_level = 0;
    }

    return filter_intra_level;
}
#else
uint8_t get_filter_intra_level(SequenceControlSet* scs, EncMode enc_mode) {
    uint8_t filter_intra_level;
    if (scs->allintra) {
        if (enc_mode <= ENC_MR) {
            filter_intra_level = 1;
        } else if (enc_mode <= ENC_M8) {
            filter_intra_level = 2;
        } else {
            filter_intra_level = 0;
        }
    } else {
        if (enc_mode <= ENC_M1) {
            filter_intra_level = 1;
        } else if (enc_mode <= ENC_M5) {
            filter_intra_level = 2;
        } else {
            filter_intra_level = 0;
        }
    }
    return filter_intra_level;
}
#endif

uint8_t svt_aom_get_inter_intra_level(EncMode enc_mode, uint8_t transition_present) {
    uint8_t inter_intra_level = 0;
    if (enc_mode <= ENC_M1) {
        inter_intra_level = 2;
    } else if (enc_mode <= ENC_M8) {
        inter_intra_level = transition_present ? 2 : 0;
    } else {
        inter_intra_level = 0;
    }
    return inter_intra_level;
}

uint8_t svt_aom_get_obmc_level(EncMode enc_mode, uint32_t qp, uint8_t seq_qp_mod) {
    uint8_t obmc_level = 0;

    if (enc_mode <= ENC_MR) {
        obmc_level = 1;
    } else if (enc_mode <= ENC_M1) {
        obmc_level = 3;
    } else if (enc_mode <= ENC_M5) {
        obmc_level = 5;
    } else if (enc_mode <= ENC_M9) {
        obmc_level = 6;
    } else {
        obmc_level = 0;
    }
    // QP-banding
    if (!(enc_mode <= ENC_M0) && obmc_level && seq_qp_mod) {
        if (enc_mode <= ENC_M3) {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43) {
                obmc_level = obmc_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 53) {
                obmc_level = obmc_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 60) {
                obmc_level = obmc_level == 1 ? 1 : obmc_level - 1;
            }
        } else {
            if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43) {
                obmc_level = obmc_level + 2;
            } else if ((seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 55) {
                obmc_level = obmc_level + 1;
            } else if ((seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59) {
                obmc_level = obmc_level == 1 ? 1 : obmc_level - 1;
            }
        }
    }

    return obmc_level;
}

static void mfmv_controls(PictureControlSet* pcs, uint8_t mfmv_level) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    const uint8_t            is_base = ppcs->temporal_layer_index == 0;
    double                   r0_th   = 0;
    ppcs->frm_hdr.use_ref_frame_mvs  = 0;
    switch (mfmv_level) {
    case 0:
        ppcs->frm_hdr.use_ref_frame_mvs = 0;
        break;
    case 1:
        ppcs->frm_hdr.use_ref_frame_mvs = 1;
        break;
    case 2:
        r0_th = ppcs->scs->tpl ? 0.15 : 0;
        break;
    case 3:
        r0_th = ppcs->scs->tpl ? 0.13 : 0;
        break;
    case 4:
        r0_th = ppcs->scs->tpl ? 0.10 : 0;
        break;
    default:
        assert(0);
        break;
    }

    if (r0_th) {
        if (pcs->ppcs->r0_gen && is_base) {
            if (pcs->ppcs->r0 < r0_th) {
                ppcs->frm_hdr.use_ref_frame_mvs = 1;
            }
        }
        assert(pcs->slice_type != I_SLICE);
        // Maintain using mfmv if at least 1 of the closest refefrace frame(s) has mfmv enabled
        EbReferenceObject* ref_obj_l0 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
        if (ref_obj_l0->is_mfmv_used) {
            ppcs->frm_hdr.use_ref_frame_mvs = 1;
        }
        if (pcs->slice_type == B_SLICE && ppcs->ref_list1_count_try) {
            EbReferenceObject* ref_obj_l1 = (EbReferenceObject*)pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
            if (ref_obj_l1->is_mfmv_used) {
                ppcs->frm_hdr.use_ref_frame_mvs = 1;
            }
        }
    }
}
#if TUNE_STILL_IMAGE
void svt_aom_sig_deriv_mode_decision_config_default(SequenceControlSet* scs, PictureControlSet* pcs) {
    PictureParentControlSet* ppcs                = pcs->ppcs;
    EncMode                  enc_mode            = pcs->enc_mode;
    const uint8_t            is_ref              = ppcs->is_ref;
    const uint8_t            is_base             = ppcs->temporal_layer_index == 0;
    const uint8_t            is_layer1           = ppcs->temporal_layer_index == 1;
    const EbInputResolution  input_resolution    = ppcs->input_resolution;
    const uint8_t            is_islice           = pcs->slice_type == I_SLICE;
    const uint8_t            sc_class1           = ppcs->sc_class1;
    const uint8_t            fast_decode         = scs->static_config.fast_decode;
    const uint32_t           hierarchical_levels = ppcs->hierarchical_levels;
    const bool               transition_present  = (ppcs->transition_present == 1);
    const bool               is_not_last_layer   = !ppcs->is_highest_layer;
    const uint32_t           sq_qp               = scs->static_config.qp;

    //MFMV
    uint8_t mfmv_level = 0;
    if (is_islice || scs->mfmv_enabled == 0 || pcs->ppcs->frm_hdr.error_resilient_mode) {
        mfmv_level = 0;
    } else {
        if (fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE) {
            if (enc_mode <= ENC_MR) {
                mfmv_level = 1;
            } else if (enc_mode <= ENC_M8) {
                mfmv_level = (input_resolution <= INPUT_SIZE_360p_RANGE) ? 1 : 2;
            } else {
                mfmv_level = (input_resolution <= INPUT_SIZE_360p_RANGE) ? 1 : 4;
            }
        } else {
            mfmv_level = 4;
        }
    }
    mfmv_controls(pcs, mfmv_level);

    uint8_t update_cdf_level = svt_aom_get_update_cdf_level_default(enc_mode, is_islice, is_base, sc_class1);
    set_cdf_controls(pcs, update_cdf_level);
    if (pcs->cdf_ctrl.enabled) {
        const uint16_t picture_sb_w = ppcs->picture_sb_width;
        const uint16_t picture_sb_h = ppcs->picture_sb_height;
        const uint16_t all_sb       = picture_sb_w * picture_sb_h;
        rtime_alloc_ec_ctx_array(pcs, all_sb);
    }

    pcs->pic_filter_intra_level = get_filter_intra_level_default(enc_mode);

    if (pcs->enc_mode <= ENC_M8) {
        pcs->ppcs->use_accurate_part_ctx = true;
    } else {
        pcs->ppcs->use_accurate_part_ctx = false;
    }

    FrameHeader* frm_hdr             = &ppcs->frm_hdr;
    frm_hdr->allow_high_precision_mv = (frm_hdr->quantization_params.base_q_idx < HIGH_PRECISION_MV_QTHRESH_0 ||
                                        (pcs->ref_hp_percentage > HIGH_PRECISION_REF_PERC_TH &&
                                         frm_hdr->quantization_params.base_q_idx < HIGH_PRECISION_MV_QTHRESH_1)) &&
            scs->input_resolution <= INPUT_SIZE_480p_RANGE
        ? 1
        : 0;
    // Set Warped Motion level and enabled flag
    pcs->wm_level = 0;
    if (frm_hdr->frame_type == KEY_FRAME || frm_hdr->frame_type == INTRA_ONLY_FRAME || frm_hdr->error_resilient_mode ||
        pcs->ppcs->frame_superres_enabled || pcs->ppcs->frame_resize_enabled) {
        pcs->wm_level = 0;
    } else {
        if (enc_mode <= ENC_M1) {
            pcs->wm_level = 1;
        } else if (enc_mode <= ENC_M3) {
            if (hierarchical_levels <= 3) {
                pcs->wm_level = is_base ? 1 : 3;
            } else {
                pcs->wm_level = (is_base || is_layer1) ? 2 : 3;
            }
        } else if (enc_mode <= ENC_M9) {
            if (input_resolution <= INPUT_SIZE_720p_RANGE) {
                pcs->wm_level = is_base ? 3 : 0;
            } else {
                pcs->wm_level = is_base ? 4 : 0;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->wm_level = is_base ? 4 : 0;
        } else {
            pcs->wm_level = 0;
        }
    }
    if (hierarchical_levels <= 2) {
        pcs->wm_level = enc_mode <= ENC_M6 ? pcs->wm_level : 0;
    }
    if (enc_mode <= ENC_M7 && scs->seq_qp_mod) {
        if (sq_qp > 55 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->wm_level = pcs->wm_level == 1 ? pcs->wm_level : pcs->wm_level == 0 ? MAX_WARP_LVL : pcs->wm_level - 1;
        }
    }

    bool enable_wm = pcs->wm_level ? 1 : 0;
    // Note: local warp should be disabled when super-res or resize is ON
    // according to the AV1 spec 5.11.27
    frm_hdr->allow_warped_motion = enable_wm &&
        !(frm_hdr->frame_type == KEY_FRAME || frm_hdr->frame_type == INTRA_ONLY_FRAME) &&
        !frm_hdr->error_resilient_mode && !pcs->ppcs->frame_superres_enabled &&
        scs->static_config.resize_mode == RESIZE_NONE;

    frm_hdr->is_motion_mode_switchable = frm_hdr->allow_warped_motion;
    ppcs->pic_obmc_level               = svt_aom_get_obmc_level(enc_mode, sq_qp, scs->seq_qp_mod);
    // Switchable Motion Mode
    frm_hdr->is_motion_mode_switchable = frm_hdr->is_motion_mode_switchable || ppcs->pic_obmc_level;

    if (enc_mode <= ENC_M9) {
        pcs->approx_inter_rate = 0;
    } else {
        pcs->approx_inter_rate = 1;
    }

    if (is_islice || transition_present) {
        pcs->skip_intra = 0;
    } else {
        if (enc_mode <= ENC_M1) {
            pcs->skip_intra = 0;
        } else {
            pcs->skip_intra = (is_ref || pcs->ref_intra_percentage > 50) ? 0 : 1;
        }
    }
    uint32_t intra_level                = 0;
    uint32_t dist_based_ang_intra_level = 0;

    svt_aom_get_intra_mode_levels_default(
        enc_mode, is_islice, is_base, transition_present, &intra_level, &dist_based_ang_intra_level);

    pcs->intra_level                = intra_level;
    pcs->dist_based_ang_intra_level = dist_based_ang_intra_level;
    // Set the level for the candidate(s) reduction feature
    pcs->cand_reduction_level = 0;
    if (is_islice) {
        pcs->cand_reduction_level = 0;
    } else if (enc_mode <= ENC_MR) {
        pcs->cand_reduction_level = 0;
    } else if (enc_mode <= ENC_M2) {
        pcs->cand_reduction_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M7) {
        pcs->cand_reduction_level = 1;
    } else {
        pcs->cand_reduction_level = 2;
    }

    if (scs->rc_stat_gen_pass_mode) {
        pcs->cand_reduction_level = 6;
    }

    // Set the level for the txt search
    pcs->txt_level = 0;
    if (enc_mode <= ENC_MR) {
        pcs->txt_level = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M2) {
        pcs->txt_level = is_base ? 2 : 5;
    } else if (enc_mode <= ENC_M10) {
        pcs->txt_level = is_base ? 7 : 9;
    } else if (enc_mode <= ENC_M11) {
        pcs->txt_level = 10;
    } else {
        pcs->txt_level = 0;
    }
    // Set the level for the txt shortcut feature
    // Any tx_shortcut_level having the chroma detector off in REF frames should be reserved for M13+
    pcs->tx_shortcut_level = 0;

    if (enc_mode <= ENC_M2) {
        pcs->tx_shortcut_level = 0;
    } else if (enc_mode <= ENC_M9) {
        pcs->tx_shortcut_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M10) {
        pcs->tx_shortcut_level = is_base ? 1 : 2;
    } else {
        pcs->tx_shortcut_level = is_islice ? 1 : 3;
    }

    // Set RDOQ level
    if (enc_mode <= ENC_M10) {
        pcs->rdoq_level = 1;
    } else {
        pcs->rdoq_level = 2;
    }

    pcs->rate_est_level = 1;

    // Set the level the interpolation search
    pcs->interpolation_search_level = 0;
    if (enc_mode <= ENC_MR) {
        pcs->interpolation_search_level = 2;
    } else if (enc_mode <= ENC_M8) {
        pcs->interpolation_search_level = 4;
    } else {
        pcs->interpolation_search_level = 4;
        if (!is_base) {
            const uint8_t th[INPUT_SIZE_COUNT] = {100, 100, 85, 50, 30, 30, 30};
            const uint8_t skip_area            = pcs->ref_skip_percentage;
            if (skip_area > th[input_resolution]) {
                pcs->interpolation_search_level = 0;
            }
        }
    }
    frm_hdr->interpolation_filter = SWITCHABLE;

    pcs->chroma_level = svt_aom_get_chroma_level_default(enc_mode, is_islice);

    // Set the level for cfl
    pcs->cfl_level = 0;
    if (sc_class1) { //DNC
        if (enc_mode <= ENC_M6) {
            pcs->cfl_level = 1;
        } else {
            pcs->cfl_level = is_base ? 2 : 0;
        }
    } else {
        if (enc_mode <= ENC_M1) {
            pcs->cfl_level = 1;
        } else if (enc_mode <= ENC_M9) {
            pcs->cfl_level = is_base ? 2 : 0;
        } else if (enc_mode <= ENC_M10) {
            pcs->cfl_level = is_islice ? 2 : 0;
        } else {
            pcs->cfl_level = 0;
        }
    }

    // Set the level for new/nearest/near injection
    if (enc_mode <= ENC_MR) {
        pcs->new_nearest_near_comb_injection = 1;
    } else if (enc_mode <= ENC_M1) {
        pcs->new_nearest_near_comb_injection = is_base ? 2 : 0;
    } else {
        pcs->new_nearest_near_comb_injection = 0;
    }

    // Set the level for unipred3x3 injection
    if (enc_mode <= ENC_MR) {
        pcs->unipred3x3_injection = 1;
    } else {
        pcs->unipred3x3_injection = 0;
    }

    // Set the level for bipred3x3 injection

    if (enc_mode <= ENC_M0) {
        pcs->bipred3x3_injection = 1;
    } else if (enc_mode <= ENC_M1) {
        pcs->bipred3x3_injection = 2;
    } else {
        pcs->bipred3x3_injection = 0;
    }

    // Set the level for inter-inter compound
    pcs->inter_compound_mode = get_inter_compound_level(enc_mode);

    // Set the level for the distance-based red pruning
    if (pcs->ppcs->ref_list0_count_try > 1 || pcs->ppcs->ref_list1_count_try > 1) {
        if (enc_mode <= ENC_MR) {
            pcs->dist_based_ref_pruning = 0;
        } else if (enc_mode <= ENC_M2) {
            pcs->dist_based_ref_pruning = is_base ? 1 : 5;
        } else if (enc_mode <= ENC_M8) {
            pcs->dist_based_ref_pruning = is_base ? 2 : 5;
        } else if (enc_mode <= ENC_M11) {
            pcs->dist_based_ref_pruning = is_base ? 2 : 5;
        } else {
            pcs->dist_based_ref_pruning = 8;
        }
    } else {
        pcs->dist_based_ref_pruning = 0;
    }

    // Set the level the spatial sse @ full-loop
    if (enc_mode <= ENC_M2) {
        pcs->spatial_sse_full_loop_level = 1;
    } else {
        pcs->spatial_sse_full_loop_level = 3;
    }

    //set the nsq_level
    pcs->nsq_geom_level   = svt_aom_get_nsq_geom_level_default(enc_mode, pcs->coeff_lvl);
    pcs->nsq_search_level = svt_aom_get_nsq_search_level_default(pcs, enc_mode, pcs->coeff_lvl, scs->static_config.qp);

    // Set the level for inter-intra level
    if (!is_islice && scs->seq_header.enable_interintra_compound) {
        pcs->inter_intra_level = svt_aom_get_inter_intra_level(enc_mode, transition_present);
    } else {
        pcs->inter_intra_level = 0;
    }

    if (enc_mode <= ENC_M1) {
        pcs->txs_level = 2;
    } else if (enc_mode <= ENC_M8) {
        pcs->txs_level = is_base ? 3 : 0;
    } else if (enc_mode <= ENC_M9) {
        pcs->txs_level = is_base ? 4 : 0;
    } else if (enc_mode <= ENC_M10) {
        pcs->txs_level = is_islice ? 4 : 0;
    } else {
        pcs->txs_level = 0;
    }

    // QP-banding
    if (pcs->txs_level && scs->seq_qp_mod) {
        if (sq_qp > 58 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->txs_level = pcs->txs_level == 1 ? pcs->txs_level : pcs->txs_level - 1;
        }
    }
    // Set tx_mode for the frame header
    frm_hdr->tx_mode = (pcs->txs_level) ? TX_MODE_SELECT : TX_MODE_LARGEST;
    // Set the level for nic
    pcs->nic_level = svt_aom_get_nic_level_default(enc_mode, is_base, sc_class1);

    // Set the level for SQ me-search
    pcs->md_sq_mv_search_level = 0;

    // Set the level for NSQ me-search
    pcs->md_nsq_mv_search_level = 2;
    // Set the level for PME search
    if (enc_mode <= ENC_MR) {
        pcs->md_pme_level = 1;
    } else if (enc_mode <= ENC_M0) {
        pcs->md_pme_level = 2;
    } else if (enc_mode <= ENC_M5) {
        pcs->md_pme_level = 3;
    } else if (enc_mode <= ENC_M10) {
        pcs->md_pme_level = 4;
    } else {
        pcs->md_pme_level = 0;
    }
    // Set the level for mds0
    pcs->mds0_level = 0;
    if (enc_mode <= ENC_M2) {
        pcs->mds0_level = 0;
    } else if (!sc_class1 && enc_mode <= ENC_M5) {
        pcs->mds0_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M10) {
        pcs->mds0_level = is_islice ? 0 : 2;
    } else {
        pcs->mds0_level = 2;
    }
    /*
    disallow_4x4
    */
#if TUNE_STILL_IMAGE
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4_default(enc_mode);
#else
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4(enc_mode);
#endif
    /*
    Bypassing EncDec
    */
    // This signal can only be modified per picture right now, not per SB.  Per SB requires
    // neighbour array updates at EncDec for all SBs, that are currently skipped if EncDec is bypassed.
    if (!ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
#if TUNE_STILL_IMAGE
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec_default(enc_mode, scs->static_config.encoder_bit_depth);
#else
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec(enc_mode, scs->static_config.encoder_bit_depth);
#endif
    } else {
        pcs->pic_bypass_encdec = 0;
    }

    /*
    set lpd0_level
    */
    set_pic_lpd0_lvl_default(pcs, enc_mode);

    if (transition_present) {
        pcs->pic_depth_removal_level = 0;
    } else {
        // Set depth_removal_level_controls
        if (sc_class1) {
            if (enc_mode <= ENC_M6) {
                pcs->pic_depth_removal_level = 0;
            } else if (enc_mode <= ENC_M9) {
                pcs->pic_depth_removal_level = is_base ? 0 : 6;
            } else {
                pcs->pic_depth_removal_level = is_base ? 5 : 14;
            }
        } else {
            if (enc_mode <= ENC_M1) {
                pcs->pic_depth_removal_level = 0;
            } else if (enc_mode <= ENC_M5) {
                if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                    if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 1 : 2;
                    } else if (pcs->coeff_lvl == HIGH_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 3 : 5;
                    } else {
                        pcs->pic_depth_removal_level = is_base ? 3 : 4;
                    }
                } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                    if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 1 : 2;
                    } else if (pcs->coeff_lvl == HIGH_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 3 : 6;
                    } else {
                        pcs->pic_depth_removal_level = is_base ? 3 : 5;
                    }
                } else {
                    if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 1 : 3;
                    } else if (pcs->coeff_lvl == HIGH_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 4 : 8;
                    } else {
                        pcs->pic_depth_removal_level = is_base ? 4 : 7;
                    }
                }
            } else if (enc_mode <= ENC_M8) {
                if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                    if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                        pcs->pic_depth_removal_level = 5;
                    } else if (pcs->coeff_lvl == HIGH_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 5 : 6;
                    } else {
                        pcs->pic_depth_removal_level = 5;
                    }
                } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                    if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                        pcs->pic_depth_removal_level = 6;
                    } else if (pcs->coeff_lvl == HIGH_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 6 : 7;
                    } else {
                        pcs->pic_depth_removal_level = 6;
                    }
                } else {
                    if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 6 : 8;
                    } else if (pcs->coeff_lvl == HIGH_LVL) {
                        pcs->pic_depth_removal_level = is_base ? 6 : 11;
                    } else {
                        pcs->pic_depth_removal_level = is_base ? 6 : 9;
                    }
                }
            } else {
                if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                    pcs->pic_depth_removal_level = 7;
                } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                    pcs->pic_depth_removal_level = is_base ? 9 : 11;
                } else {
                    pcs->pic_depth_removal_level = is_base ? 9 : 14;
                }
            }
        }
    }
    if (sc_class1) {
        if (enc_mode <= ENC_M5) {
            pcs->pic_block_based_depth_refinement_level = 0;
        } else if (enc_mode <= ENC_M6) {
            pcs->pic_block_based_depth_refinement_level = 2;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_block_based_depth_refinement_level = 6;
        } else {
            pcs->pic_block_based_depth_refinement_level = 8;
        }
    } else {
        if (enc_mode <= ENC_M0) {
            pcs->pic_block_based_depth_refinement_level = 0;
        } else if (enc_mode <= ENC_M3) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 2;
            } else {
                pcs->pic_block_based_depth_refinement_level = 3;
            }
        } else if (enc_mode <= ENC_M6) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 5;
            } else if (pcs->coeff_lvl == HIGH_LVL) {
                pcs->pic_block_based_depth_refinement_level = 7;
            } else {
                pcs->pic_block_based_depth_refinement_level = 6;
            }
        } else if (enc_mode <= ENC_M7) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 6;
            } else if (pcs->coeff_lvl == HIGH_LVL) {
#if FTR_NIC_DREFI_NEW_LVL_DEFS
                pcs->pic_block_based_depth_refinement_level = 10;
#else
                pcs->pic_block_based_depth_refinement_level = 9;
#endif
            } else {
                pcs->pic_block_based_depth_refinement_level = 8;
            }
        } else {
#if FTR_NIC_DREFI_NEW_LVL_DEFS
            pcs->pic_block_based_depth_refinement_level = 10;
#else
            pcs->pic_block_based_depth_refinement_level = 9;
#endif
        }
    }

    // r0-modulation
    if (enc_mode <= ENC_M9) {
        if (pcs->pic_block_based_depth_refinement_level && pcs->ppcs->r0_gen) {
            double r0_tab[MAX_TEMPORAL_LAYERS] = {0.20, 0.30, 0.40, 0.50, 0.50, 0.50};
            double r0_th                       = pcs->slice_type == I_SLICE ? 0.05 : r0_tab[pcs->temporal_layer_index];
            if (pcs->ppcs->r0 < r0_th) {
#if FTR_NIC_DREFI_NEW_LVL_DEFS
                pcs->pic_block_based_depth_refinement_level = MIN(pcs->pic_block_based_depth_refinement_level - 1, 8);
#else
                pcs->pic_block_based_depth_refinement_level = pcs->pic_block_based_depth_refinement_level - 1;
#endif
            }
        }
    }
    if (sc_class1) {
        if (enc_mode <= ENC_M4) {
            pcs->pic_lpd1_lvl = 0;
        } else if (enc_mode <= ENC_M8) {
            pcs->pic_lpd1_lvl = is_not_last_layer ? 0 : 2;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_lpd1_lvl = is_base ? 0 : 4;
        } else {
            pcs->pic_lpd1_lvl = is_base ? 0 : 5;
        }
    } else {
        if (enc_mode <= ENC_M6) {
            pcs->pic_lpd1_lvl = 0;
        } else if (enc_mode <= ENC_M9) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd1_lvl = is_not_last_layer ? 0 : 2;
            } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                pcs->pic_lpd1_lvl = is_base ? 0 : 2;
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 3;
            }
        } else if (enc_mode <= ENC_M10) {
            if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                    pcs->pic_lpd1_lvl = is_base ? 0 : 3;
                } else if (pcs->coeff_lvl == HIGH_LVL) {
                    pcs->pic_lpd1_lvl = is_base ? 0 : 5;
                } else { // Regular
                    pcs->pic_lpd1_lvl = is_base ? 0 : 4;
                }
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 5;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->pic_lpd1_lvl = is_base ? 0 : 7;
        } else {
            pcs->pic_lpd1_lvl = is_islice ? 0 : is_base ? 3 : 7;
        }
    }
    // Can only use light-PD1 under the following conditions
    // There is another check before PD1 is called; pred_depth_only is not checked here, because some modes
    // may force pred_depth_only at the light-pd1 detector
    if (pcs->pic_lpd1_lvl && !(ppcs->hbd_md == 0 && pcs->pic_disallow_4x4 == true && scs->super_block_size == 64)) {
        pcs->pic_lpd1_lvl = 0;
    }

    pcs->lambda_weight = 0;
    if (pcs->scs->static_config.tune == TUNE_IQ) {
        // Adjust lambda weight towards more favorable still-picture performance (from 128 to 200),
        // with gradual ramp-down for the lowest and highest QPs
        // Lower QP cutoff: QP 18 = (QP) * 4
        // Upper QP cutoff: QP 39 = (63 - QP) * 3
        pcs->lambda_weight = CLIP3(0, 72, MIN(ppcs->picture_qp * 4, (63 - ppcs->picture_qp) * 3)) + 128;
    } else { // Tune 0 to 2
        if (!(enc_mode <= ENC_MR)) {
            if (!is_islice && ppcs->picture_qp >= 62) {
                pcs->lambda_weight = 300;
            } else if (ppcs->picture_qp >= 56) {
                pcs->lambda_weight = 175;
            } else if (ppcs->picture_qp >= 16) {
                pcs->lambda_weight = 150;
            }
        }
    }
    // Extended CRF range (63.25 - 70), increase lambda weight toward further bit saving
    // Max lambda weight increase: 28 * 28 = 784
    // The multiplier of "28" was derived empirically to allow a smooth bitrate decrease as
    // CRF increases from 63.25 (extended_crf_qindex_offset = 1) to 70 (extended_crf_qindex_offset = 4 * 7)
    if (scs->static_config.qp == MAX_QP_VALUE && scs->static_config.extended_crf_qindex_offset) {
        pcs->lambda_weight += scs->static_config.extended_crf_qindex_offset * 28;
    }

    uint8_t dlf_level = 0;
    if (pcs->scs->static_config.enable_dlf_flag && frm_hdr->allow_intrabc == 0) {
        EncMode dlf_enc_mode = enc_mode;

        if (pcs->scs->static_config.enable_dlf_flag == 2) {
            // trade off more accurate deblocking for longer encode time
            // use dlf_mode as if were being set for 3 presets lower
            dlf_enc_mode = AOMMAX(ENC_MR, enc_mode - 3);
        }

        dlf_level = get_dlf_level_default(
            pcs, dlf_enc_mode, is_not_last_layer, fast_decode, input_resolution, (pcs->temporal_layer_index == 0));
    }
    svt_aom_set_dlf_controls(pcs->ppcs, dlf_level);
}

void svt_aom_sig_deriv_mode_decision_config_rtc(SequenceControlSet* scs, PictureControlSet* pcs) {
    PictureParentControlSet* ppcs                = pcs->ppcs;
    EncMode                  enc_mode            = pcs->enc_mode;
    const uint8_t            is_ref              = ppcs->is_ref;
    const uint8_t            is_base             = ppcs->temporal_layer_index == 0;
    const EbInputResolution  input_resolution    = ppcs->input_resolution;
    const uint8_t            is_islice           = pcs->slice_type == I_SLICE;
    const uint8_t            sc_class1           = ppcs->sc_class1;
    const uint8_t            fast_decode         = scs->static_config.fast_decode;
    const uint32_t           hierarchical_levels = ppcs->hierarchical_levels;
    const bool               transition_present  = (ppcs->transition_present == 1);
    const bool               is_not_last_layer   = !ppcs->is_highest_layer;
    const uint32_t           sq_qp               = scs->static_config.qp;
    const bool               flat_rtc            = scs->use_flat_ipp;
    //MFMV
    uint8_t mfmv_level = 0;
    if (is_islice || scs->mfmv_enabled == 0 || pcs->ppcs->frm_hdr.error_resilient_mode) {
        mfmv_level = 0;
    } else {
        if (fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE) {
            if (enc_mode <= ENC_MR) {
                mfmv_level = 1;
            } else if (enc_mode <= ENC_M8) {
                mfmv_level = (input_resolution <= INPUT_SIZE_360p_RANGE) ? 1 : 2;
            } else {
                mfmv_level = (input_resolution <= INPUT_SIZE_360p_RANGE) ? 1 : 4;
            }
        } else {
            mfmv_level = 4;
        }
    }
    mfmv_controls(pcs, mfmv_level);

    uint8_t update_cdf_level = svt_aom_get_update_cdf_level_rtc(enc_mode, is_islice, is_base, sc_class1);
    set_cdf_controls(pcs, update_cdf_level);
    if (pcs->cdf_ctrl.enabled) {
        const uint16_t picture_sb_w = ppcs->picture_sb_width;
        const uint16_t picture_sb_h = ppcs->picture_sb_height;
        const uint16_t all_sb       = picture_sb_w * picture_sb_h;
        rtime_alloc_ec_ctx_array(pcs, all_sb);
    }

    pcs->pic_filter_intra_level = get_filter_intra_level_rtc(enc_mode);

    if (pcs->enc_mode <= ENC_M8) {
        pcs->ppcs->use_accurate_part_ctx = true;
    } else {
        pcs->ppcs->use_accurate_part_ctx = false;
    }

    FrameHeader* frm_hdr             = &ppcs->frm_hdr;
    frm_hdr->allow_high_precision_mv = (frm_hdr->quantization_params.base_q_idx < HIGH_PRECISION_MV_QTHRESH_0 ||
                                        (pcs->ref_hp_percentage > HIGH_PRECISION_REF_PERC_TH &&
                                         frm_hdr->quantization_params.base_q_idx < HIGH_PRECISION_MV_QTHRESH_1)) &&
            scs->input_resolution <= INPUT_SIZE_480p_RANGE
        ? 1
        : 0;
    // Set Warped Motion level and enabled flag
    pcs->wm_level = 0;
    if (frm_hdr->frame_type == KEY_FRAME || frm_hdr->frame_type == INTRA_ONLY_FRAME || frm_hdr->error_resilient_mode ||
        pcs->ppcs->frame_superres_enabled || pcs->ppcs->frame_resize_enabled) {
        pcs->wm_level = 0;
    } else {
        pcs->wm_level = 0;
    }
    if (hierarchical_levels <= 2) {
        pcs->wm_level = enc_mode <= ENC_M6 ? pcs->wm_level : 0;
    }
    if (enc_mode <= ENC_M6 && scs->seq_qp_mod) {
        if (sq_qp > 55 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->wm_level = pcs->wm_level == 1 ? pcs->wm_level : pcs->wm_level == 0 ? MAX_WARP_LVL : pcs->wm_level - 1;
        }
    }

    bool enable_wm = pcs->wm_level ? 1 : 0;
    // Note: local warp should be disabled when super-res or resize is ON
    // according to the AV1 spec 5.11.27
    frm_hdr->allow_warped_motion = enable_wm &&
        !(frm_hdr->frame_type == KEY_FRAME || frm_hdr->frame_type == INTRA_ONLY_FRAME) &&
        !frm_hdr->error_resilient_mode && !pcs->ppcs->frame_superres_enabled &&
        scs->static_config.resize_mode == RESIZE_NONE;

    frm_hdr->is_motion_mode_switchable = frm_hdr->allow_warped_motion;
    ppcs->pic_obmc_level               = svt_aom_get_obmc_level(enc_mode, sq_qp, scs->seq_qp_mod);
    // Switchable Motion Mode
    frm_hdr->is_motion_mode_switchable = frm_hdr->is_motion_mode_switchable || ppcs->pic_obmc_level;

    if (enc_mode <= ENC_M8) {
        pcs->approx_inter_rate = 0;
    } else if ((!flat_rtc && enc_mode <= ENC_M11) || (flat_rtc && enc_mode <= ENC_M10)) {
        pcs->approx_inter_rate = 1;
    } else {
        pcs->approx_inter_rate = 2;
    }

    if (is_islice || transition_present) {
        pcs->skip_intra = 0;
    } else {
        if ((!flat_rtc && enc_mode <= ENC_M11) || (flat_rtc && enc_mode <= ENC_M10)) {
            pcs->skip_intra = 0;
        } else {
            pcs->skip_intra = (is_ref || pcs->ref_intra_percentage > 50) ? 0 : 1;
        }
    }
    uint32_t intra_level                = 0;
    uint32_t dist_based_ang_intra_level = 0;

    svt_aom_get_intra_mode_levels_rtc(
        enc_mode, is_islice, sc_class1, transition_present, flat_rtc, &intra_level, &dist_based_ang_intra_level);

    pcs->intra_level                = intra_level;
    pcs->dist_based_ang_intra_level = dist_based_ang_intra_level;
    // Set the level for the candidate(s) reduction feature
    pcs->cand_reduction_level = 0;
    if (is_islice) {
        pcs->cand_reduction_level = 0;
    } else {
        pcs->cand_reduction_level = 1;
    }
    if (scs->rc_stat_gen_pass_mode) {
        pcs->cand_reduction_level = 6;
    }

    // Set the level for the txt search
    pcs->txt_level = 0;
    if (enc_mode <= ENC_MR) {
        pcs->txt_level = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M2) {
        pcs->txt_level = is_base ? 2 : 5;
    } else if ((!flat_rtc && enc_mode <= ENC_M10) || (flat_rtc && enc_mode <= ENC_M11)) {
        pcs->txt_level = is_base ? 7 : 9;
    } else {
        pcs->txt_level = 0;
    }
    // Set the level for the txt shortcut feature
    // Any tx_shortcut_level having the chroma detector off in REF frames should be reserved for M13+
    pcs->tx_shortcut_level = 0;
    if (flat_rtc) {
        if (enc_mode <= ENC_M7) {
            pcs->tx_shortcut_level = 0;
        } else if (enc_mode <= ENC_M11) {
            pcs->tx_shortcut_level = is_islice ? 0 : 3;
        } else {
            pcs->tx_shortcut_level = 3;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            pcs->tx_shortcut_level = 0;
        } else if (enc_mode <= ENC_M9) {
            pcs->tx_shortcut_level = is_islice ? 0 : 3;
        } else {
            pcs->tx_shortcut_level = 3;
        }
    }

    // Set RDOQ level
    if ((!flat_rtc && enc_mode <= ENC_M8) || (flat_rtc && enc_mode <= ENC_M9)) {
        pcs->rdoq_level = 1;
    } else {
        pcs->rdoq_level = is_islice ? 1 : 0;
    }

    pcs->rate_est_level = 1;

    // Set the level the interpolation search
    pcs->interpolation_search_level = 0;
    if (enc_mode <= ENC_MR) {
        pcs->interpolation_search_level = 2;
    } else if (enc_mode <= ENC_M7) {
        pcs->interpolation_search_level = 4;
    } else {
        pcs->interpolation_search_level = 4;
        if (!is_base) {
            const uint8_t th[INPUT_SIZE_COUNT] = {100, 100, 85, 50, 30, 30, 30};
            const uint8_t skip_area            = pcs->ref_skip_percentage;
            if (skip_area > th[input_resolution]) {
                pcs->interpolation_search_level = 0;
            }
        }
    }
    frm_hdr->interpolation_filter = SWITCHABLE;

    pcs->chroma_level = svt_aom_get_chroma_level_rtc(enc_mode, is_islice);

    // Set the level for cfl
    pcs->cfl_level = 0;
    if (sc_class1) { //DNC
        if (enc_mode <= ENC_M6) {
            pcs->cfl_level = 1;
        } else {
            pcs->cfl_level = is_base ? 2 : 0;
        }
    } else if (enc_mode <= ENC_M1) {
        pcs->cfl_level = 1;
    } else if (enc_mode <= ENC_M6) {
        pcs->cfl_level = is_base ? 2 : 0;
    } else {
        pcs->cfl_level = is_islice ? 2 : 0;
    }

    // Set the level for new/nearest/near injection
    if (enc_mode <= ENC_MR) {
        pcs->new_nearest_near_comb_injection = 1;
    } else if (enc_mode <= ENC_M1) {
        pcs->new_nearest_near_comb_injection = is_base ? 2 : 0;
    } else {
        pcs->new_nearest_near_comb_injection = 0;
    }

    // Set the level for unipred3x3 injection
    if (enc_mode <= ENC_MR) {
        pcs->unipred3x3_injection = 1;
    } else {
        pcs->unipred3x3_injection = 0;
    }

    // Set the level for bipred3x3 injection

    if (enc_mode <= ENC_M0) {
        pcs->bipred3x3_injection = 1;
    } else if (enc_mode <= ENC_M1) {
        pcs->bipred3x3_injection = 2;
    } else {
        pcs->bipred3x3_injection = 0;
    }

    // Set the level for inter-inter compound
    pcs->inter_compound_mode = get_inter_compound_level(enc_mode);

    // Set the level for the distance-based red pruning
    if (pcs->ppcs->ref_list0_count_try > 1 || pcs->ppcs->ref_list1_count_try > 1) {
        if (enc_mode <= ENC_MR) {
            pcs->dist_based_ref_pruning = 0;
        } else if (enc_mode <= ENC_M2) {
            pcs->dist_based_ref_pruning = is_base ? 1 : 5;
        } else if (enc_mode <= ENC_M10) {
            pcs->dist_based_ref_pruning = is_base ? 2 : 5;
        } else {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->dist_based_ref_pruning = is_base ? 2 : 6;
            } else {
                pcs->dist_based_ref_pruning = is_base ? 3 : 6;
            }
        }
    } else {
        pcs->dist_based_ref_pruning = 0;
    }

    // Set the level the spatial sse @ full-loop
    if (enc_mode <= ENC_M2) {
        pcs->spatial_sse_full_loop_level = 1;
    } else {
        pcs->spatial_sse_full_loop_level = 3;
    }

    //set the nsq_level
    pcs->nsq_geom_level   = svt_aom_get_nsq_geom_level_rtc(enc_mode);
    pcs->nsq_search_level = svt_aom_get_nsq_search_level_rtc(pcs, enc_mode, pcs->coeff_lvl, scs->static_config.qp);

    // Set the level for inter-intra level
    if (!is_islice && scs->seq_header.enable_interintra_compound) {
        pcs->inter_intra_level = svt_aom_get_inter_intra_level(enc_mode, transition_present);
    } else {
        pcs->inter_intra_level = 0;
    }

    if (enc_mode <= ENC_M7) {
        pcs->txs_level = is_base ? 3 : 0;
    } else if (enc_mode <= ENC_M8) {
        if (sc_class1) {
            pcs->txs_level = is_base ? 4 : 0;
        } else {
            pcs->txs_level = is_islice ? 4 : 0;
        }
    } else {
        pcs->txs_level = is_islice ? 4 : 0;
    }

    // QP-banding
    if (pcs->txs_level && scs->seq_qp_mod) {
        if (sq_qp > 58 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->txs_level = pcs->txs_level == 1 ? pcs->txs_level : pcs->txs_level - 1;
        }
    }
    // Set tx_mode for the frame header
    frm_hdr->tx_mode = (pcs->txs_level) ? TX_MODE_SELECT : TX_MODE_LARGEST;
    // Set the level for nic
    pcs->nic_level = svt_aom_get_nic_level_rtc(enc_mode, scs->use_flat_ipp);

    // Set the level for SQ me-search
    pcs->md_sq_mv_search_level = 0;

    // Set the level for NSQ me-search
    pcs->md_nsq_mv_search_level = 2;
    // Set the level for PME search
    if (enc_mode <= ENC_MR) {
        pcs->md_pme_level = 1;
    } else if (enc_mode <= ENC_M0) {
        pcs->md_pme_level = 2;
    } else if (enc_mode <= ENC_M5) {
        pcs->md_pme_level = 3;
    } else if (enc_mode <= ENC_M8) {
        pcs->md_pme_level = 4;
    } else {
        pcs->md_pme_level = 0;
    }
    // Set the level for mds0
    pcs->mds0_level = 0;
    if (enc_mode <= ENC_M2) {
        pcs->mds0_level = 0;
    } else if (!sc_class1 && enc_mode <= ENC_M5) {
        pcs->mds0_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M10) {
        pcs->mds0_level = is_islice ? 0 : 2;
    } else {
        pcs->mds0_level = 2;
    }
    /*
    disallow_4x4
    */
#if TUNE_STILL_IMAGE
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4_rtc(enc_mode);
#else
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4(enc_mode);
#endif
    /*
    Bypassing EncDec
    */
    // This signal can only be modified per picture right now, not per SB.  Per SB requires
    // neighbour array updates at EncDec for all SBs, that are currently skipped if EncDec is bypassed.
    if (!ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
#if TUNE_STILL_IMAGE
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec_rtc(enc_mode, scs->static_config.encoder_bit_depth);
#else
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec(enc_mode, scs->static_config.encoder_bit_depth);
#endif
    } else {
        pcs->pic_bypass_encdec = 0;
    }

    /*
    set lpd0_level
    */
    // for the low delay enhance base layer frames, lower the enc_mode to improve the quality

    set_pic_lpd0_lvl_rtc(pcs, enc_mode);

    if (transition_present) {
        pcs->pic_depth_removal_level = 0;
    } else {
        // Set depth_removal_level_controls
        if ((!flat_rtc && enc_mode <= ENC_M8) || (flat_rtc && enc_mode <= ENC_M9)) {
            pcs->pic_depth_removal_level = 0;
        } else {
            pcs->pic_depth_removal_level = 5;
        }
    }
    if (sc_class1) {
        if (enc_mode <= ENC_M5) {
            pcs->pic_block_based_depth_refinement_level = 0;
        } else if (enc_mode <= ENC_M6) {
            pcs->pic_block_based_depth_refinement_level = 2;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_block_based_depth_refinement_level = 6;
        } else if (enc_mode <= ENC_M10) {
            pcs->pic_block_based_depth_refinement_level = 8;
        } else {
#if FTR_NIC_DREFI_NEW_LVL_DEFS
            pcs->pic_block_based_depth_refinement_level = 10;
#else
            pcs->pic_block_based_depth_refinement_level = 9;
#endif
        }
    } else {
        if (enc_mode <= ENC_M0) {
            pcs->pic_block_based_depth_refinement_level = 0;
        } else if (enc_mode <= ENC_M3) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 2;
            } else {
                pcs->pic_block_based_depth_refinement_level = 3;
            }
        } else if (enc_mode <= ENC_M6) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 5;
            } else if (pcs->coeff_lvl == HIGH_LVL) {
                pcs->pic_block_based_depth_refinement_level = 7;
            } else {
                pcs->pic_block_based_depth_refinement_level = 6;
            }
        } else if (enc_mode <= ENC_M7) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 6;
            } else if (pcs->coeff_lvl == HIGH_LVL) {
#if FTR_NIC_DREFI_NEW_LVL_DEFS
                pcs->pic_block_based_depth_refinement_level = 10;
#else
                pcs->pic_block_based_depth_refinement_level = 9;
#endif
            } else {
                pcs->pic_block_based_depth_refinement_level = 8;
            }
        } else {
#if FTR_NIC_DREFI_NEW_LVL_DEFS
            pcs->pic_block_based_depth_refinement_level = 10;
#else
            pcs->pic_block_based_depth_refinement_level = 9;
#endif
        }
    }
    // r0-modulation
    if (enc_mode <= ENC_M9) {
        if (pcs->pic_block_based_depth_refinement_level && pcs->ppcs->r0_gen) {
            double r0_tab[MAX_TEMPORAL_LAYERS] = {0.20, 0.30, 0.40, 0.50, 0.50, 0.50};
            double r0_th                       = pcs->slice_type == I_SLICE ? 0.05 : r0_tab[pcs->temporal_layer_index];
            if (pcs->ppcs->r0 < r0_th) {
                pcs->pic_block_based_depth_refinement_level = pcs->pic_block_based_depth_refinement_level - 1;
            }
        }
    }
    if (sc_class1) {
        if (enc_mode <= ENC_M4) {
            pcs->pic_lpd1_lvl = 0;
        } else if (enc_mode <= ENC_M8) {
            pcs->pic_lpd1_lvl = is_not_last_layer ? 0 : 2;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_lpd1_lvl = is_base ? 0 : 4;
        } else {
            pcs->pic_lpd1_lvl = is_base ? 0 : 5;
        }
    } else {
        if (enc_mode <= ENC_M7) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd1_lvl = is_base ? 0 : 2;
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 3;
            }
        } else if (enc_mode <= ENC_M10) {
            if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                pcs->pic_lpd1_lvl = is_base ? 0 : is_not_last_layer ? 2 : 3;
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 5;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->pic_lpd1_lvl = is_islice ? 0 : is_base ? 1 : 5;
        } else {
            pcs->pic_lpd1_lvl = is_islice ? 0 : is_base ? 4 : 7;
        }
    }
    // Can only use light-PD1 under the following conditions
    // There is another check before PD1 is called; pred_depth_only is not checked here, because some modes
    // may force pred_depth_only at the light-pd1 detector
    if (pcs->pic_lpd1_lvl && !(ppcs->hbd_md == 0 && pcs->pic_disallow_4x4 == true && scs->super_block_size == 64)) {
        pcs->pic_lpd1_lvl = 0;
    }

    pcs->lambda_weight = 0;
    if (pcs->scs->static_config.tune == TUNE_IQ) {
        // Adjust lambda weight towards more favorable still-picture performance (from 128 to 200),
        // with gradual ramp-down for the lowest and highest QPs
        // Lower QP cutoff: QP 18 = (QP) * 4
        // Upper QP cutoff: QP 39 = (63 - QP) * 3
        pcs->lambda_weight = CLIP3(0, 72, MIN(ppcs->picture_qp * 4, (63 - pcs->ppcs->picture_qp) * 3)) + 128;
    }
    // Extended CRF range (63.25 - 70), increase lambda weight toward further bit saving
    // Max lambda weight increase: 28 * 28 = 784
    // The multiplier of "28" was derived empirically to allow a smooth bitrate decrease as
    // CRF increases from 63.25 (extended_crf_qindex_offset = 1) to 70 (extended_crf_qindex_offset = 4 * 7)
    if (scs->static_config.qp == MAX_QP_VALUE && scs->static_config.extended_crf_qindex_offset) {
        pcs->lambda_weight += scs->static_config.extended_crf_qindex_offset * 28;
    }

    uint8_t dlf_level = 0;
    if (pcs->scs->static_config.enable_dlf_flag && frm_hdr->allow_intrabc == 0) {
        EncMode dlf_enc_mode = enc_mode;

        if (pcs->scs->static_config.enable_dlf_flag == 2) {
            // trade off more accurate deblocking for longer encode time
            // use dlf_mode as if were being set for 3 presets lower
            dlf_enc_mode = AOMMAX(ENC_MR, enc_mode - 3);
        }

        dlf_level = get_dlf_level_rtc(pcs, dlf_enc_mode, is_not_last_layer, (pcs->temporal_layer_index == 0));
    }
    svt_aom_set_dlf_controls(pcs->ppcs, dlf_level);
}

void svt_aom_sig_deriv_mode_decision_config_allintra(SequenceControlSet* scs, PictureControlSet* pcs) {
    PictureParentControlSet* ppcs             = pcs->ppcs;
    EncMode                  enc_mode         = pcs->enc_mode;
    const EbInputResolution  input_resolution = ppcs->input_resolution;
    const uint8_t            fast_decode      = scs->static_config.fast_decode;
    const uint32_t           sq_qp            = scs->static_config.qp;
    FrameHeader*             frm_hdr          = &ppcs->frm_hdr;

    //MFMV
    mfmv_controls(pcs, 0);

    uint8_t update_cdf_level = svt_aom_get_update_cdf_level_allintra(enc_mode);
    set_cdf_controls(pcs, update_cdf_level);
    if (pcs->cdf_ctrl.enabled) {
        const uint16_t picture_sb_w = ppcs->picture_sb_width;
        const uint16_t picture_sb_h = ppcs->picture_sb_height;
        const uint16_t all_sb       = picture_sb_w * picture_sb_h;
        rtime_alloc_ec_ctx_array(pcs, all_sb);
    }

    pcs->pic_filter_intra_level = get_filter_intra_level_allintra(enc_mode);
#if TUNE_M7_ALLINTRA
    if (pcs->enc_mode <= ENC_M6) {
#else
    if (pcs->enc_mode <= ENC_M7) {

#endif
        pcs->ppcs->use_accurate_part_ctx = true;
    } else {
        pcs->ppcs->use_accurate_part_ctx = false;
    }

    pcs->wm_level = 0;

    pcs->approx_inter_rate = 0;

    pcs->skip_intra = 0;

    // Set the intra level
    uint32_t intra_level                = 0;
    uint32_t dist_based_ang_intra_level = 0;

    svt_aom_get_intra_mode_levels_allintra(enc_mode, &intra_level, &dist_based_ang_intra_level);

    pcs->intra_level                = intra_level;
    pcs->dist_based_ang_intra_level = dist_based_ang_intra_level;

    // Set the level for the candidate(s) reduction feature
    pcs->cand_reduction_level = 0;

    // Set the level for the txt search
    if (enc_mode <= ENC_M3) {
        pcs->txt_level = 2;
    } else if (enc_mode <= ENC_M5) {
        pcs->txt_level = 3;
    } else if (enc_mode <= ENC_M6) {
        pcs->txt_level = 6;
    } else {
        pcs->txt_level = 10;
    }

    // Set the level for the txt shortcut feature
    pcs->tx_shortcut_level = 0;

    // Set RDOQ level
    if (enc_mode <= ENC_M7) {
        pcs->rdoq_level = 1;
    } else if (pcs->coeff_lvl == HIGH_LVL) {
        pcs->rdoq_level = 0;
    } else if (pcs->coeff_lvl == NORMAL_LVL) {
        pcs->rdoq_level = 3;
    } else {
        pcs->rdoq_level = 2;
    }
    // Set the rate estimation level
#if TUNE_M7_ALLINTRA
    if (enc_mode <= ENC_M6) {
#else
    if (enc_mode <= ENC_M7) {
#endif
        pcs->rate_est_level = 1;
    } else if (enc_mode <= ENC_M8) {
        pcs->rate_est_level = 4;
    } else {
        pcs->rate_est_level = 0;
    }

    // Set the interpolation search level.
    // This could be avoided for all-intra coding, but only after ensuring
    // it is not used during MD, to avoid checking an uninitialized variable.
    pcs->interpolation_search_level = 0;

    // Set the chroma level
    pcs->chroma_level = svt_aom_get_chroma_level_allintra(enc_mode);

    // Set the level for cfl
    if (enc_mode <= ENC_M0) {
        pcs->cfl_level = 1;
    } else if (enc_mode <= ENC_M6) {
        pcs->cfl_level = 4;
    } else if (enc_mode <= ENC_M7) {
        pcs->cfl_level = 5;
    } else {
        pcs->cfl_level = 0;
    }

    // Set the level for new/nearest/near injection
    pcs->new_nearest_near_comb_injection = 0;

    // Set the level for unipred3x3 injection
    pcs->unipred3x3_injection = 0;

    // Set the level for bipred3x3 injection
    pcs->bipred3x3_injection = 0;

    // Set the level for inter-inter compound
    pcs->inter_compound_mode = 0;

    // Set the level for the distance-based red pruning
    pcs->dist_based_ref_pruning = 0;

    // Set the level the spatial sse @ full-loop
    pcs->spatial_sse_full_loop_level = 3;

    //set the nsq_level
    pcs->nsq_geom_level   = svt_aom_get_nsq_geom_level_allintra(enc_mode);
    pcs->nsq_search_level = svt_aom_get_nsq_search_level_allintra(pcs, enc_mode, scs->static_config.qp);

    //set the txs level
    if (enc_mode <= ENC_M3) {
        pcs->txs_level = 2;
    } else if (enc_mode <= ENC_M7) {
        pcs->txs_level = 3;
    } else {
        pcs->txs_level = 5;
    }

    // QP-banding
    if (pcs->txs_level && scs->seq_qp_mod) {
        if (sq_qp > 58 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->txs_level = pcs->txs_level == 1 ? pcs->txs_level : pcs->txs_level - 1;
        }
    }
    // Set tx_mode for the frame header
    frm_hdr->tx_mode = (pcs->txs_level) ? TX_MODE_SELECT : TX_MODE_LARGEST;

    // Set the level for nic
    pcs->nic_level = svt_aom_get_nic_level_allintra(enc_mode);

    // Set the level for SQ me-search
    pcs->md_sq_mv_search_level = 0;

    // Set the level for NSQ me-search
    pcs->md_nsq_mv_search_level = 0;

    // Set the level for PME search
    pcs->md_pme_level = 0;

    // Set the level for mds0
    pcs->mds0_level = 0;

    /*
    disallow_4x4
    */
#if TUNE_STILL_IMAGE
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4_allintra(enc_mode);
#else
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4(enc_mode);
#endif
    /*
    Bypassing EncDec
    */
    // This signal can only be modified per picture right now, not per SB.  Per SB requires
    // neighbour array updates at EncDec for all SBs, that are currently skipped if EncDec is bypassed.
    if (!ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
#if TUNE_STILL_IMAGE
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec_allintra(enc_mode);
#else
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec(enc_mode, scs->static_config.encoder_bit_depth);
#endif
    } else {
        pcs->pic_bypass_encdec = 0;
    }

    /*
    set lpd0_level
    */
    // for the low delay enhance base layer frames, lower the enc_mode to improve the quality
    set_pic_lpd0_lvl_allintra(pcs, enc_mode);

    pcs->pic_depth_removal_level = 0;

    // Set the depth refinement level
#if FTR_NIC_DREFI_NEW_LVL_DEFS
    if (enc_mode <= ENC_M4) {
        pcs->pic_block_based_depth_refinement_level = 6;
    } else if (enc_mode <= ENC_M5) {
        pcs->pic_block_based_depth_refinement_level = 9;
    } else {
        pcs->pic_block_based_depth_refinement_level = 10;
    }
#else
    if (enc_mode <= ENC_M2) {
        pcs->pic_block_based_depth_refinement_level = 5;
    } else if (enc_mode <= ENC_M4) {
        pcs->pic_block_based_depth_refinement_level = 6;
    } else if (enc_mode <= ENC_M5) {
        pcs->pic_block_based_depth_refinement_level = 10;
    } else {
        pcs->pic_block_based_depth_refinement_level = 9;
    }
#endif
    pcs->pic_lpd1_lvl = 0;

    pcs->lambda_weight = 0;
    if (pcs->scs->static_config.tune == TUNE_IQ) {
        // Adjust lambda weight towards more favorable still-picture performance (from 128 to 200),
        // with gradual ramp-down for the lowest and highest QPs
        // Lower QP cutoff: QP 18 = (QP) * 4
        // Upper QP cutoff: QP 39 = (63 - QP) * 3
        pcs->lambda_weight = CLIP3(0, 72, MIN(ppcs->picture_qp * 4, (63 - ppcs->picture_qp) * 3)) + 128;
    } else { // Tune 0 to 2
        if (ppcs->picture_qp >= 56) {
            pcs->lambda_weight = 175;
        } else if (ppcs->picture_qp >= 16) {
            pcs->lambda_weight = 150;
        }
    }
    // Extended CRF range (63.25 - 70), increase lambda weight toward further bit saving
    // Max lambda weight increase: 28 * 28 = 784
    // The multiplier of "28" was derived empirically to allow a smooth bitrate decrease as
    // CRF increases from 63.25 (extended_crf_qindex_offset = 1) to 70 (extended_crf_qindex_offset = 4 * 7)
    if (scs->static_config.qp == MAX_QP_VALUE && scs->static_config.extended_crf_qindex_offset) {
        pcs->lambda_weight += scs->static_config.extended_crf_qindex_offset * 28;
    }
    // Set the dlf level
    uint8_t dlf_level = 0;
    if (pcs->scs->static_config.enable_dlf_flag && frm_hdr->allow_intrabc == 0) {
        EncMode dlf_enc_mode = enc_mode;

        if (pcs->scs->static_config.enable_dlf_flag == 2) {
            // trade off more accurate deblocking for longer encode time
            // use dlf_mode as if were being set for 3 presets lower
            dlf_enc_mode = AOMMAX(ENC_MR, enc_mode - 3);
        }
        dlf_level = get_dlf_level_allintra(dlf_enc_mode, fast_decode, input_resolution);
    }
    svt_aom_set_dlf_controls(pcs->ppcs, dlf_level);
}
#else
void svt_aom_sig_deriv_mode_decision_config(SequenceControlSet* scs, PictureControlSet* pcs) {
    PictureParentControlSet* ppcs                = pcs->ppcs;
    = EncMode                enc_mode            = pcs->enc_mode;
    const uint8_t            is_ref              = ppcs->is_ref;
    const uint8_t            is_base             = ppcs->temporal_layer_index == 0;
    const uint8_t            is_layer1           = ppcs->temporal_layer_index == 1;
    const EbInputResolution  input_resolution    = ppcs->input_resolution;
    const uint8_t            is_islice           = pcs->slice_type == I_SLICE;
    const uint8_t            sc_class1           = ppcs->sc_class1;
    const uint8_t            fast_decode         = scs->static_config.fast_decode;
    const uint32_t           hierarchical_levels = ppcs->hierarchical_levels;
    const bool               transition_present  = (ppcs->transition_present == 1);
    const bool               rtc_tune            = scs->static_config.rtc;
    const bool               is_not_last_layer   = !ppcs->is_highest_layer;
    const uint32_t           sq_qp               = scs->static_config.qp;
    const bool               allintra            = scs->allintra;
    const bool               flat_rtc            = rtc_tune && scs->use_flat_ipp;
    //MFMV
    uint8_t mfmv_level = 0;
    if (is_islice || scs->mfmv_enabled == 0 || pcs->ppcs->frm_hdr.error_resilient_mode) {
        mfmv_level = 0;
    } else {
        if (fast_decode == 0 || input_resolution <= INPUT_SIZE_360p_RANGE) {
            if (enc_mode <= ENC_MR) {
                mfmv_level = 1;
            } else if (enc_mode <= ENC_M8) {
                mfmv_level = (input_resolution <= INPUT_SIZE_360p_RANGE) ? 1 : 2;
            } else {
                mfmv_level = (input_resolution <= INPUT_SIZE_360p_RANGE) ? 1 : 4;
            }
        } else {
            mfmv_level = 4;
        }
    }
    mfmv_controls(pcs, mfmv_level);
    uint8_t update_cdf_level = svt_aom_get_update_cdf_level(
        enc_mode, is_islice, is_base, sc_class1, input_resolution, allintra);
    //set the conrols uisng the required level
    set_cdf_controls(pcs, update_cdf_level);

    if (pcs->cdf_ctrl.enabled) {
        const uint16_t picture_sb_w = ppcs->picture_sb_width;
        const uint16_t picture_sb_h = ppcs->picture_sb_height;
        const uint16_t all_sb       = picture_sb_w * picture_sb_h;
        rtime_alloc_ec_ctx_array(pcs, all_sb);
    }
    //Filter Intra Mode : 0: OFF  1: ON
    // pic_filter_intra_level specifies whether filter intra would be active
    // for a given picture.
    pcs->pic_filter_intra_level = get_filter_intra_level(scs, enc_mode);
    if (allintra) {
        if (enc_mode <= ENC_M9) {
            ppcs->use_accurate_part_ctx = true;
        } else {
            ppcs->use_accurate_part_ctx = false;
        }
    } else if (enc_mode <= ENC_M8) {
        ppcs->use_accurate_part_ctx = true;
    } else {
        ppcs->use_accurate_part_ctx = false;
    }
    FrameHeader* frm_hdr             = &ppcs->frm_hdr;
    frm_hdr->allow_high_precision_mv = (frm_hdr->quantization_params.base_q_idx < HIGH_PRECISION_MV_QTHRESH_0 ||
                                        (pcs->ref_hp_percentage > HIGH_PRECISION_REF_PERC_TH &&
                                         frm_hdr->quantization_params.base_q_idx < HIGH_PRECISION_MV_QTHRESH_1)) &&
            scs->input_resolution <= INPUT_SIZE_480p_RANGE
        ? 1
        : 0;
    // Set Warped Motion level and enabled flag
    pcs->wm_level = 0;
    if (frame_is_intra_only(ppcs) || frm_hdr->error_resilient_mode || pcs->ppcs->frame_superres_enabled ||
        pcs->ppcs->frame_resize_enabled) {
        pcs->wm_level = 0;
    } else {
        if (rtc_tune) {
            pcs->wm_level = 0;
        } else if (enc_mode <= ENC_M1) {
            pcs->wm_level = 1;
        } else if (enc_mode <= ENC_M3) {
            if (hierarchical_levels <= 3) {
                pcs->wm_level = is_base ? 1 : 3;
            } else {
                pcs->wm_level = (is_base || is_layer1) ? 2 : 3;
            }
        } else if (enc_mode <= ENC_M9) {
            if (input_resolution <= INPUT_SIZE_720p_RANGE) {
                pcs->wm_level = is_base ? 3 : 0;
            } else {
                pcs->wm_level = is_base ? 4 : 0;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->wm_level = is_base ? 4 : 0;
        } else {
            pcs->wm_level = 0;
        }
    }
    if (hierarchical_levels <= 2) {
        pcs->wm_level = enc_mode <= ENC_M6 ? pcs->wm_level : 0;
    }
    if ((enc_mode <= ENC_M6 || (!rtc_tune && !allintra && enc_mode <= ENC_M7)) && scs->seq_qp_mod) {
        if (sq_qp > 55 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->wm_level = pcs->wm_level == 1 ? pcs->wm_level : pcs->wm_level == 0 ? MAX_WARP_LVL : pcs->wm_level - 1;
        }
    }

    bool enable_wm = pcs->wm_level ? 1 : 0;
    // Note: local warp should be disabled when super-res or resize is ON
    // according to the AV1 spec 5.11.27
    frm_hdr->allow_warped_motion = enable_wm && !frame_is_intra_only(ppcs) && !frm_hdr->error_resilient_mode &&
        !pcs->ppcs->frame_superres_enabled && scs->static_config.resize_mode == RESIZE_NONE;

    frm_hdr->is_motion_mode_switchable = frm_hdr->allow_warped_motion;
    ppcs->pic_obmc_level               = svt_aom_get_obmc_level(enc_mode, sq_qp, scs->seq_qp_mod);
    // Switchable Motion Mode
    frm_hdr->is_motion_mode_switchable = frm_hdr->is_motion_mode_switchable || ppcs->pic_obmc_level;

    if (rtc_tune) {
        if (enc_mode <= ENC_M8) {
            pcs->approx_inter_rate = 0;
        } else if ((!scs->use_flat_ipp && enc_mode <= ENC_M11) || (scs->use_flat_ipp && enc_mode <= ENC_M10)) {
            pcs->approx_inter_rate = 1;
        } else {
            pcs->approx_inter_rate = 2;
        }
    } else {
        if (enc_mode <= ENC_M9) {
            pcs->approx_inter_rate = 0;
        } else {
            pcs->approx_inter_rate = 1;
        }
    }
    if (is_islice || transition_present) {
        pcs->skip_intra = 0;
    } else if (rtc_tune) {
        if ((!scs->use_flat_ipp && enc_mode <= ENC_M11) || (scs->use_flat_ipp && enc_mode <= ENC_M10)) {
            pcs->skip_intra = 0;
        } else {
            pcs->skip_intra = (is_ref || pcs->ref_intra_percentage > 50) ? 0 : 1;
        }
    } else {
        if (enc_mode <= ENC_M1) {
            pcs->skip_intra = 0;
        } else {
            pcs->skip_intra = (is_ref || pcs->ref_intra_percentage > 50) ? 0 : 1;
        }
    }
    uint32_t intra_level                = 0;
    uint32_t dist_based_ang_intra_level = 0;
    svt_aom_get_intra_mode_levels(enc_mode,
                                  input_resolution,
                                  allintra,
                                  rtc_tune,
                                  is_islice,
                                  is_base,
                                  sc_class1,
                                  transition_present,
                                  scs->use_flat_ipp,
                                  &intra_level,
                                  &dist_based_ang_intra_level);

    pcs->intra_level                = intra_level;
    pcs->dist_based_ang_intra_level = dist_based_ang_intra_level;
    // Set the level for the candidate(s) reduction feature
    pcs->cand_reduction_level = 0;
    if (is_islice) {
        pcs->cand_reduction_level = 0;
    } else if (rtc_tune) {
        pcs->cand_reduction_level = 1;
    } else if (enc_mode <= ENC_MR) {
        pcs->cand_reduction_level = 0;
    } else if (enc_mode <= ENC_M2) {
        pcs->cand_reduction_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M7) {
        pcs->cand_reduction_level = 1;
    } else {
        pcs->cand_reduction_level = 2;
    }

    if (scs->rc_stat_gen_pass_mode) {
        pcs->cand_reduction_level = 6;
    }

    // Set the level for the txt search
    pcs->txt_level = 0;
    if (allintra) {
        if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_M2) {
                pcs->txt_level = 2;
            } else if (enc_mode <= ENC_M6) {
                pcs->txt_level = 3;
            } else if (enc_mode <= ENC_M7) {
                pcs->txt_level = 4;
            } else if (enc_mode <= ENC_M8) {
                pcs->txt_level = 6;
            } else if (enc_mode <= ENC_M11) {
                pcs->txt_level = 10;
            } else if (input_resolution <= INPUT_SIZE_720p_RANGE) {
                pcs->txt_level = 10;
            } else {
                pcs->txt_level = 0;
            }
        } else {
            if (enc_mode <= ENC_M0) {
                pcs->txt_level = 2;
            } else if (enc_mode <= ENC_M2) {
                pcs->txt_level = 3;
            } else if (enc_mode <= ENC_M8) {
                pcs->txt_level = 4;
            } else if (enc_mode <= ENC_M9) {
                pcs->txt_level = 8;
            } else {
                pcs->txt_level = 0;
            }
        }
    } else if (enc_mode <= ENC_MR) {
        pcs->txt_level = is_base ? 2 : 3;
    } else if (enc_mode <= ENC_M2) {
        pcs->txt_level = is_base ? 2 : 5;
    } else if ((!rtc_tune && enc_mode <= ENC_M10) || (rtc_tune && !scs->use_flat_ipp && enc_mode <= ENC_M10) ||
               (rtc_tune && scs->use_flat_ipp && enc_mode <= ENC_M11)) {
        pcs->txt_level = is_base ? 7 : 9;
    } else if (!rtc_tune && enc_mode <= ENC_M11) {
        pcs->txt_level = 10;
    } else {
        pcs->txt_level = 0;
    }
    // Set the level for the txt shortcut feature
    // Any tx_shortcut_level having the chroma detector off in REF frames should be reserved for M13+
    pcs->tx_shortcut_level = 0;
    if (allintra) { // allintra
        if (enc_mode <= ENC_M2) {
            pcs->tx_shortcut_level = 0;
        } else if (enc_mode <= ENC_M11) {
            pcs->tx_shortcut_level = is_base ? 0 : 1;
        } else {
            pcs->tx_shortcut_level = is_islice ? 0 : 3;
        }
    } else if (flat_rtc) { // FLAT-RTC
        if (enc_mode <= ENC_M7) {
            pcs->tx_shortcut_level = 0;
        } else if (enc_mode <= ENC_M11) {
            pcs->tx_shortcut_level = is_islice ? 0 : 3;
        } else {
            pcs->tx_shortcut_level = 3;
        }
    } else if (rtc_tune) { // Non-FLAT RTC
        if (enc_mode <= ENC_M7) {
            pcs->tx_shortcut_level = 0;
        } else if (enc_mode <= ENC_M9) {
            pcs->tx_shortcut_level = is_islice ? 0 : 3;
        } else {
            pcs->tx_shortcut_level = 3;
        }
    } else {
        if (enc_mode <= ENC_M2) {
            pcs->tx_shortcut_level = 0;
        } else if (enc_mode <= ENC_M9) {
            pcs->tx_shortcut_level = is_base ? 0 : 1;
        } else if (enc_mode <= ENC_M10) {
            pcs->tx_shortcut_level = is_base ? 1 : 2;
        } else {
            pcs->tx_shortcut_level = is_islice ? 1 : 3;
        }
    }
    // Set RDOQ level
    if (allintra) {
        if (enc_mode <= ENC_M10) {
            pcs->rdoq_level = 1;
        } else {
            pcs->rdoq_level = 2;
        }
    } else if (rtc_tune) {
        if ((!scs->use_flat_ipp && enc_mode <= ENC_M8) || (scs->use_flat_ipp && enc_mode <= ENC_M9)) {
            pcs->rdoq_level = 1;
        } else {
            pcs->rdoq_level = is_islice ? 1 : 0;
        }
    } else {
        if (enc_mode <= ENC_M10) {
            pcs->rdoq_level = 1;
        } else {
            pcs->rdoq_level = 2;
        }
    }

    // Set the level the interpolation search
    pcs->interpolation_search_level = 0;
    if (enc_mode <= ENC_MR) {
        pcs->interpolation_search_level = 2;
    } else if (enc_mode <= ENC_M7 || (!rtc_tune && !allintra && enc_mode <= ENC_M8)) {
        pcs->interpolation_search_level = 4;
    } else {
        pcs->interpolation_search_level = 4;
        if (!is_base) {
            const uint8_t th[INPUT_SIZE_COUNT] = {100, 100, 85, 50, 30, 30, 30};
            const uint8_t skip_area            = pcs->ref_skip_percentage;
            if (skip_area > th[input_resolution]) {
                pcs->interpolation_search_level = 0;
            }
        }
    }
    frm_hdr->interpolation_filter = SWITCHABLE;
    pcs->chroma_level             = svt_aom_get_chroma_level(enc_mode, is_islice, allintra);
    // Set the level for cfl
    pcs->cfl_level = 0;
    if (sc_class1) { //DNC
        if (enc_mode <= ENC_M6) {
            pcs->cfl_level = 1;
        } else {
            pcs->cfl_level = is_base ? 2 : 0;
        }
    } else if (allintra) {
        if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_MR) {
                pcs->cfl_level = 1;
            } else if (enc_mode <= ENC_M8) {
                pcs->cfl_level = 4;
            } else if (enc_mode <= ENC_M9) {
                pcs->cfl_level = 5;
            } else {
                pcs->cfl_level = 0;
            }
        } else {
            if (enc_mode <= ENC_M0) {
                pcs->cfl_level = 1;
            } else if (enc_mode <= ENC_M1) {
                pcs->cfl_level = 2;
            } else {
                pcs->cfl_level = 0;
            }
        }
    } else if (enc_mode <= ENC_M1) {
        pcs->cfl_level = 1;
    } else if ((rtc_tune && enc_mode <= ENC_M6) || (!rtc_tune && enc_mode <= ENC_M9)) {
        pcs->cfl_level = is_base ? 2 : 0;
    } else if (!rtc_tune && enc_mode <= ENC_M10) {
        pcs->cfl_level = is_islice ? 2 : 0;
    } else if (!rtc_tune && enc_mode <= ENC_M11) {
        pcs->cfl_level = 0;
    } else {
        pcs->cfl_level = is_islice ? 2 : 0;
    }

    // Set the level for new/nearest/near injection
    if (enc_mode <= ENC_MR) {
        pcs->new_nearest_near_comb_injection = 1;
    } else if (enc_mode <= ENC_M1) {
        pcs->new_nearest_near_comb_injection = is_base ? 2 : 0;
    } else {
        pcs->new_nearest_near_comb_injection = 0;
    }

    // Set the level for unipred3x3 injection
    if (enc_mode <= ENC_MR) {
        pcs->unipred3x3_injection = 1;
    } else {
        pcs->unipred3x3_injection = 0;
    }

    // Set the level for bipred3x3 injection

    if (enc_mode <= ENC_M0) {
        pcs->bipred3x3_injection = 1;
    } else if (enc_mode <= ENC_M1) {
        pcs->bipred3x3_injection = 2;
    } else {
        pcs->bipred3x3_injection = 0;
    }

    // Set the level for inter-inter compound
    pcs->inter_compound_mode = get_inter_compound_level(enc_mode);

    // Set the level for the distance-based red pruning
    if (pcs->ppcs->ref_list0_count_try > 1 || pcs->ppcs->ref_list1_count_try > 1) {
        if (enc_mode <= ENC_MR) {
            pcs->dist_based_ref_pruning = 0;
        } else if (enc_mode <= ENC_M2) {
            pcs->dist_based_ref_pruning = is_base ? 1 : 5;
        } else if ((!rtc_tune && enc_mode <= ENC_M8) || (rtc_tune && enc_mode <= ENC_M10)) {
            pcs->dist_based_ref_pruning = is_base ? 2 : 5;
        } else if (rtc_tune) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->dist_based_ref_pruning = is_base ? 2 : 6;
            } else {
                pcs->dist_based_ref_pruning = is_base ? 3 : 6;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->dist_based_ref_pruning = is_base ? 2 : 5;
        } else {
            pcs->dist_based_ref_pruning = 8;
        }
    } else {
        pcs->dist_based_ref_pruning = 0;
    }

    // Set the level the spatial sse @ full-loop
    if (enc_mode <= ENC_M2) {
        pcs->spatial_sse_full_loop_level = allintra ? 3 : 1;
    } else {
        pcs->spatial_sse_full_loop_level = 3;
    }
    //set the nsq_level
    pcs->nsq_geom_level = svt_aom_get_nsq_geom_level(
        allintra, scs->input_resolution, enc_mode, pcs->coeff_lvl, rtc_tune);
    pcs->nsq_search_level = svt_aom_get_nsq_search_level(pcs, enc_mode, pcs->coeff_lvl, scs->static_config.qp);
    // Set the level for inter-intra level
    if (!is_islice && scs->seq_header.enable_interintra_compound) {
        pcs->inter_intra_level = svt_aom_get_inter_intra_level(enc_mode, transition_present);
    } else {
        pcs->inter_intra_level = 0;
    }

    if (allintra) {
        if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
            if (enc_mode <= ENC_M3) {
                pcs->txs_level = 2;
            } else if (enc_mode <= ENC_M7) {
                pcs->txs_level = 3;
            } else if (enc_mode <= ENC_M9) {
                pcs->txs_level = 4;
            } else if (enc_mode <= ENC_M10) {
                pcs->txs_level = 5;
            } else {
                if (input_resolution <= INPUT_SIZE_720p_RANGE) {
                    pcs->txs_level = 5;
                } else {
                    pcs->txs_level = 0;
                }
            }
        } else {
            if (enc_mode <= ENC_M3) {
                pcs->txs_level = 2;
            } else if (enc_mode <= ENC_M5) {
                pcs->txs_level = 3;
            } else if (enc_mode <= ENC_M6) {
                pcs->txs_level = is_islice ? 4 : 0;
            } else {
                pcs->txs_level = 0;
            }
        }
    } else if (rtc_tune) {
        if (enc_mode <= ENC_M7) {
            pcs->txs_level = is_base ? 3 : 0;
        } else if (enc_mode <= ENC_M8) {
            if (sc_class1) {
                pcs->txs_level = is_base ? 4 : 0;
            } else {
                pcs->txs_level = is_islice ? 4 : 0;
            }
        } else {
            pcs->txs_level = is_islice ? 4 : 0;
        }
    } else if (enc_mode <= ENC_M1) {
        pcs->txs_level = 2;
    } else if (enc_mode <= ENC_M8) {
        pcs->txs_level = is_base ? 3 : 0;
    } else if (enc_mode <= ENC_M9) {
        pcs->txs_level = is_base ? 4 : 0;
    } else if (enc_mode <= ENC_M10) {
        pcs->txs_level = is_islice ? 4 : 0;
    } else {
        pcs->txs_level = 0;
    }

    // QP-banding
    if (pcs->txs_level && scs->seq_qp_mod) {
        if (sq_qp > 58 && (scs->seq_qp_mod == 1 || scs->seq_qp_mod == 2)) {
            pcs->txs_level = pcs->txs_level == 1 ? pcs->txs_level : pcs->txs_level - 1;
        }
    }
    // Set tx_mode for the frame header
    frm_hdr->tx_mode = (pcs->txs_level) ? TX_MODE_SELECT : TX_MODE_LARGEST;
    // Set the level for nic
    pcs->nic_level = svt_aom_get_nic_level(pcs->scs, enc_mode, is_base, rtc_tune, sc_class1);
    // Set the level for SQ me-search
    pcs->md_sq_mv_search_level = 0;

    // Set the level for NSQ me-search
    pcs->md_nsq_mv_search_level = 2;
    // Set the level for PME search
    if (enc_mode <= ENC_MR) {
        pcs->md_pme_level = 1;
    } else if (enc_mode <= ENC_M0) {
        pcs->md_pme_level = 2;
    } else if (enc_mode <= ENC_M5) {
        pcs->md_pme_level = 3;
    } else if ((!rtc_tune && enc_mode <= ENC_M10) || (rtc_tune && enc_mode <= ENC_M8)) {
        pcs->md_pme_level = 4;
    } else {
        pcs->md_pme_level = 0;
    }
    // Set the level for mds0
    pcs->mds0_level = 0;
    if (enc_mode <= ENC_M2) {
        pcs->mds0_level = 0;
    } else if (!sc_class1 && enc_mode <= ENC_M5) {
        pcs->mds0_level = is_base ? 0 : 1;
    } else if (enc_mode <= ENC_M10) {
        pcs->mds0_level = is_islice ? 0 : 2;
    } else {
        pcs->mds0_level = 2;
    }
    /*
    disallow_4x4
    */
    pcs->pic_disallow_4x4 = svt_aom_get_disallow_4x4(enc_mode);
    /*
    Bypassing EncDec
    */
    // This signal can only be modified per picture right now, not per SB.  Per SB requires
    // neighbour array updates at EncDec for all SBs, that are currently skipped if EncDec is bypassed.
    if (!ppcs->frm_hdr.segmentation_params.segmentation_enabled) {
        pcs->pic_bypass_encdec = svt_aom_get_bypass_encdec(enc_mode, scs->static_config.encoder_bit_depth);
    } else {
        pcs->pic_bypass_encdec = 0;
    }

    /*
    set lpd0_level
    */
    // for the low delay enhance base layer frames, lower the enc_mode to improve the quality
    set_pic_lpd0_lvl(pcs, enc_mode);
    if (allintra) {
        if (enc_mode <= ENC_M11) {
            pcs->pic_depth_removal_level = 0;
        } else {
            if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                pcs->pic_depth_removal_level = 0;
            } else if (input_resolution <= INPUT_SIZE_720p_RANGE) {
                pcs->pic_depth_removal_level = 1;
            } else {
                pcs->pic_depth_removal_level = 2;
            }
        }
    } else if (is_islice || transition_present) {
        pcs->pic_depth_removal_level = 0;
    } else {
        // Set depth_removal_level_controls
        if (rtc_tune) {
            if ((!scs->use_flat_ipp && enc_mode <= ENC_M8) || (scs->use_flat_ipp && enc_mode <= ENC_M9)) {
                pcs->pic_depth_removal_level = 0;
            } else {
                pcs->pic_depth_removal_level = 5;
            }
        } else {
            if (sc_class1) {
                if (enc_mode <= ENC_M6) {
                    pcs->pic_depth_removal_level = 0;
                } else if (enc_mode <= ENC_M9) {
                    pcs->pic_depth_removal_level = is_base ? 0 : 6;
                } else {
                    pcs->pic_depth_removal_level = is_base ? 5 : 14;
                }
            } else {
                if (enc_mode <= ENC_M1) {
                    pcs->pic_depth_removal_level = 0;
                } else if (enc_mode <= ENC_M5) {
                    if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                        if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 1 : 2;
                        } else if (pcs->coeff_lvl == HIGH_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 3 : 5;
                        } else {
                            pcs->pic_depth_removal_level = is_base ? 3 : 4;
                        }
                    } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                        if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 1 : 2;
                        } else if (pcs->coeff_lvl == HIGH_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 3 : 6;
                        } else {
                            pcs->pic_depth_removal_level = is_base ? 3 : 5;
                        }
                    } else {
                        if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 1 : 3;
                        } else if (pcs->coeff_lvl == HIGH_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 4 : 8;
                        } else {
                            pcs->pic_depth_removal_level = is_base ? 4 : 7;
                        }
                    }
                } else if (enc_mode <= ENC_M8) {
                    if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                        if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                            pcs->pic_depth_removal_level = 5;
                        } else if (pcs->coeff_lvl == HIGH_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 5 : 6;
                        } else {
                            pcs->pic_depth_removal_level = 5;
                        }
                    } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                        if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                            pcs->pic_depth_removal_level = 6;
                        } else if (pcs->coeff_lvl == HIGH_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 6 : 7;
                        } else {
                            pcs->pic_depth_removal_level = 6;
                        }
                    } else {
                        if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 6 : 8;
                        } else if (pcs->coeff_lvl == HIGH_LVL) {
                            pcs->pic_depth_removal_level = is_base ? 6 : 11;
                        } else {
                            pcs->pic_depth_removal_level = is_base ? 6 : 9;
                        }
                    }
                } else {
                    if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                        pcs->pic_depth_removal_level = 7;
                    } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                        pcs->pic_depth_removal_level = is_base ? 9 : 11;
                    } else {
                        pcs->pic_depth_removal_level = is_base ? 9 : 14;
                    }
                }
            }
        }
    }
    if (sc_class1) {
        if (enc_mode <= ENC_M5) {
            pcs->pic_block_based_depth_refinement_level = 0;
        } else if (enc_mode <= ENC_M6) {
            pcs->pic_block_based_depth_refinement_level = 2;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_block_based_depth_refinement_level = 6;
        } else if (enc_mode <= ENC_M10) {
            pcs->pic_block_based_depth_refinement_level = 8;
        } else {
            pcs->pic_block_based_depth_refinement_level = rtc_tune ? 9 : 8;
        }
    } else {
        if (allintra) {
            if (scs->input_resolution <= INPUT_SIZE_1080p_RANGE) {
                if (enc_mode <= ENC_MR) {
                    pcs->pic_block_based_depth_refinement_level = 3;
                } else if (enc_mode <= ENC_M1) {
                    pcs->pic_block_based_depth_refinement_level = 5;
                } else if (enc_mode <= ENC_M5) {
                    pcs->pic_block_based_depth_refinement_level = 6;
                } else {
                    pcs->pic_block_based_depth_refinement_level = 9;
                }
            } else {
                if (enc_mode <= ENC_M5) {
                    pcs->pic_block_based_depth_refinement_level = 6;
                } else if (enc_mode <= ENC_M8) {
                    pcs->pic_block_based_depth_refinement_level = 7;
                } else {
                    pcs->pic_block_based_depth_refinement_level = 9;
                }
            }
        } else if (enc_mode <= ENC_M0) {
            pcs->pic_block_based_depth_refinement_level = 0;
        } else if (enc_mode <= ENC_M3) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 2;
            } else {
                pcs->pic_block_based_depth_refinement_level = 3;
            }
        } else if (enc_mode <= ENC_M6) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 5;
            } else if (pcs->coeff_lvl == HIGH_LVL) {
                pcs->pic_block_based_depth_refinement_level = 7;
            } else {
                pcs->pic_block_based_depth_refinement_level = 6;
            }
        } else if (enc_mode <= ENC_M7) {
            if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                pcs->pic_block_based_depth_refinement_level = 6;
            } else if (pcs->coeff_lvl == HIGH_LVL) {
                pcs->pic_block_based_depth_refinement_level = 9;
            } else {
                pcs->pic_block_based_depth_refinement_level = 8;
            }
        } else {
            pcs->pic_block_based_depth_refinement_level = 9;
        }
    }

    // r0-modulation
    if (enc_mode <= ENC_M9) {
        if (pcs->pic_block_based_depth_refinement_level && pcs->ppcs->r0_gen) {
            double r0_tab[MAX_TEMPORAL_LAYERS] = {0.20, 0.30, 0.40, 0.50, 0.50, 0.50};
            double r0_th                       = pcs->slice_type == I_SLICE ? 0.05 : r0_tab[pcs->temporal_layer_index];
            if (pcs->ppcs->r0 < r0_th) {
                pcs->pic_block_based_depth_refinement_level = pcs->pic_block_based_depth_refinement_level - 1;
            }
        }
    }
    if (sc_class1) {
        if (enc_mode <= ENC_M4) {
            pcs->pic_lpd1_lvl = 0;
        } else if (enc_mode <= ENC_M8) {
            pcs->pic_lpd1_lvl = is_not_last_layer ? 0 : 2;
        } else if (enc_mode <= ENC_M9) {
            pcs->pic_lpd1_lvl = is_base ? 0 : 4;
        } else {
            pcs->pic_lpd1_lvl = is_base ? 0 : 5;
        }
    } else if (rtc_tune) {
        if (enc_mode <= ENC_M7) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd1_lvl = is_base ? 0 : 2;
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 3;
            }
        } else if (enc_mode <= ENC_M10) {
            if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                pcs->pic_lpd1_lvl = is_base ? 0 : is_not_last_layer ? 2 : 3;
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 5;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->pic_lpd1_lvl = is_islice ? 0 : is_base ? 1 : 5;
        } else {
            pcs->pic_lpd1_lvl = is_islice ? 0 : is_base ? 4 : 7;
        }
    } else {
        if (enc_mode <= ENC_M6) {
            pcs->pic_lpd1_lvl = 0;
        } else if (enc_mode <= ENC_M9) {
            if (input_resolution <= INPUT_SIZE_360p_RANGE) {
                pcs->pic_lpd1_lvl = is_not_last_layer ? 0 : 2;
            } else if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                pcs->pic_lpd1_lvl = is_base ? 0 : 2;
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 3;
            }
        } else if (enc_mode <= ENC_M10) {
            if (input_resolution <= INPUT_SIZE_480p_RANGE) {
                if (pcs->coeff_lvl == VLOW_LVL || pcs->coeff_lvl == LOW_LVL) {
                    pcs->pic_lpd1_lvl = is_base ? 0 : 3;
                } else if (pcs->coeff_lvl == HIGH_LVL) {
                    pcs->pic_lpd1_lvl = is_base ? 0 : 5;
                } else { // Regular
                    pcs->pic_lpd1_lvl = is_base ? 0 : 4;
                }
            } else {
                pcs->pic_lpd1_lvl = is_base ? 0 : 5;
            }
        } else if (enc_mode <= ENC_M11) {
            pcs->pic_lpd1_lvl = is_base ? 0 : 7;
        } else {
            pcs->pic_lpd1_lvl = is_islice ? 0 : is_base ? 3 : 7;
        }
    }
    // Can only use light-PD1 under the following conditions
    // There is another check before PD1 is called; pred_depth_only is not checked here, because some modes
    // may force pred_depth_only at the light-pd1 detector
    if (pcs->pic_lpd1_lvl && !(ppcs->hbd_md == 0 && pcs->pic_disallow_4x4 == true && scs->super_block_size == 64)) {
        pcs->pic_lpd1_lvl = 0;
    }

    pcs->lambda_weight = 0;
    if (pcs->scs->static_config.tune == TUNE_IQ) {
        // Adjust lambda weight towards more favorable still-picture performance (from 128 to 200),
        // with gradual ramp-down for the lowest and highest QPs
        // Lower QP cutoff: QP 18 = (QP) * 4
        // Upper QP cutoff: QP 39 = (63 - QP) * 3
        pcs->lambda_weight = CLIP3(0, 72, MIN(ppcs->picture_qp * 4, (63 - ppcs->picture_qp) * 3)) + 128;
    } else { // Tune 0 to 2
        if (!rtc_tune && !(enc_mode <= ENC_MR)) {
            if (!is_islice && ppcs->picture_qp >= 62) {
                pcs->lambda_weight = 300;
            } else if (ppcs->picture_qp >= 56) {
                pcs->lambda_weight = 175;
            } else if (ppcs->picture_qp >= 16) {
                pcs->lambda_weight = 150;
            }
        }
    }

    // Extended CRF range (63.25 - 70), increase lambda weight toward further bit saving
    // Max lambda weight increase: 28 * 28 = 784
    // The multiplier of "28" was derived empirically to allow a smooth bitrate decrease as
    // CRF increases from 63.25 (extended_crf_qindex_offset = 1) to 70 (extended_crf_qindex_offset = 4 * 7)
    if (scs->static_config.qp == MAX_QP_VALUE && scs->static_config.extended_crf_qindex_offset) {
        pcs->lambda_weight += scs->static_config.extended_crf_qindex_offset * 28;
    }

    uint8_t dlf_level = 0;
    if (pcs->scs->static_config.enable_dlf_flag && frm_hdr->allow_intrabc == 0) {
        EncMode dlf_enc_mode = enc_mode;

        if (pcs->scs->static_config.enable_dlf_flag == 2) {
            // trade off more accurate deblocking for longer encode time
            // use dlf_mode as if were being set for 3 presets lower
            dlf_enc_mode = AOMMAX(ENC_MR, enc_mode - 3);
        }
        dlf_level = get_dlf_level(pcs,
                                  dlf_enc_mode,
                                  is_not_last_layer,
                                  fast_decode,
                                  input_resolution,
                                  allintra,
                                  (pcs->temporal_layer_index == 0));
    }
    svt_aom_set_dlf_controls(pcs->ppcs, dlf_level);
}

#endif

/****************************************************
* svt_aom_set_mfmv_config: enable/disable mfmv based on the enc_mode, input_res and pred_structure at sequence level
****************************************************/
void svt_aom_set_mfmv_config(SequenceControlSet* scs) {
    if (scs->static_config.enable_mfmv == DEFAULT) {
        const bool rtc_tune = scs->static_config.rtc;
        if (rtc_tune) {
            scs->mfmv_enabled = 0;
        } else {
            if (scs->static_config.enc_mode <= ENC_M10) {
                scs->mfmv_enabled = 1;
            } else {
                scs->mfmv_enabled = 0;
            }
        }
    } else {
        scs->mfmv_enabled = scs->static_config.enable_mfmv;
    }
}
