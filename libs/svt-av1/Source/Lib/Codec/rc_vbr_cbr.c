/*
* Copyright(c) 2026 Meta Platforms, Inc. and affiliates.
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include "pcs.h"
#include "sequence_control_set.h"
#include "entropy_coding.h"

#include "rc_process.h"
#include "pass2_strategy.h"

#include "rc_tables.h"

static uint8_t NOINLINE clamp_qindex(SequenceControlSet* scs, int qindex) {
    int qmin = quantizer_to_qindex[scs->static_config.min_qp_allowed];
    int qmax = quantizer_to_qindex[scs->static_config.max_qp_allowed];
    return (uint8_t)CLIP3(qmin, qmax, qindex);
}

static EbReferenceObject* get_ref_obj(PictureControlSet* pcs, RefList ref_list, int idx) {
    return pcs->ref_pic_ptr_array[ref_list][idx]->object_ptr;
}

static int get_active_quality(int q, int boost, int low, int high, const int* low_motion_minq,
                              const int* high_motion_minq) {
    if (boost > high) {
        return low_motion_minq[q];
    }
    if (boost < low) {
        return high_motion_minq[q];
    }

    int gap        = high - low;
    int offset     = high - boost;
    int qdiff      = high_motion_minq[q] - low_motion_minq[q];
    int adjustment = (offset * qdiff + (gap >> 1)) / gap;
    return low_motion_minq[q] + adjustment;
}

static int get_kf_active_quality_tpl(RATE_CONTROL* rc, int q, EbBitDepth bit_depth) {
    const int* kf_low_motion_minq_cqp;
    const int* kf_high_motion_minq;
    ASSIGN_MINQ_TABLE(bit_depth, kf_low_motion_minq_cqp);
    ASSIGN_MINQ_TABLE(bit_depth, kf_high_motion_minq);
    return get_active_quality(
        q, rc->kf_boost, BOOST_KF_LOW, BOOST_KF_HIGH, kf_low_motion_minq_cqp, kf_high_motion_minq);
}

static int get_gf_active_quality_tpl_la(RATE_CONTROL* rc, int q, EbBitDepth bit_depth) {
    const int* arfgf_low_motion_minq;
    const int* arfgf_high_motion_minq;
    ASSIGN_MINQ_TABLE(bit_depth, arfgf_low_motion_minq);
    ASSIGN_MINQ_TABLE(bit_depth, arfgf_high_motion_minq);
    return get_active_quality(
        q, rc->gfu_boost, BOOST_GF_LOW_TPL_LA, BOOST_GF_HIGH_TPL_LA, arfgf_low_motion_minq, arfgf_high_motion_minq);
}

static int get_gf_high_motion_quality(int q, EbBitDepth bit_depth) {
    const int* arfgf_high_motion_minq;
    ASSIGN_MINQ_TABLE(bit_depth, arfgf_high_motion_minq);
    return arfgf_high_motion_minq[q];
}

static int av1_calc_pframe_target_size_one_pass_cbr(PictureParentControlSet* pcs) {
    SequenceControlSet* scs              = pcs->scs;
    EncodeContext*      enc_ctx          = scs->enc_ctx;
    RATE_CONTROL*       rc               = &enc_ctx->rc;
    RateControlCfg*     rc_cfg           = &enc_ctx->rc_cfg;
    int64_t             diff             = rc->optimal_buffer_level - rc->buffer_level;
    int64_t             one_pct_bits     = 1 + rc->optimal_buffer_level / 100;
    int                 min_frame_target = AOMMAX(rc->avg_frame_bandwidth >> 4, FRAME_OVERHEAD_BITS);
    int                 target           = rc->avg_frame_bandwidth;

    if (diff > 0) {
        // Lower the target bandwidth for this frame.
        int pct_low = (int)AOMMIN(diff / one_pct_bits, rc_cfg->under_shoot_pct);
        target -= (target * pct_low) / 200;
    } else if (diff < 0) {
        // Increase the target bandwidth for this frame.
        int pct_high = (int)AOMMIN(-diff / one_pct_bits, rc_cfg->over_shoot_pct);
        target += (target * pct_high) / 200;
    }
    if (rc_cfg->max_inter_bitrate_pct) {
        int max_rate = rc->avg_frame_bandwidth * rc_cfg->max_inter_bitrate_pct / 100;
        target       = AOMMIN(target, max_rate);
    }
    return AOMMAX(min_frame_target, target);
}

static void svt_aom_reset_update_frame_target(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    rc->buffer_level            = rc->optimal_buffer_level;
    rc->bits_off_target         = rc->optimal_buffer_level;
    ppcs->this_frame_target     = av1_calc_pframe_target_size_one_pass_cbr(ppcs);
}

// Adjust active_worst_quality level based on buffer level.
static int calc_active_worst_quality_no_stats_cbr(PictureParentControlSet* ppcs) {
    // Adjust active_worst_quality: If buffer is above the optimal/target level,
    // bring active_worst_quality down depending on fullness of buffer.
    // If buffer is below the optimal level, let the active_worst_quality go from
    // ambient Q (at buffer = optimal level) to worst_quality level
    // (at buffer = critical level).
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    // Buffer level below which we push active_worst to worst_quality.
    int64_t critical_level = rc->optimal_buffer_level >> 3;
    int64_t buff_lvl_step  = 0;
    int     adjustment     = 0;
    int     active_worst_quality;
    if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
        return rc->worst_quality;
    }
    // For ambient_qp we use minimum of avg_frame_qindex[KEY_FRAME/INTER_FRAME]
    // for the first few frames following key frame. These are both initialized
    // to worst_quality and updated with (3/4, 1/4) average in postencode_update.
    // So for first few frames following key, the qp of that key frame is weighted
    // into the active_worst_quality setting.
    svt_block_on_mutex(enc_ctx->frame_updated_mutex);
    int32_t frame_updated = enc_ctx->frame_updated;
    svt_release_mutex(enc_ctx->frame_updated_mutex);
    int ambient_qp = (frame_updated < 4) ? AOMMIN(rc->avg_frame_qindex[INTER_FRAME], rc->avg_frame_qindex[KEY_FRAME])
                                         : rc->avg_frame_qindex[INTER_FRAME];
    ambient_qp     = AOMMIN(rc->worst_quality, ambient_qp);
    if (rc->buffer_level > rc->optimal_buffer_level) {
        active_worst_quality = AOMMIN(rc->worst_quality, ambient_qp * 5 / 4);
        // Adjust down.
        // Maximum limit for down adjustment, ~30%.
        int max_adjustment_down = active_worst_quality / 3;
        if (max_adjustment_down) {
            buff_lvl_step = ((rc->maximum_buffer_size - rc->optimal_buffer_level) / max_adjustment_down);
            if (buff_lvl_step) {
                adjustment = (int)((rc->buffer_level - rc->optimal_buffer_level) / buff_lvl_step);
            }
            active_worst_quality -= adjustment;
        }
    } else if (rc->buffer_level > critical_level) {
        active_worst_quality = AOMMIN(rc->worst_quality, ambient_qp);
        // Adjust up from ambient Q.
        if (critical_level) {
            buff_lvl_step = (rc->optimal_buffer_level - critical_level);
            if (buff_lvl_step) {
                adjustment = (int)((rc->worst_quality - ambient_qp) * (rc->optimal_buffer_level - rc->buffer_level) /
                                   buff_lvl_step);
            }
            active_worst_quality += adjustment;
        }
    } else {
        // Set to worst_quality if buffer is below critical level.
        active_worst_quality = rc->worst_quality;
    }
    return active_worst_quality;
}

static int adjust_q_cbr_flat(PictureParentControlSet* ppcs, int q) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;

    // Flag to indicate previous frame has overshoot, and buffer level
    // for current frame is low (less than ~half of optimal). For such
    // (inter) frames, if the source_sad is non-zero, relax the max_delta_up
    // and clamp applied below.
    bool overshoot_buffer_low = rc->rc_1_frame == -1 && rc->cur_avg_base_me_dist > 1000 &&
        rc->buffer_level < (rc->optimal_buffer_level >> 1) && rc->frames_since_key > 4;
    int max_delta_up               = overshoot_buffer_low ? 120 : 90;
    int change_avg_frame_bandwidth = abs(rc->avg_frame_bandwidth - rc->prev_avg_frame_bandwidth) >
        0.1 * (rc->avg_frame_bandwidth);

    // Set the maximum adjustment down for Q for this frame.
    // Add logic to handle cyclic refresh !!
    int max_delta_down = ppcs->sc_class1 ? clamp(rc->q_1_frame / 16, 1, 8) : clamp(rc->q_1_frame / 8, 1, 16);

    // Apply some control/clamp to QP under certain conditions.
    // Delay the use of the clamping for svc until after num_temporal_layers,
    // to make they have been set for each temporal layer.
    // Check for rc->q_1/2_frame > 0 in case they have not been set due to
    // dropped frames.
    if (ppcs->frm_hdr.frame_type != KEY_FRAME && rc->frames_since_key > 1 && rc->q_1_frame > 0 && rc->q_2_frame > 0 &&
        !change_avg_frame_bandwidth) {
        // If in the previous two frames we have seen both overshoot and undershoot
        // clamp Q between the two.
        if (rc->rc_1_frame * rc->rc_2_frame == -1 && rc->q_1_frame != rc->q_2_frame && !overshoot_buffer_low) {
            int qclamp = clamp(q, AOMMIN(rc->q_1_frame, rc->q_2_frame), AOMMAX(rc->q_1_frame, rc->q_2_frame));
            // If the previous frame had overshoot and the current q needs to
            // increase above the clamped value, reduce the clamp for faster reaction
            // to overshoot.
            if (rc->rc_1_frame == -1 && q > qclamp && rc->frames_since_key > 10) {
                q = (q + qclamp) >> 1;
            } else {
                q = qclamp;
            }
        }
        // Adjust Q base on source content change from scene detection.
        if (rc->prev_avg_base_me_dist > 0 && rc->frames_since_key > 5 && rc->cur_avg_base_me_dist > 0) {
            int    bit_depth = scs->static_config.encoder_bit_depth;
            double delta     = (double)rc->cur_avg_base_me_dist / (double)rc->prev_avg_base_me_dist - 1.0;
            // Push Q downwards if content change is decreasing and buffer level
            // is stable (at least 1/4-optimal level), so not overshooting. Do so
            // only for high Q to avoid excess overshoot.
            // Else reduce decrease in Q from previous frame if content change is
            // increasing and buffer is below max (so not undershooting).
            if (delta < 0.0 && rc->buffer_level > (rc->optimal_buffer_level >> 2) && q > (rc->worst_quality >> 1)) {
                double q_adj_factor = 1.0 + 0.5 * tanh(4.0 * delta);
                double q_val        = svt_av1_convert_qindex_to_q(q, bit_depth);
                q += svt_av1_compute_qdelta(q_val, q_val * q_adj_factor, bit_depth);
            } else if (rc->q_1_frame - q > 0 && delta > 0.1 &&
                       rc->buffer_level < AOMMIN(rc->maximum_buffer_size, rc->optimal_buffer_level << 1)) {
                q = (3 * q + rc->q_1_frame) >> 2;
            }
        }
        // Limit the decrease in Q from previous frame.
        if (rc->q_1_frame - q > max_delta_down) {
            q = rc->q_1_frame - max_delta_down;
        }
        // Limit the increase in Q from previous frame.
        else if (q - rc->q_1_frame > max_delta_up) {
            q = rc->q_1_frame + max_delta_up;
        }
    }

    return AOMMAX(AOMMIN(q, rc->worst_quality), rc->best_quality);
}

static const int max_delta_per_layer[MAX_HIERARCHICAL_LEVEL][MAX_TEMPORAL_LAYERS] = {
    {60}, {60, 5}, {60, 20, 2}, {60, 20, 10, 2}, {60, 20, 10, 5, 2}, {60, 30, 20, 10, 5, 2}};

static int adjust_q_cbr(PictureParentControlSet* ppcs, int q) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;

    int max_delta                  = max_delta_per_layer[ppcs->hierarchical_levels][ppcs->temporal_layer_index];
    int max_delta_down             = (ppcs->sc_class1) ? AOMMIN(max_delta, AOMMAX(1, rc->q_1_frame / 2))
                                                       : AOMMIN(max_delta, AOMMAX(1, rc->q_1_frame / 3));
    int change_avg_frame_bandwidth = abs(rc->avg_frame_bandwidth - rc->prev_avg_frame_bandwidth) >
        0.1 * (rc->avg_frame_bandwidth);
    // If resolution changes or avg_frame_bandwidth significantly changed,
    // then set this flag to indicate change in target bits per macroblock.
    int change_target_bits_mb = change_avg_frame_bandwidth;
    // Apply some control/clamp to QP under certain conditions.
    if (ppcs->frm_hdr.frame_type != KEY_FRAME && /*!cpi->use_svc &&*/
        rc->frames_since_key > 1 && !change_target_bits_mb) {
        // Adjust Q base on source content change.
        if (ppcs->temporal_layer_index == 0 && rc->prev_avg_base_me_dist > 0 && rc->frames_since_key > 5 &&
            rc->cur_avg_base_me_dist > 0) {
            int    bit_depth = scs->static_config.encoder_bit_depth;
            double delta     = (double)rc->cur_avg_base_me_dist / (double)rc->prev_avg_base_me_dist - 1.0;
            // Push Q downwards if content change is decreasing and buffer level
            // is stable (at least 1/4-optimal level), so not overshooting. Do so
            // only for high Q to avoid excess overshoot.
            if (delta < 0.0 && rc->buffer_level > (rc->optimal_buffer_level >> 2) && q > (rc->worst_quality >> 1)) {
                double q_adj_factor = 1.0 + 0.5 * tanh(4.0 * delta);
                double q_val        = svt_av1_convert_qindex_to_q(q, bit_depth);
                q += svt_av1_compute_qdelta(q_val, q_val * q_adj_factor, bit_depth);
            }
        }
        // Make sure q is between oscillating Qs to prevent resonance.
        // Limit the decrease in Q from previous frame.
        if (rc->q_1_frame - q > max_delta_down) {
            q = rc->q_1_frame - max_delta_down;
        }
    }
    return AOMMAX(AOMMIN(q, rc->worst_quality), rc->best_quality);
}

/******************************************************************************
* svt_av1_cyclic_refresh_rc_bits_per_mb
* Compute bits per mb for cyclic refresh mode
*******************************************************************************/
static int svt_av1_cyclic_refresh_rc_bits_per_mb(PictureParentControlSet* ppcs, double correction_factor, int q) {
    double weight_segment = (double)ppcs->cyclic_refresh.percent_refresh / 100;

    SequenceControlSet* scs = ppcs->scs;
    CyclicRefresh*      cr  = &ppcs->cyclic_refresh;
    // Compute delta-q corresponding to qindex i.
    int deltaq = svt_av1_compute_deltaq(ppcs, q, cr->rate_ratio_qdelta);
    // Take segment weighted average for bits per mb.
    int bits_per_mb = (int)((1.0 - weight_segment) *
                                svt_av1_rc_bits_per_mb(ppcs->frm_hdr.frame_type,
                                                       q,
                                                       correction_factor,
                                                       scs->static_config.encoder_bit_depth,
                                                       ppcs->sc_class1) +
                            weight_segment *
                                svt_av1_rc_bits_per_mb(ppcs->frm_hdr.frame_type,
                                                       q + deltaq,
                                                       correction_factor,
                                                       scs->static_config.encoder_bit_depth,
                                                       ppcs->sc_class1));
    return bits_per_mb;
}

// Calculate rate for the given 'q'.
static int get_bits_per_mb(PictureParentControlSet* ppcs, int use_cyclic_refresh, double correction_factor, int q) {
    SequenceControlSet* scs = ppcs->scs;
    return use_cyclic_refresh
        ? svt_av1_cyclic_refresh_rc_bits_per_mb(ppcs, correction_factor, q)
        : svt_av1_rc_bits_per_mb(
              ppcs->frm_hdr.frame_type, q, correction_factor, scs->static_config.encoder_bit_depth, ppcs->sc_class1);
}

// Similar to find_qindex_by_rate() function in ratectrl.c, but returns the q
// index with rate just above or below the desired rate, depending on which of
// the two rates is closer to the desired rate.
// Also, respects the selected aq_mode when computing the rate.
static int find_closest_qindex_by_rate(int desired_bits_per_mb, PictureParentControlSet* ppcs, double correction_factor,
                                       int best_qindex, int worst_qindex) {
    const int use_cyclic_refresh = 0;
    // Find 'qindex' based on 'desired_bits_per_mb'.
    assert(best_qindex <= worst_qindex);
    int low  = best_qindex;
    int high = worst_qindex;
    while (low < high) {
        int mid             = (low + high) >> 1;
        int mid_bits_per_mb = get_bits_per_mb(ppcs, use_cyclic_refresh, correction_factor, mid);
        if (mid_bits_per_mb > desired_bits_per_mb) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    assert(low == high);

    // Calculate rate difference of this q index from the desired rate.
    int curr_q           = low;
    int curr_bits_per_mb = get_bits_per_mb(ppcs, use_cyclic_refresh, correction_factor, curr_q);
    int curr_bit_diff    = (curr_bits_per_mb <= desired_bits_per_mb) ? desired_bits_per_mb - curr_bits_per_mb : INT_MAX;
    assert((curr_bit_diff != INT_MAX && curr_bit_diff >= 0) || curr_q == worst_qindex);

    // Calculate rate difference for previous q index too.
    int prev_q = curr_q - 1;
    int prev_bit_diff;
    if (curr_bit_diff == INT_MAX || curr_q == best_qindex) {
        prev_bit_diff = INT_MAX;
    } else {
        int prev_bits_per_mb = get_bits_per_mb(ppcs, use_cyclic_refresh, correction_factor, prev_q);
        assert(prev_bits_per_mb > desired_bits_per_mb);
        prev_bit_diff = prev_bits_per_mb - desired_bits_per_mb;
    }

    // Pick one of the two q indices, depending on which one has rate closer to
    // the desired rate.
    return (curr_bit_diff <= prev_bit_diff) ? curr_q : prev_q;
}

static double get_rate_correction_factor(PictureParentControlSet* ppcs, int width, int height) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    svt_block_on_mutex(rc->rc_mutex);
    double rcf;
    if (enc_ctx->rc_cfg.mode == AOM_VBR) {
        rate_factor_level rf_lvl = ppcs->frm_hdr.frame_type == KEY_FRAME ? 0 : ppcs->temporal_layer_index + 1;
        rcf                      = rc->rate_correction_factors[rf_lvl];
    } else {
        if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
            rcf = rc->rate_correction_factors[KF_STD];
        } else if ((ppcs->update_type == SVT_AV1_GF_UPDATE || ppcs->update_type == SVT_AV1_ARF_UPDATE) &&
                   !ppcs->is_overlay && enc_ctx->rc_cfg.mode != AOM_CBR) {
            rcf = rc->rate_correction_factors[GF_ARF_STD];
        } else {
            rcf = rc->rate_correction_factors[INTER_NORMAL];
        }
    }
    rcf *= (double)(ppcs->av1_cm->frm_size.frame_width * ppcs->av1_cm->frm_size.frame_height) / (width * height);
    svt_release_mutex(rc->rc_mutex);
    return fclamp(rcf, MIN_BPB_FACTOR, MAX_BPB_FACTOR);
}

static void set_rate_correction_factor(PictureParentControlSet* ppcs, double factor, int width, int height) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    svt_block_on_mutex(rc->rc_mutex);

    // Normalize RCF to account for the size-dependent scaling factor.
    factor /= (double)(ppcs->av1_cm->frm_size.frame_width * ppcs->av1_cm->frm_size.frame_height) / (width * height);

    factor = fclamp(factor, MIN_BPB_FACTOR, MAX_BPB_FACTOR);

    if (enc_ctx->rc_cfg.mode == AOM_VBR) {
        rate_factor_level rf_lvl = ppcs->frm_hdr.frame_type == KEY_FRAME ? 0 : ppcs->temporal_layer_index + 1;
        rc->rate_correction_factors[rf_lvl] = factor;
    } else {
        if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
            rc->rate_correction_factors[KF_STD] = factor;
        } else if ((ppcs->update_type == SVT_AV1_GF_UPDATE || ppcs->update_type == SVT_AV1_ARF_UPDATE) &&
                   !ppcs->is_overlay && enc_ctx->rc_cfg.mode != AOM_CBR) {
            rc->rate_correction_factors[GF_ARF_STD] = factor;
        } else {
            rc->rate_correction_factors[INTER_NORMAL] = factor;
        }
    }
    svt_release_mutex(rc->rc_mutex);
}

static int av1_rc_regulate_q(PictureParentControlSet* ppcs, int active_best_quality, int active_worst_quality,
                             int width, int height) {
    int    MBs                = ((width + 15) / 16) * ((height + 15) / 16);
    double correction_factor  = get_rate_correction_factor(ppcs, width, height);
    int    target_bits_per_mb = (int)(((uint64_t)ppcs->this_frame_target << BPER_MB_NORMBITS) / MBs);

    int q = find_closest_qindex_by_rate(
        target_bits_per_mb, ppcs, correction_factor, active_best_quality, active_worst_quality);
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    if (enc_ctx->rc_cfg.mode == AOM_CBR) {
        if (ppcs->scs->use_flat_ipp) {
            return adjust_q_cbr_flat(ppcs, q);
        } else {
            return adjust_q_cbr(ppcs, q);
        }
    }

    return q;
}

void svt_av1_resize_reset_rc(PictureParentControlSet* ppcs, int32_t resize_width, int32_t resize_height,
                             int32_t prev_width, int32_t prev_height) {
    SequenceControlSet* scs              = ppcs->scs;
    EncodeContext*      enc_ctx          = scs->enc_ctx;
    RATE_CONTROL*       rc               = &enc_ctx->rc;
    double              tot_scale_change = (double)(resize_width * resize_height) / (double)(prev_width * prev_height);
    // Reset buffer level to optimal, update target size.
    svt_aom_reset_update_frame_target(ppcs);
    if (tot_scale_change > 4.0) {
        rc->avg_frame_qindex[INTER_FRAME] = rc->worst_quality;
    } else if (tot_scale_change > 1.0) {
        rc->avg_frame_qindex[INTER_FRAME] = (rc->avg_frame_qindex[INTER_FRAME] + rc->worst_quality) >> 1;
    }
    int32_t active_worst_quality = calc_active_worst_quality_no_stats_cbr(ppcs);
    int32_t qindex = av1_rc_regulate_q(ppcs, rc->best_quality, active_worst_quality, resize_width, resize_height);
    // If resize is down, check if projected q index is close to worst_quality,
    // and if so, reduce the rate correction factor (since likely can afford
    // lower q for resized frame).
    if (tot_scale_change < 1.0 && qindex > 90 * rc->worst_quality / 100) {
        rc->rate_correction_factors[INTER_NORMAL] *= 0.85;
    }
    // If resize is back up: check if projected q index is too much above the
    // previous index, and if so, reduce the rate correction factor
    // (since prefer to keep q for resized frame at least closet to previous q).
    // Also check if projected qindex is close to previous qindex, if so
    // increase correction factor (to push qindex higher and avoid overshoot).
    if (tot_scale_change >= 1.0) {
        if (tot_scale_change < 4.0 && qindex > 130 * rc->last_q[INTER_FRAME] / 100) {
            rc->rate_correction_factors[INTER_NORMAL] *= 0.8;
        }
        if (qindex <= 120 * rc->last_q[INTER_FRAME] / 100) {
            rc->rate_correction_factors[INTER_NORMAL] *= 2.0;
        }
    }
}

#define DEFAULT_KF_BOOST_RT 2300
#define DEFAULT_GF_BOOST_RT 2000

static int set_gf_interval_update_onepass_rt(PictureParentControlSet* pcs) {
    SequenceControlSet* scs       = pcs->scs;
    EncodeContext*      enc_ctx   = scs->enc_ctx;
    RATE_CONTROL*       rc        = &enc_ctx->rc;
    int                 gf_update = 0;
    // GF update based on frames_till_gf_update_due, also
    // force update on resize pending frame or for scene change.
    if (pcs->frame_offset % MAX_GF_INTERVAL == 0) {
        rc->baseline_gf_interval = MAX_GF_INTERVAL;
        if (rc->baseline_gf_interval > rc->frames_to_key) {
            rc->baseline_gf_interval = rc->frames_to_key;
        }
        rc->gfu_boost            = DEFAULT_GF_BOOST_RT;
        rc->constrained_gf_group = (rc->baseline_gf_interval >= rc->frames_to_key) ? 1 : 0;
        gf_update                = 1;
    }
    return gf_update;
}

static void dynamic_resize_one_pass_cbr(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs           = ppcs->scs;
    EncodeContext*      enc_ctx       = scs->enc_ctx;
    RATE_CONTROL*       rc            = &enc_ctx->rc;
    RESIZE_ACTION       resize_action = NO_RESIZE;
    const int32_t       avg_qp_thr1   = 70;
    const int32_t       avg_qp_thr2   = 50;
    // Don't allow for resized frame to go below 160x90, resize in steps of 3/4.
    const int32_t min_width    = (160 * 4) / 3;
    const int32_t min_height   = (90 * 4) / 3;
    bool          down_size_on = true;

    // Step 1: check frame type
    // Don't resize on key frame; reset the counters on key frame.
    if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
        rc->resize_avg_qp           = 0;
        rc->resize_count            = 0;
        rc->resize_buffer_underflow = 0;
        return;
    }

    // Step 2: check frame size
    // No resizing down if frame size is below some limit.
    if (ppcs->frame_width * ppcs->frame_height < min_width * min_height) {
        down_size_on = false;
    }

    // Step 3: calculate dynamic resize state
    // Resize based on average buffer underflow and QP over some window.
    // Ignore samples close to key frame, since QP is usually high after key.
    if (rc->frames_since_key > scs->new_framerate) {
        int32_t window = AOMMIN(30, (int32_t)(2 * scs->new_framerate));
        rc->resize_avg_qp += rc->last_q[INTER_FRAME];
        if (rc->buffer_level < 30 * rc->optimal_buffer_level / 100) {
            ++rc->resize_buffer_underflow;
        }
        ++rc->resize_count;
        // Check for resize action every "window" frames.
        if (rc->resize_count >= window) {
            int32_t avg_qp = rc->resize_avg_qp / rc->resize_count;
            // Resize down if buffer level has underflowed sufficient amount in past
            // window, and we are at original or 3/4 of original resolution.
            // Resize back up if average QP is low, and we are currently in a resized
            // down state, i.e. 1/2 or 3/4 of original resolution.
            // Currently, use a flag to turn 3/4 resizing feature on/off.
            if (rc->resize_buffer_underflow > (rc->resize_count >> 2) && down_size_on) {
                if (rc->resize_state == THREE_QUARTER) {
                    resize_action = DOWN_ONEHALF;
                    printf("Dynamic resize: %d --> %d\n", rc->resize_state, ONE_HALF);
                    rc->resize_state = ONE_HALF;
                } else if (rc->resize_state == ORIG) {
                    resize_action = DOWN_THREEFOUR;
                    printf("Dynamic resize: %d --> %d\n", rc->resize_state, THREE_QUARTER);
                    rc->resize_state = THREE_QUARTER;
                }
            } else if (rc->resize_state != ORIG && avg_qp < avg_qp_thr1 * rc->worst_quality / 100) {
                if (rc->resize_state == THREE_QUARTER || avg_qp < avg_qp_thr2 * rc->worst_quality / 100) {
                    resize_action = UP_ORIG;
                    printf("Dynamic resize: %d --> %d\n", rc->resize_state, ORIG);
                    rc->resize_state = ORIG;
                } else if (rc->resize_state == ONE_HALF) {
                    resize_action = UP_THREEFOUR;
                    printf("Dynamic resize: %d --> %d\n", rc->resize_state, THREE_QUARTER);
                    rc->resize_state = THREE_QUARTER;
                }
            }
            // Reset for next window measurement.
            rc->resize_avg_qp           = 0;
            rc->resize_count            = 0;
            rc->resize_buffer_underflow = 0;
        }
    }

    // Step 4: reset rate control configuration
    // If decision is to resize, reset some quantities, and check is we should
    // reduce rate correction factor,
    if (resize_action != NO_RESIZE) {
        int32_t resize_width     = ppcs->frame_width; // cpi->oxcf.frm_dim_cfg.width;
        int32_t resize_height    = ppcs->frame_height; // cpi->oxcf.frm_dim_cfg.height;
        int32_t resize_scale_num = 1;
        int32_t resize_scale_den = 1;
        if (resize_action == DOWN_THREEFOUR || resize_action == UP_THREEFOUR) {
            resize_scale_num = 3;
            resize_scale_den = 4;
        } else if (resize_action == DOWN_ONEHALF) {
            resize_scale_num = 1;
            resize_scale_den = 2;
        }
        resize_width  = resize_width * resize_scale_num / resize_scale_den;
        resize_height = resize_height * resize_scale_num / resize_scale_den;
        svt_av1_resize_reset_rc(ppcs, resize_width, resize_height, ppcs->frame_width, ppcs->frame_height);
    }
}

static int av1_rc_clamp_iframe_target_size(PictureParentControlSet* pcs, int target) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    RateControlCfg*     rc_cfg  = &enc_ctx->rc_cfg;
    if (rc_cfg->max_intra_bitrate_pct) {
        int max_rate = rc->avg_frame_bandwidth * rc_cfg->max_intra_bitrate_pct / 100;
        target       = AOMMIN(target, max_rate);
    }
    if (target > rc->max_frame_bandwidth) {
        target = rc->max_frame_bandwidth;
    }
    return target;
}

// buffer level weights to calculate the target rate for Key frame
static int av1_calc_iframe_target_size_one_pass_cbr(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    int                 target;
    if (pcs->picture_number == 0) {
        target = ((rc->starting_buffer_level / 2) > INT_MAX) ? INT_MAX : (int)(rc->starting_buffer_level / 2);
    } else {
        int    kf_boost  = 32;
        double framerate = scs->new_framerate;
        kf_boost         = AOMMAX(kf_boost, (int)(2 * framerate - 16));
        if (rc->frames_since_key < framerate / 2) {
            kf_boost = (int)(kf_boost * rc->frames_since_key / (framerate / 2));
        }
        target = ((16 + kf_boost) * rc->avg_frame_bandwidth) >> 4;
    }
    return av1_rc_clamp_iframe_target_size(pcs, target);
}

static void svt_aom_one_pass_rt_rate_alloc(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    int                 target  = 0;
    // Set frame type.
    if (frame_is_intra_only(pcs)) {
        rc->kf_boost              = DEFAULT_KF_BOOST_RT;
        rc->this_key_frame_forced = pcs->picture_number != 0 && rc->frames_to_key == 0;
        rc->frames_to_key         = scs->static_config.intra_period_length + 1;
    }

    /* resize dynamic mode make desicion of scaling here and store it in resize_pending_params,
     * the actual resizing performs on the next new input picture in PD, current picture and
     * other pictures already in pipeline use their own resolution without resizing
     */
    // resize dynamic mode only works with 1-pass CBR low delay mode
    if (scs->static_config.resize_mode == RESIZE_DYNAMIC && scs->static_config.pass == ENC_SINGLE_PASS &&
        scs->static_config.pred_structure == LOW_DELAY) {
        dynamic_resize_one_pass_cbr(pcs);
        if (rc->resize_state != scs->resize_pending_params.resize_state) {
            if (rc->resize_state == ORIG) {
                scs->resize_pending_params.resize_denom = SCALE_NUMERATOR;
            } else if (rc->resize_state == THREE_QUARTER) {
                scs->resize_pending_params.resize_denom = SCALE_THREE_QUATER;
            } else if (rc->resize_state == ONE_HALF) {
                scs->resize_pending_params.resize_denom = SCALE_DENOMINATOR_MAX;
            } else {
                svt_aom_assert_err(0, "unknown resize denom");
            }
            scs->resize_pending_params.resize_state = rc->resize_state;
        }
    } else if (pcs->rc_reset_flag) {
        svt_av1_resize_reset_rc(
            pcs, pcs->render_width, pcs->render_height, scs->max_input_luma_width, scs->max_input_luma_height);
    }

    // Set the GF interval and update flag.
    set_gf_interval_update_onepass_rt(pcs);

    // Set target size.
    if (pcs->frm_hdr.frame_type == KEY_FRAME) {
        target = av1_calc_iframe_target_size_one_pass_cbr(pcs);
    } else {
        target = av1_calc_pframe_target_size_one_pass_cbr(pcs);
    }
    pcs->this_frame_target = target;
    pcs->base_frame_target = target;
}

static void set_rc_buffer_sizes(SequenceControlSet* scs) {
    EncodeContext*  enc_ctx   = scs->enc_ctx;
    RATE_CONTROL*   rc        = &enc_ctx->rc;
    RateControlCfg* rc_cfg    = &enc_ctx->rc_cfg;
    int64_t         bandwidth = scs->static_config.target_bit_rate;
    int64_t         starting  = rc_cfg->starting_buffer_level_ms;
    int64_t         optimal   = rc_cfg->optimal_buffer_level_ms;
    int64_t         maximum   = rc_cfg->maximum_buffer_size_ms;

    rc->starting_buffer_level = starting * bandwidth / 1000;
    rc->optimal_buffer_level  = (optimal == 0) ? bandwidth / 8 : optimal * bandwidth / 1000;
    rc->maximum_buffer_size   = (maximum == 0) ? bandwidth / 8 : maximum * bandwidth / 1000;
}

#define MIN_BOOST_COMBINE_FACTOR 4.0

/******************************************************************************
* process_tpl_stats_frame_kf_gfu_boost
* update r0, calculate kf and gfu boosts for VBR
*******************************************************************************/
static void process_tpl_stats_frame_kf_gfu_boost(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs                = pcs->ppcs;
    SequenceControlSet*      scs                 = ppcs->scs;
    uint8_t                  hierarchical_levels = ppcs->hierarchical_levels;
    EncodeContext*           enc_ctx             = scs->enc_ctx;
    RATE_CONTROL*            rc                  = &enc_ctx->rc;
    // The new tpl only looks at pictures in tpl group, which is fewer than before,
    // As a results, we defined a factor to adjust r0
    if (!frame_is_intra_only(ppcs)) {
        if (ppcs->tpl_ctrls.r0_adjust_factor) {
            ppcs->r0 /= ppcs->tpl_ctrls.r0_adjust_factor;
            // Further scale r0 based on the GOP structure
            ppcs->r0 /= svt_av1_tpl_hl_base_frame_div_factor[hierarchical_levels];
        }
        rc->gfu_boost = svt_av1_get_gfu_boost_from_r0_lap(
            MIN_BOOST_COMBINE_FACTOR, MAX_GFUBOOST_FACTOR, ppcs->r0, rc->frames_to_key);
    }

    if (ppcs->frm_hdr.frame_type == KEY_FRAME) {
        if (ppcs->tpl_ctrls.r0_adjust_factor) {
            ppcs->r0 /= ppcs->tpl_ctrls.r0_adjust_factor;
        }
        // Scale r0 based on the GOP structure
        ppcs->r0 /= svt_av1_tpl_hl_islice_div_factor[hierarchical_levels];

        // when frames_to_key not available, i.e. in 1 pass encoding
        rc->kf_boost  = svt_av1_get_cqp_kf_boost_from_r0(ppcs->r0, rc->frames_to_key, scs->input_resolution);
        int max_boost = 10000; // ppcs->used_tpl_frame_num * KB;
        rc->kf_boost  = AOMMIN(rc->kf_boost, max_boost);

        rc->gfu_boost = svt_av1_get_gfu_boost_from_r0_lap(
            MIN_BOOST_COMBINE_FACTOR, MAX_GFUBOOST_FACTOR, ppcs->r0, rc->frames_to_key);
    }
}

#define VBR_PCT_ADJUSTMENT_LIMIT 50

// For VBR...adjustment to the frame target based on error from previous frames
static void vbr_rate_correction(PictureControlSet* pcs, int* this_frame_target) {
    SequenceControlSet* scs                 = pcs->ppcs->scs;
    EncodeContext*      enc_ctx             = scs->enc_ctx;
    RATE_CONTROL*       rc                  = &enc_ctx->rc;
    TWO_PASS*           twopass             = &scs->twopass;
    int64_t             vbr_bits_off_target = rc->vbr_bits_off_target;
    int stats_count = twopass->stats_buf_ctx->total_stats != NULL ? (int)twopass->stats_buf_ctx->total_stats->count : 0;
    int frame_window = AOMMIN(16, stats_count - (int)pcs->picture_number);
    assert(VBR_PCT_ADJUSTMENT_LIMIT <= 100);
    if (frame_window > 0) {
        int max_delta = (int)AOMMIN(abs((int)(vbr_bits_off_target / frame_window)),
                                    (int64_t)(*this_frame_target) * VBR_PCT_ADJUSTMENT_LIMIT / 100);

        // vbr_bits_off_target > 0 means we have extra bits to spend
        // vbr_bits_off_target < 0 we are currently overshooting
        *this_frame_target += (vbr_bits_off_target >= 0) ? max_delta : -max_delta;
    }

    // Fast redistribution of bits arising from massive local undershoot.
    // Dont do it for kf,arf,gf or overlay frames.
    if (!svt_aom_frame_is_kf_gf_arf(pcs->ppcs) && !pcs->ppcs->is_overlay && rc->vbr_bits_off_target_fast) {
        int one_frame_bits  = AOMMAX(rc->avg_frame_bandwidth, *this_frame_target);
        int fast_extra_bits = (int)AOMMIN(rc->vbr_bits_off_target_fast, one_frame_bits);
        fast_extra_bits = (int)AOMMIN(fast_extra_bits, AOMMAX(one_frame_bits / 8, rc->vbr_bits_off_target_fast / 8));
        *this_frame_target += (int)fast_extra_bits;
        rc->vbr_bits_off_target_fast -= fast_extra_bits;
    }
}

static void av1_set_target_rate(PictureControlSet* pcs) {
    int target_rate = pcs->ppcs->base_frame_target;

    // Correction to rate target based on prior over or under shoot.
    vbr_rate_correction(pcs, &target_rate);
    pcs->ppcs->this_frame_target = target_rate;
}

/************************************************************************************************
* Populate the required parameters in two_pass structure from other structures
*************************************************************************************************/
static void restore_two_pass_param(PictureParentControlSet*         ppcs,
                                   RateControlIntervalParamContext* rate_control_param_ptr) {
    SequenceControlSet* scs     = ppcs->scs;
    TWO_PASS*           twopass = &scs->twopass;
    if (ppcs->scs->enable_dec_order == 1 && ppcs->scs->lap_rc && ppcs->temporal_layer_index == 0) {
        for (uint64_t num_frames = ppcs->stats_in_offset; num_frames < ppcs->stats_in_end_offset; ++num_frames) {
            FIRSTPASS_STATS* cur_frame = ppcs->scs->twopass.stats_buf_ctx->stats_in_start + num_frames;
            if ((int64_t)cur_frame->frame > ppcs->scs->twopass.stats_buf_ctx->last_frame_accumulated) {
                svt_av1_accumulate_stats(ppcs->scs->twopass.stats_buf_ctx->total_stats, cur_frame);
                ppcs->scs->twopass.stats_buf_ctx->last_frame_accumulated = (int64_t)cur_frame->frame;
            }
        }
    }

    twopass->stats_in                    = scs->twopass.stats_buf_ctx->stats_in_start + ppcs->stats_in_offset;
    twopass->stats_buf_ctx->stats_in_end = scs->twopass.stats_buf_ctx->stats_in_start + ppcs->stats_in_end_offset;
    twopass->kf_group_bits               = rate_control_param_ptr->kf_group_bits;
    twopass->kf_group_error_left         = rate_control_param_ptr->kf_group_error_left;
    if (scs->static_config.gop_constraint_rc) {
        twopass->extend_minq         = rate_control_param_ptr->extend_minq;
        twopass->extend_maxq         = rate_control_param_ptr->extend_maxq;
        twopass->extend_minq_fast    = rate_control_param_ptr->extend_minq_fast;
        RATE_CONTROL* rc             = &scs->enc_ctx->rc;
        rc->vbr_bits_off_target      = rate_control_param_ptr->vbr_bits_off_target;
        rc->vbr_bits_off_target_fast = rate_control_param_ptr->vbr_bits_off_target_fast;
        rc->rolling_target_bits      = rate_control_param_ptr->rolling_target_bits;
        rc->rolling_actual_bits      = rate_control_param_ptr->rolling_actual_bits;
        rc->rate_error_estimate      = rate_control_param_ptr->rate_error_estimate;
        rc->total_actual_bits        = rate_control_param_ptr->total_actual_bits;
        rc->total_target_bits        = rate_control_param_ptr->total_target_bits;
    }
}

/************************************************************************************************
* Populate the required parameters in rc, twopass and gf_group structures from other structures
*************************************************************************************************/
static void restore_param(PictureParentControlSet* ppcs, RateControlIntervalParamContext* rate_control_param_ptr) {
    SequenceControlSet* scs    = ppcs->scs;
    EncodeContext*      ec_ctx = scs->enc_ctx;
    if (scs->static_config.gop_constraint_rc && rate_control_param_ptr->first_poc == ppcs->picture_number) {
        rate_control_param_ptr->rolling_target_bits = ec_ctx->rc.avg_frame_bandwidth;
        rate_control_param_ptr->rolling_actual_bits = ec_ctx->rc.avg_frame_bandwidth;
    }

    restore_two_pass_param(ppcs, rate_control_param_ptr);

    int     key_max         = scs->static_config.intra_period_length + 1;
    int64_t last_frame_diff = (int)(scs->twopass.stats_buf_ctx->stats_in_end[-1].frame - ppcs->last_idr_picture + 1);
    if (scs->lap_rc) {
        if (scs->static_config.hierarchical_levels != ppcs->hierarchical_levels || ppcs->end_of_sequence_region) {
            key_max = (int)MIN(key_max, last_frame_diff);
        }
    } else {
        key_max = (int)MIN(key_max, last_frame_diff);
    }

    TWO_PASS*     twopass = &scs->twopass;
    RATE_CONTROL* rc      = &ec_ctx->rc;
    // For the last minigop of the sequence, when look ahead is not long enough to find the GOP size, the GOP size is set
    // to kf_cfg->key_freq_max and the kf_group_bits is calculated based on that. However, when we get closer to the end, the
    // end of sequence will be in the look ahead and frames_to_key is updated. In this case, kf_group_bits is calculated based
    // on the new GOP size
    rc->frames_since_key = (int)(ppcs->decode_order - ppcs->last_idr_picture);
    rc->frames_to_key    = key_max - rc->frames_since_key;
    if (scs->lap_rc && ((scs->static_config.intra_period_length + 1) != rc->frames_since_key) &&
        (scs->lad_mg + 1) * (1 << scs->static_config.hierarchical_levels) < scs->static_config.intra_period_length &&
        (scs->static_config.hierarchical_levels != ppcs->hierarchical_levels || ppcs->end_of_sequence_region) &&
        !rate_control_param_ptr->end_of_seq_seen) {
        twopass->kf_group_bits = rc->frames_to_key * twopass->kf_group_bits /
            (scs->static_config.intra_period_length + 1 - rc->frames_since_key);
        rate_control_param_ptr->end_of_seq_seen = 1;
    }
}

/************************************************************************************************
* Store the required parameters from rc, twopass and gf_group structures to other structures
*************************************************************************************************/
static void store_param(PictureParentControlSet* ppcs, RateControlIntervalParamContext* rate_control_param_ptr) {
    rate_control_param_ptr->kf_group_bits       = ppcs->scs->twopass.kf_group_bits;
    rate_control_param_ptr->kf_group_error_left = ppcs->scs->twopass.kf_group_error_left;
}

/******************************************************
 * svt_av1_rc_process_rate_allocation
 * Processes rate allocation for VBR/CBR modes:
 * - Initializes RC buffers on first frame
 * - Processes TPL statistics for key/golden frames
 * - Performs rate allocation
 * - Sets target rate
 ******************************************************/
void svt_av1_rc_process_rate_allocation(PictureControlSet* pcs, SequenceControlSet* scs) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    if (pcs->picture_number == 0 || ppcs->seq_param_changed) {
        set_rc_buffer_sizes(scs);
        svt_av1_rc_init(scs);
    }

    int32_t update_type = ppcs->update_type;
    if (ppcs->tpl_ctrls.enable && ppcs->r0 != 0 &&
        (update_type == SVT_AV1_KF_UPDATE || update_type == SVT_AV1_GF_UPDATE || update_type == SVT_AV1_ARF_UPDATE)) {
        process_tpl_stats_frame_kf_gfu_boost(pcs);
    }

    if (scs->enc_ctx->rc_cfg.mode == AOM_CBR) {
        svt_aom_one_pass_rt_rate_alloc(ppcs);
    } else {
        svt_block_on_mutex(scs->enc_ctx->stat_file_mutex);

        restore_param(ppcs, ppcs->rate_control_param_ptr);
        svt_aom_process_rc_stat(ppcs);
        av1_set_target_rate(pcs);
        store_param(ppcs, ppcs->rate_control_param_ptr);

        svt_release_mutex(scs->enc_ctx->stat_file_mutex);
    }
}

// Calculate the active_best_quality level.
static int calc_active_best_quality_no_stats_cbr(PictureControlSet* pcs, int active_worst_quality, int width,
                                                 int height) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    SequenceControlSet*      scs     = ppcs->scs;
    EncodeContext*           enc_ctx = scs->enc_ctx;
    RATE_CONTROL*            rc      = &enc_ctx->rc;
    const int*               rtc_minq;
    int                      bit_depth           = scs->static_config.encoder_bit_depth;
    int                      active_best_quality = rc->best_quality;
    ASSIGN_MINQ_TABLE(bit_depth, rtc_minq);

    if (frame_is_intra_only(ppcs)) {
        if (ppcs->frame_offset > 0) {
            // not first frame of one pass and kf_boost is set
            double q_adj_factor = 1.0;
            double q_val;
            active_best_quality = get_kf_active_quality_tpl(rc, rc->avg_frame_qindex[KEY_FRAME], bit_depth);
            // Allow somewhat lower kf minq with small image formats.
            if (width * height <= 352 * 288) {
                q_adj_factor -= 0.25;
            }
            // Convert the adjustment factor to a qindex delta
            // on active_best_quality.
            q_val = svt_av1_convert_qindex_to_q(active_best_quality, bit_depth);
            active_best_quality += svt_av1_compute_qdelta(q_val, q_val * q_adj_factor, bit_depth);
        }
    } else {
        if (scs->use_flat_ipp) {
            // Use the lower of active_worst_quality and recent/average Q.
            rc->arf_q = MAX(0, (int)pcs->ref_base_q_idx[REF_LIST_0][0] - 30);

            if (rc->arf_q < active_worst_quality) {
                active_best_quality = rtc_minq[rc->arf_q];
            } else {
                active_best_quality = rtc_minq[active_worst_quality];
            }
        } else {
            // Inherit qp from reference qps. Derive the temporal layer of the reference pictures
            EbReferenceObject* ref_obj_l0     = get_ref_obj(pcs, REF_LIST_0, 0);
            uint8_t            ref_base_q_idx = pcs->ref_base_q_idx[REF_LIST_0][0];
            uint8_t            max_tmp_layer  = ref_obj_l0->tmp_layer_idx;
            int                dist           = abs((int)pcs->picture_number - (int)ref_obj_l0->ref_poc);
            bool               best_is_islice = ref_obj_l0->slice_type == I_SLICE;

            // Check remaining list0 refs
            for (int i = 1; i < ppcs->ref_list0_count_try; i++) {
                ref_obj_l0 = get_ref_obj(pcs, REF_LIST_0, i);
                if (ref_obj_l0->slice_type != I_SLICE) {
                    // If ref is from lower temporal layer(or the same but a temporally closer ref), or the
                    // first ref was an I_SLICE, update the QP info
                    if (ref_obj_l0->tmp_layer_idx < max_tmp_layer ||
                        (ref_obj_l0->tmp_layer_idx == max_tmp_layer &&
                         abs((int)pcs->picture_number - (int)ref_obj_l0->ref_poc) < dist) ||
                        best_is_islice) {
                        ref_base_q_idx = pcs->ref_base_q_idx[REF_LIST_0][i];
                        max_tmp_layer  = ref_obj_l0->tmp_layer_idx;
                        dist           = abs((int)pcs->picture_number - (int)ref_obj_l0->ref_poc);
                        best_is_islice = false;
                    }
                }
            }

            // Check list1 refs
            for (int i = 0; i < ppcs->ref_list1_count_try; i++) {
                EbReferenceObject* ref_obj_l1 = get_ref_obj(pcs, REF_LIST_1, i);
                if (ref_obj_l1->slice_type != I_SLICE) {
                    // If ref is from lower temporal layer(or the same but a temporally closer ref), or the
                    // first ref was an I_SLICE, update the QP info
                    if (ref_obj_l1->tmp_layer_idx < max_tmp_layer ||
                        (ref_obj_l1->tmp_layer_idx == max_tmp_layer &&
                         abs((int)pcs->picture_number - (int)ref_obj_l1->ref_poc) < dist) ||
                        best_is_islice) {
                        ref_base_q_idx = pcs->ref_base_q_idx[REF_LIST_1][i];
                        max_tmp_layer  = ref_obj_l1->tmp_layer_idx;
                        dist           = abs((int)pcs->picture_number - (int)ref_obj_l1->ref_poc);
                        best_is_islice = false;
                    }
                }
            }
            uint8_t ref_tmp_layer = max_tmp_layer;
            rc->arf_q             = MAX(0, (int)ref_base_q_idx - 30);
            active_best_quality   = rtc_minq[rc->arf_q];
            int q                 = active_worst_quality;
            // Adjust wors and boost QP based on the average sad of the current picture
            int8_t tmp_layer_delta = (int8_t)ppcs->temporal_layer_index - (int8_t)ref_tmp_layer;
            // active_best_quality is updated with the q index of the reference
            while (tmp_layer_delta > 0) {
                active_best_quality = (active_best_quality + q + 1) / 2;
                tmp_layer_delta--;
            }
        }
    }
    return active_best_quality;
}

#define QFACTOR 1.1

static int rc_pick_q_and_bounds_no_stats_cbr(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    SequenceControlSet*      scs     = ppcs->scs;
    EncodeContext*           enc_ctx = scs->enc_ctx;
    RATE_CONTROL*            rc      = &enc_ctx->rc;
    assert(enc_ctx->rc_cfg.mode == AOM_CBR);

    int q;
    int bit_depth            = scs->static_config.encoder_bit_depth;
    int width                = ppcs->av1_cm->frm_size.frame_width;
    int height               = ppcs->av1_cm->frm_size.frame_height;
    int active_worst_quality = calc_active_worst_quality_no_stats_cbr(ppcs);
    int active_best_quality  = calc_active_best_quality_no_stats_cbr(pcs, active_worst_quality, width, height);

    // Clip the active best and worst quality values to limits
    active_best_quality  = clamp(active_best_quality, rc->best_quality, rc->worst_quality);
    active_worst_quality = clamp(active_worst_quality, active_best_quality, rc->worst_quality);

    ppcs->top_index    = active_worst_quality;
    ppcs->bottom_index = active_best_quality;

    // Limit Q range for the adaptive loop.
    if (ppcs->frm_hdr.frame_type == KEY_FRAME && !rc->this_key_frame_forced && ppcs->frame_offset != 0) {
        int qdelta = svt_av1_compute_qdelta_by_rate(
            rc, ppcs->frm_hdr.frame_type, active_worst_quality, 2.0, bit_depth, ppcs->sc_class1);
        ppcs->top_index = active_worst_quality + qdelta;
        ppcs->top_index = AOMMAX(ppcs->top_index, ppcs->bottom_index);
    }

    // Special case code to try and match quality with forced key frames
    if (ppcs->frm_hdr.frame_type == KEY_FRAME && rc->this_key_frame_forced) {
        q = rc->last_boosted_qindex;
    } else {
        q = av1_rc_regulate_q(ppcs, active_best_quality, active_worst_quality, width, height);
        if (q > ppcs->top_index) {
            // Special case when we are targeting the max allowed rate
            if (ppcs->this_frame_target >= rc->max_frame_bandwidth) {
                ppcs->top_index = q;
            } else {
                q = ppcs->top_index;
            }
        }
    }
    assert(ppcs->top_index <= rc->worst_quality && ppcs->top_index >= rc->best_quality);
    assert(ppcs->bottom_index <= rc->worst_quality && ppcs->bottom_index >= rc->best_quality);
    assert(q <= rc->worst_quality && q >= rc->best_quality);
    if (ppcs->update_type == SVT_AV1_ARF_UPDATE) {
        rc->arf_q = q;
    }
    int ip = scs->static_config.intra_period_length;
    // if short intra refresh
    if (ip > -1 && ip < 256) {
        if (pcs->slice_type == I_SLICE) {
            int q1 = pcs->picture_number == 0 ? q + 20 : rc->q_1_frame;
            q      = (q + q1) / 2;
        } else if (ppcs->temporal_layer_index == 0) {
            int qdelta = svt_av1_compute_qdelta_by_rate(
                rc, ppcs->frm_hdr.frame_type, active_worst_quality, QFACTOR, bit_depth, ppcs->sc_class1);
            q = q + qdelta;
        }
    }
    return q;
}

static int av1_frame_type_qdelta_org(PictureParentControlSet* ppcs, RATE_CONTROL* rc, int q, int bit_depth) {
    rate_factor_level rf_lvl      = svt_av1_rate_factor_levels[ppcs->update_type];
    FrameType         frame_type  = (rf_lvl == KF_STD) ? KEY_FRAME : INTER_FRAME;
    double            rate_factor = svt_av1_rate_factor_deltas[rf_lvl];

    if (rf_lvl == GF_ARF_LOW) {
        rate_factor -= (ppcs->layer_depth - 2) * 0.1;
        rate_factor = AOMMAX(rate_factor, 1.0);
    }
    return svt_av1_compute_qdelta_by_rate(rc, frame_type, q, rate_factor, bit_depth, ppcs->sc_class1);
}

// Returns |active_best_quality| for an inter frame.
// The returning active_best_quality could further be adjusted in
// adjust_active_best_and_worst_quality().
static int get_active_best_quality(PictureControlSet* pcs, int active_worst_quality) {
    PictureParentControlSet* ppcs               = pcs->ppcs;
    SequenceControlSet*      scs                = ppcs->scs;
    EncodeContext*           enc_ctx            = scs->enc_ctx;
    RATE_CONTROL*            rc                 = &enc_ctx->rc;
    int                      bit_depth          = scs->static_config.encoder_bit_depth;
    int                      is_intrl_arf_boost = ppcs->update_type == SVT_AV1_INTNL_ARF_UPDATE;
    const int*               inter_minq;
    ASSIGN_MINQ_TABLE(bit_depth, inter_minq);
    int active_best_quality = 0;
    int is_leaf_frame       = !(ppcs->update_type == SVT_AV1_GF_UPDATE || ppcs->update_type == SVT_AV1_ARF_UPDATE ||
                          is_intrl_arf_boost);
    int is_overlay_frame    = ppcs->is_overlay;

    if (is_leaf_frame || is_overlay_frame) {
        return inter_minq[active_worst_quality];
    }
    // Determine active_best_quality for frames that are not leaf or overlay.
    int q = active_worst_quality;
    // Use the lower of active_worst_quality and recent
    // average Q as basis for GF/ARF best Q limit unless last frame was
    // a key frame.
    if (rc->frames_since_key > 1 && rc->avg_frame_qindex[INTER_FRAME] < active_worst_quality) {
        q = rc->avg_frame_qindex[INTER_FRAME];
    }
    active_best_quality = get_gf_active_quality_tpl_la(rc, q, bit_depth);
    int min_boost       = get_gf_high_motion_quality(q, bit_depth);
    int boost           = min_boost - active_best_quality;

    double arf_boost_factor = (pcs->ref_slice_type[REF_LIST_0][0] == I_SLICE &&
                               pcs->ref_pic_r0[REF_LIST_0][0] - ppcs->r0 >= 0.08)
        ? 1.3
        : 1.0;
    active_best_quality     = min_boost - (int)(boost * arf_boost_factor);
    if (!is_intrl_arf_boost) {
        return active_best_quality;
    }

    int this_height = ppcs->layer_depth;
    while (this_height > 1) {
        active_best_quality = (active_best_quality + active_worst_quality + 1) / 2;
        --this_height;
    }
    return active_best_quality;
}

static void adjust_active_best_and_worst_quality_org(PictureControlSet* pcs, RATE_CONTROL* rc, int* active_worst,
                                                     int* active_best) {
    int                      active_best_quality  = *active_best;
    int                      active_worst_quality = *active_worst;
    PictureParentControlSet* ppcs                 = pcs->ppcs;
    SequenceControlSet*      scs                  = ppcs->scs;
    int                      bit_depth            = scs->static_config.encoder_bit_depth;
    TWO_PASS*                twopass              = &scs->twopass;
    // Extension to max or min Q if undershoot or overshoot is outside
    // the permitted range.
    if (ppcs->transition_present != 1) {
        if (frame_is_intra_only(ppcs) || (scs->static_config.gop_constraint_rc && ppcs->is_ref) ||
            (ppcs->temporal_layer_index < 2 && scs->is_short_clip) || (ppcs->is_ref && !scs->is_short_clip)) {
            active_best_quality -= twopass->extend_minq + twopass->extend_minq_fast;
            active_worst_quality += twopass->extend_maxq / 2;
        } else {
            active_best_quality -= (twopass->extend_minq + twopass->extend_minq_fast) / 2;
            active_worst_quality += twopass->extend_maxq;
        }
    }
    // Static forced key frames Q restrictions dealt with elsewhere.
    int qdelta = av1_frame_type_qdelta_org(ppcs, rc, active_worst_quality, bit_depth);

    active_worst_quality = AOMMAX(active_worst_quality + qdelta, active_best_quality);
    active_best_quality  = clamp(active_best_quality, rc->best_quality, rc->worst_quality);
    active_worst_quality = clamp(active_worst_quality, active_best_quality, rc->worst_quality);

    *active_best  = active_best_quality;
    *active_worst = active_worst_quality;
}

static int get_q(PictureControlSet* pcs, int active_worst_quality, int active_best_quality) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    SequenceControlSet*      scs     = ppcs->scs;
    EncodeContext*           enc_ctx = scs->enc_ctx;
    RATE_CONTROL*            rc      = &enc_ctx->rc;
    TWO_PASS*                twopass = &scs->twopass;
    int                      width   = ppcs->av1_cm->frm_size.frame_width;
    int                      height  = ppcs->av1_cm->frm_size.frame_height;
    int                      q;
    if (frame_is_intra_only(ppcs) && twopass->kf_zeromotion_pct >= STATIC_KF_GROUP_THRESH && rc->frames_to_key > 1) {
        q = active_best_quality;
    } else {
        q = av1_rc_regulate_q(ppcs, active_best_quality, active_worst_quality, width, height);
        if (q > active_worst_quality) {
            // Special case when we are targeting the max allowed rate.
            if (ppcs->this_frame_target < rc->max_frame_bandwidth) {
                q = active_worst_quality;
            }
        }
        q = AOMMAX(q, active_best_quality);
    }
    return q;
}

/******************************************************
 * rc_pick_q_and_bounds
 * assigns the q_index per frame using first pass statistics per frame.
 * used in the second pass of two pass encoding
 ******************************************************/
static int rc_pick_q_and_bounds(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    SequenceControlSet*      scs     = ppcs->scs;
    EncodeContext*           enc_ctx = scs->enc_ctx;
    RATE_CONTROL*            rc      = &enc_ctx->rc;
    assert(enc_ctx->rc_cfg.mode == AOM_VBR);

    int     q;
    int     active_best_quality  = 0;
    int     active_worst_quality = rc->active_worst_quality;
    int     is_intrl_arf_boost   = ppcs->update_type == SVT_AV1_INTNL_ARF_UPDATE;
    uint8_t hierarchical_levels  = ppcs->hierarchical_levels;

    // Calculated qindex based on r0 using qstep calculation
    if (ppcs->temporal_layer_index == 0) {
        unsigned int r0_weight_idx = !frame_is_intra_only(ppcs);
        assert(r0_weight_idx <= 2);
        double weight      = svt_av1_r0_weight[r0_weight_idx];
        double qstep_ratio = sqrt(ppcs->r0) * weight *
            svt_av1_qp_scale_compress_weight[scs->static_config.qp_scale_compress_strength];
        if (scs->static_config.qp_scale_compress_strength) {
            // clamp qstep_ratio so it doesn't get past the weight value
            qstep_ratio = MIN(weight, qstep_ratio);
        }
        int qindex_from_qstep_ratio = svt_av1_get_q_index_from_qstep_ratio(
            rc->active_worst_quality, qstep_ratio, scs->static_config.encoder_bit_depth);
        if (ppcs->sc_class1 && scs->passes == 1 && frame_is_intra_only(ppcs)) {
            qindex_from_qstep_ratio /= 2;
        }
        if (!frame_is_intra_only(ppcs)) {
            rc->arf_q = qindex_from_qstep_ratio;
        }
        active_best_quality  = clamp(qindex_from_qstep_ratio, rc->best_quality, rc->active_worst_quality);
        active_worst_quality = (active_best_quality + (3 * active_worst_quality) + 2) / 4;
    } else {
        int pyramid_level = ppcs->layer_depth;
        if (pyramid_level <= 1 || pyramid_level > MAX_ARF_LAYERS) {
            active_best_quality = get_active_best_quality(pcs, active_worst_quality);
        } else {
            active_best_quality = rc->active_best_quality[pyramid_level - 1] + 1;
            int w1              = svt_av1_non_base_qindex_weight_ref[hierarchical_levels];
            int w2              = svt_av1_non_base_qindex_weight_wq[hierarchical_levels];
            active_best_quality = (w1 * active_best_quality + (w2 * active_worst_quality) + ((w1 + w2) / 2)) /
                (w1 + w2);
        }
        // For alt_ref and GF frames (including internal arf frames) adjust the
        // worst allowed quality as well. This insures that even on hard
        // sections we don't clamp the Q at the same value for arf frames and
        // leaf (non arf) frames. This is important to the TPL model which assumes
        // Q drops with each arf level.
        if (!ppcs->is_overlay &&
            (ppcs->update_type == SVT_AV1_GF_UPDATE || ppcs->update_type == SVT_AV1_ARF_UPDATE || is_intrl_arf_boost)) {
            active_worst_quality = (active_best_quality + (3 * active_worst_quality) + 2) / 4;
        }
    }
    adjust_active_best_and_worst_quality_org(pcs, rc, &active_worst_quality, &active_best_quality);

    q = get_q(pcs, active_worst_quality, active_best_quality);

    // Special case when we are targeting the max allowed rate.
    if (ppcs->this_frame_target >= rc->max_frame_bandwidth && q > active_worst_quality) {
        active_worst_quality = q;
    }
    ppcs->top_index    = active_worst_quality;
    ppcs->bottom_index = active_best_quality;
    assert(ppcs->top_index <= rc->worst_quality && ppcs->top_index >= rc->best_quality);
    assert(ppcs->bottom_index <= rc->worst_quality && ppcs->bottom_index >= rc->best_quality);

    assert(q <= rc->worst_quality && q >= rc->best_quality);
    if (ppcs->update_type == SVT_AV1_ARF_UPDATE) {
        rc->arf_q = q;
    }

    return q;
}

static int NOINLINE find_min_ref_base_q_idx(PictureControlSet* pcs, RefList k) {
    int ref_base_q_idx = INT_MAX;
    int cnt            = (k == REF_LIST_0) ? pcs->ppcs->ref_list0_count_try : pcs->ppcs->ref_list1_count_try;
    for (int i = 0; i < cnt; i++) {
        EbReferenceObject* ref_obj  = get_ref_obj(pcs, k, i);
        bool               pic_used = ref_obj->tmp_layer_idx < pcs->temporal_layer_index || pcs->scs->use_flat_ipp;
        if (pcs->ref_slice_type[k][i] != I_SLICE && pic_used) {
            ref_base_q_idx = MIN(ref_base_q_idx, pcs->ref_base_q_idx[k][i]);
        }
    }
    return (ref_base_q_idx < INT_MAX) ? ref_base_q_idx : -1;
}

/******************************************************
 * svt_av1_rc_calc_qindex_rate_control
 * Calculates qindex for VBR/CBR rate control modes:
 * - Picks Q and bounds
 * - Limits QP based on reference frame QP
 ******************************************************/
void svt_av1_rc_calc_qindex_rate_control(PictureControlSet* pcs, SequenceControlSet* scs) {
    PictureParentControlSet* ppcs = pcs->ppcs;

    // Qindex calculating
    int32_t new_qindex;
    if (scs->enc_ctx->rc_cfg.mode == AOM_CBR) {
        new_qindex = rc_pick_q_and_bounds_no_stats_cbr(pcs);
    } else {
        new_qindex = rc_pick_q_and_bounds(pcs);
    }
    new_qindex = clamp_qindex(scs, new_qindex);

    // Limit the qindex based on the qindex of the reference frames
    if (pcs->temporal_layer_index != 0 && !scs->use_flat_ipp) {
        int list0_ref_base_q_idx = find_min_ref_base_q_idx(pcs, REF_LIST_0);
        int list1_ref_base_q_idx = find_min_ref_base_q_idx(pcs, REF_LIST_1);
        int ref_base_q_idx       = MAX(list0_ref_base_q_idx, list1_ref_base_q_idx);
        int limit                = scs->static_config.gop_constraint_rc ? 2 : 0;

        new_qindex = MAX(new_qindex, ref_base_q_idx - limit * 4);
    } else if (scs->enc_ctx->rc_cfg.mode == AOM_CBR) {
        int list0_ref_base_q_idx = find_min_ref_base_q_idx(pcs, REF_LIST_0);
        int list1_ref_base_q_idx = find_min_ref_base_q_idx(pcs, REF_LIST_1);
        int ref_base_q_idx       = MAX(list0_ref_base_q_idx, list1_ref_base_q_idx);
        int limit                = 4;

        new_qindex = MAX(new_qindex, ref_base_q_idx - limit * 4);
    } else if (ppcs->transition_present != 1 && pcs->slice_type != I_SLICE) {
        if (!scs->static_config.gop_constraint_rc) {
            uint64_t cur_dist = 0, ref_dist = 0;

            EbReferenceObject* ref_obj_l0 = get_ref_obj(pcs, REF_LIST_0, 0);
            for (uint32_t sb_index = 0; sb_index < pcs->b64_total_count; ++sb_index) {
                ref_dist += ref_obj_l0->sb_me_64x64_dist[sb_index];
                cur_dist += ppcs->me_64x64_distortion[sb_index];
            }

            int limit = 25;
            if (cur_dist > 3 * ref_dist || (ppcs->r0 - ref_obj_l0->r0 > 0)) {
                limit = 6;
            }
            int ref_base_q_idx = 0;
            if (pcs->ref_slice_type[REF_LIST_0][0] != I_SLICE) {
                ref_base_q_idx = pcs->ref_base_q_idx[REF_LIST_0][0];
            }
            if (pcs->slice_type == B_SLICE && ppcs->ref_list1_count_try &&
                pcs->ref_slice_type[REF_LIST_1][0] != I_SLICE) {
                ref_base_q_idx = MAX(ref_base_q_idx, pcs->ref_base_q_idx[REF_LIST_1][0]);
            }
            new_qindex = MAX(new_qindex, ref_base_q_idx - limit * 4);
        }
    }

    ppcs->frm_hdr.quantization_params.base_q_idx = clamp_qindex(scs, new_qindex);
}

static int av1_estimate_bits_at_q(FrameType frame_type, int q, int mbs, double correction_factor, EbBitDepth bit_depth,
                                  uint8_t is_screen_content_type) {
    int bpm = svt_av1_rc_bits_per_mb(frame_type, q, correction_factor, bit_depth, is_screen_content_type);
    return AOMMAX(FRAME_OVERHEAD_BITS, (int)((uint64_t)bpm * mbs) >> BPER_MB_NORMBITS);
}

static void av1_rc_update_rate_correction_factors(PictureParentControlSet* ppcs, int width, int height) {
    SequenceControlSet* scs                    = ppcs->scs;
    EncodeContext*      enc_ctx                = scs->enc_ctx;
    RATE_CONTROL*       rc                     = &enc_ctx->rc;
    CyclicRefresh*      cr                     = &ppcs->cyclic_refresh;
    FrameType           frame_type             = ppcs->frm_hdr.frame_type;
    EbBitDepth          bit_depth              = scs->static_config.encoder_bit_depth;
    int                 base_q_idx             = ppcs->frm_hdr.quantization_params.base_q_idx;
    int                 correction_factor      = 100;
    double              rate_correction_factor = get_rate_correction_factor(ppcs, width, height);
    double              adjustment_limit;
    int                 MBs = ((width + 15) / 16) * ((height + 15) / 16);

    // Do not update the rate factors for arf overlay frames.
    if (ppcs->is_overlay) {
        return;
    }
    // Work out how big we would have expected the frame to be at this Q given
    // the current correction factor.
    // Stay in double to avoid int overflow when values are large
    int projected_size_based_on_q = 0;
    if (cr->apply_cyclic_refresh) {
        // Weight for non-base segments
        double weight_segment1 = (double)cr->actual_num_seg1_sbs / ppcs->b64_total_count;
        double weight_segment2 = (double)cr->actual_num_seg2_sbs / ppcs->b64_total_count;

        // Take segment weighted average for estimated bits.
        projected_size_based_on_q = (int)round(
            (1.0 - weight_segment1 - weight_segment2) *
                av1_estimate_bits_at_q(
                    frame_type, base_q_idx, MBs, rate_correction_factor, bit_depth, ppcs->sc_class1) +
            weight_segment1 *
                av1_estimate_bits_at_q(frame_type,
                                       base_q_idx + cr->qindex_delta[CR_SEGMENT_ID_BOOST1],
                                       MBs,
                                       rate_correction_factor,
                                       bit_depth,
                                       ppcs->sc_class1) +
            weight_segment2 *
                av1_estimate_bits_at_q(frame_type,
                                       base_q_idx + cr->qindex_delta[CR_SEGMENT_ID_BOOST2],
                                       MBs,
                                       rate_correction_factor,
                                       bit_depth,
                                       ppcs->sc_class1));
    } else {
        projected_size_based_on_q = av1_estimate_bits_at_q(
            frame_type, base_q_idx, MBs, rate_correction_factor, bit_depth, ppcs->sc_class1);
    }
    // Work out a size correction factor.
    if (projected_size_based_on_q > FRAME_OVERHEAD_BITS) {
        correction_factor = (int)(100 * (int64_t)ppcs->projected_frame_size / projected_size_based_on_q);
    }
    // Clamp correction factor to prevent anything too extreme
    correction_factor = AOMMAX(correction_factor, 25);
    rc->q_2_frame     = rc->q_1_frame;
    rc->q_1_frame     = base_q_idx;
    rc->rc_2_frame    = rc->rc_1_frame;
    if (correction_factor > 110) {
        rc->rc_1_frame = -1;
    } else if (correction_factor < 90) {
        rc->rc_1_frame = 1;
    } else {
        rc->rc_1_frame = 0;
    }
    // Decide how heavily to dampen the adjustment
    if (enc_ctx->rc_cfg.mode == AOM_CBR) {
        if (correction_factor > 0) {
            if (ppcs->sc_class1) {
                adjustment_limit = 0.25 + 0.5 * AOMMIN(0.5, fabs(log10(0.01 * correction_factor)));
            } else {
                adjustment_limit = 0.25 + 0.75 * AOMMIN(0.5, fabs(log10(0.01 * correction_factor)));
            }
        } else {
            adjustment_limit = 0.75;
        }
    } else {
        if (correction_factor > 0) {
            adjustment_limit = 0.25 + 0.5 * AOMMIN(1, fabs(log10(0.01 * correction_factor)));
        } else {
            adjustment_limit = 0.75;
        }
    }
    // Adjustment to delta Q and number of blocks updated in cyclic refresh
    // based on over or under shoot of target in current frame.
    if (cr->apply_cyclic_refresh) {
        if (correction_factor > 125) {
            rc->percent_refresh_adjustment   = AOMMAX(rc->percent_refresh_adjustment - 1, -5);
            rc->rate_ratio_qdelta_adjustment = AOMMAX(rc->rate_ratio_qdelta_adjustment - 0.05, -0.0);
        } else if (correction_factor < 50) {
            rc->percent_refresh_adjustment   = AOMMIN(rc->percent_refresh_adjustment + 1, 5);
            rc->rate_ratio_qdelta_adjustment = AOMMIN(rc->rate_ratio_qdelta_adjustment + 0.05, 0.25);
        }
    }
    if (correction_factor > 101) {
        // We are not already at the worst allowable quality
        correction_factor      = (int)(100 + ((correction_factor - 100) * adjustment_limit));
        rate_correction_factor = rate_correction_factor * correction_factor / 100;
        // Keep rate_correction_factor within limits
        if (rate_correction_factor > MAX_BPB_FACTOR) {
            rate_correction_factor = MAX_BPB_FACTOR;
        }
    } else if (correction_factor < 99) {
        // We are not already at the best allowable quality
        double tmp_corr_fac    = 100 / (double)correction_factor;
        tmp_corr_fac           = (1.0 + ((tmp_corr_fac - 1.0) * adjustment_limit));
        tmp_corr_fac           = 1.0 / tmp_corr_fac;
        correction_factor      = (int)(100 * tmp_corr_fac);
        rate_correction_factor = rate_correction_factor * correction_factor / 100;

        // Keep rate_correction_factor within limits
        if (rate_correction_factor < MIN_BPB_FACTOR) {
            rate_correction_factor = MIN_BPB_FACTOR;
        }
    }

    set_rate_correction_factor(ppcs, rate_correction_factor, width, height);
}

// Update the buffer level: leaky bucket model.
static void update_buffer_level(PictureParentControlSet* ppcs, int encoded_frame_size) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;

    // Non-viewable frames are a special case and are treated as pure overhead.
    if (!ppcs->frm_hdr.showable_frame) {
        rc->bits_off_target -= encoded_frame_size;
    } else {
        rc->bits_off_target += rc->avg_frame_bandwidth - encoded_frame_size;
    }

    // Clip the buffer level to the maximum specified buffer size.
    rc->bits_off_target = AOMMIN(rc->bits_off_target, rc->maximum_buffer_size);
    rc->buffer_level    = rc->bits_off_target;
}

/*********************************************************************************************
 * Update the internal RC and TWO_PASS struct stats based on the received feedback
 ***********************************************************************************************/
void svt_av1_rc_postencode_update_gop_const(PictureParentControlSet* ppcs) {
    SequenceControlSet*              scs           = ppcs->scs;
    EncodeContext*                   enc_cont      = scs->enc_ctx;
    RATE_CONTROL*                    rc            = &enc_cont->rc;
    FrameHeader*                     frm_hdr       = &ppcs->frm_hdr;
    RateControlIntervalParamContext* rc_param_ptr  = ppcs->rate_control_param_ptr;
    int                              width         = ppcs->av1_cm->frm_size.frame_width;
    int                              height        = ppcs->av1_cm->frm_size.frame_height;
    int                              is_intrnl_arf = ppcs->update_type == SVT_AV1_INTNL_ARF_UPDATE;

    int qindex = frm_hdr->quantization_params.base_q_idx;

    // Update rate control heuristics
    ppcs->projected_frame_size = (int)ppcs->total_num_bits;
    // Post encode loop adjustment of Q prediction.
    av1_rc_update_rate_correction_factors(ppcs, width, height);

    // Keep a record of last Q and ambient average Q.
    if (frm_hdr->frame_type == KEY_FRAME) {
        rc->avg_frame_qindex[KEY_FRAME] = ROUND_POWER_OF_TWO(3 * rc->avg_frame_qindex[KEY_FRAME] + qindex, 2);
        rc->last_q[KEY_FRAME]           = (int32_t)svt_av1_convert_qindex_to_q(qindex, scs->encoder_bit_depth);
        svt_block_on_mutex(enc_cont->frame_updated_mutex);
        enc_cont->frame_updated = 0;
        svt_release_mutex(enc_cont->frame_updated_mutex);
    } else {
        svt_block_on_mutex(enc_cont->frame_updated_mutex);
        enc_cont->frame_updated++;
        svt_release_mutex(enc_cont->frame_updated_mutex);
        if (!ppcs->is_overlay &&
            !(ppcs->update_type == SVT_AV1_GF_UPDATE || ppcs->update_type == SVT_AV1_ARF_UPDATE || is_intrnl_arf)) {
            rc->avg_frame_qindex[INTER_FRAME] = ROUND_POWER_OF_TWO(3 * rc->avg_frame_qindex[INTER_FRAME] + qindex, 2);
            rc->last_q[INTER_FRAME]           = (int32_t)svt_av1_convert_qindex_to_q(qindex, scs->encoder_bit_depth);
        }
    }

    // Keep record of last boosted (KF/GF/ARF) Q value.
    // If the current frame is coded at a lower Q then we also update it.
    // If all mbs in this group are skipped only update if the Q value is
    // better than that already stored.
    // This is used to help set quality in forced key frames to reduce popping
    if (qindex < rc->last_boosted_qindex || frm_hdr->frame_type == KEY_FRAME ||
        (!rc->constrained_gf_group &&
         (ppcs->update_type == SVT_AV1_ARF_UPDATE || is_intrnl_arf ||
          (ppcs->update_type == SVT_AV1_GF_UPDATE && !ppcs->is_overlay)))) {
        rc->last_boosted_qindex = qindex;
    }
    update_buffer_level(ppcs, ppcs->projected_frame_size);
    rc->prev_avg_frame_bandwidth = rc->avg_frame_bandwidth;

    // Rolling monitors of whether we are over or underspending used to help
    // regulate min and Max Q in two pass.
    if (frm_hdr->frame_type != KEY_FRAME) {
        rc_param_ptr->rolling_target_bits = (int)ROUND_POWER_OF_TWO_64(
            rc_param_ptr->rolling_target_bits * 3 + ppcs->this_frame_target, 2);
        rc_param_ptr->rolling_actual_bits = (int)ROUND_POWER_OF_TWO_64(
            rc_param_ptr->rolling_actual_bits * 3 + ppcs->projected_frame_size, 2);
    }

    // Actual bits spent
    rc_param_ptr->total_actual_bits += ppcs->projected_frame_size;
    rc_param_ptr->total_target_bits += ppcs->frm_hdr.showable_frame ? rc->avg_frame_bandwidth : 0;

    if (frm_hdr->frame_type == KEY_FRAME) {
        rc->frames_since_key = 0;
    }
}

void svt_av1_rc_postencode_update(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs           = ppcs->scs;
    EncodeContext*      enc_ctx       = scs->enc_ctx;
    RATE_CONTROL*       rc            = &enc_ctx->rc;
    FrameHeader*        frm_hdr       = &ppcs->frm_hdr;
    int                 width         = ppcs->av1_cm->frm_size.frame_width;
    int                 height        = ppcs->av1_cm->frm_size.frame_height;
    int                 is_intrnl_arf = ppcs->update_type == SVT_AV1_INTNL_ARF_UPDATE;

    int qindex = frm_hdr->quantization_params.base_q_idx;

    // Update rate control heuristics
    ppcs->projected_frame_size = (int)ppcs->total_num_bits;
    // Post encode loop adjustment of Q prediction.
    av1_rc_update_rate_correction_factors(ppcs, width, height);

    // Keep a record of last Q and ambient average Q.
    if (frm_hdr->frame_type == KEY_FRAME) {
        rc->avg_frame_qindex[KEY_FRAME] = ROUND_POWER_OF_TWO(3 * rc->avg_frame_qindex[KEY_FRAME] + qindex, 2);
        rc->last_q[KEY_FRAME]           = (int32_t)svt_av1_convert_qindex_to_q(qindex, scs->encoder_bit_depth);
        svt_block_on_mutex(enc_ctx->frame_updated_mutex);
        enc_ctx->frame_updated = 0;
        svt_release_mutex(enc_ctx->frame_updated_mutex);
    } else {
        svt_block_on_mutex(enc_ctx->frame_updated_mutex);
        enc_ctx->frame_updated++;
        svt_release_mutex(enc_ctx->frame_updated_mutex);
        if (!ppcs->is_overlay &&
            !(ppcs->update_type == SVT_AV1_GF_UPDATE || ppcs->update_type == SVT_AV1_ARF_UPDATE || is_intrnl_arf)) {
            rc->avg_frame_qindex[INTER_FRAME] = ROUND_POWER_OF_TWO(3 * rc->avg_frame_qindex[INTER_FRAME] + qindex, 2);
            rc->last_q[INTER_FRAME]           = (int32_t)svt_av1_convert_qindex_to_q(qindex, scs->encoder_bit_depth);
        }
    }

    // Keep record of last boosted (KF/GF/ARF) Q value.
    // If the current frame is coded at a lower Q then we also update it.
    // If all mbs in this group are skipped only update if the Q value is
    // better than that already stored.
    // This is used to help set quality in forced key frames to reduce popping
    if (qindex < rc->last_boosted_qindex || frm_hdr->frame_type == KEY_FRAME ||
        (!rc->constrained_gf_group &&
         (ppcs->update_type == SVT_AV1_ARF_UPDATE || is_intrnl_arf ||
          (ppcs->update_type == SVT_AV1_GF_UPDATE && !ppcs->is_overlay)))) {
        rc->last_boosted_qindex = qindex;
    }
    update_buffer_level(ppcs, ppcs->projected_frame_size);
    rc->prev_avg_frame_bandwidth = rc->avg_frame_bandwidth;

    // Rolling monitors of whether we are over or underspending used to help
    // regulate min and Max Q in two pass.
    if (frm_hdr->frame_type != KEY_FRAME) {
        rc->rolling_target_bits = (int)ROUND_POWER_OF_TWO_64(rc->rolling_target_bits * 3 + ppcs->this_frame_target, 2);
        rc->rolling_actual_bits = (int)ROUND_POWER_OF_TWO_64(rc->rolling_actual_bits * 3 + ppcs->projected_frame_size,
                                                             2);
    }
    rc->avg_frame_low_motion = (rc->avg_frame_low_motion == 0)
        ? (int)(ppcs->child_pcs->avg_cnt_zeromv)
        : (int)((3 * rc->avg_frame_low_motion + ppcs->child_pcs->avg_cnt_zeromv) / 4);
    // Actual bits spent
    rc->total_actual_bits += ppcs->projected_frame_size;
    rc->total_target_bits += ppcs->frm_hdr.showable_frame ? rc->avg_frame_bandwidth : 0;

    if (frm_hdr->frame_type == KEY_FRAME) {
        rc->frames_since_key = 0;
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////
//
// Recode operation
//
///////////////////////////////////////////////////////////////////////////////////////////////////////////////

/**************************************************************************************************************
* get_kf_q_tpl()
* This function finds the q for a selected active quality for key frame. The functionality is the
* reverse of get_kf_active_quality_tpl()
**************************************************************************************************************/
static int get_kf_q_tpl(RATE_CONTROL* rc, int target_active_quality, EbBitDepth bit_depth) {
    const int* kf_low_motion_minq_cqp;
    const int* kf_high_motion_minq;
    ASSIGN_MINQ_TABLE(bit_depth, kf_low_motion_minq_cqp);
    ASSIGN_MINQ_TABLE(bit_depth, kf_high_motion_minq);
    int q              = rc->active_worst_quality;
    int active_quality = get_active_quality(
        q, rc->kf_boost, BOOST_KF_LOW, BOOST_KF_HIGH, kf_low_motion_minq_cqp, kf_high_motion_minq);
    int prev_dif = abs(target_active_quality - active_quality);
    while (abs(target_active_quality - active_quality) > 4 && abs(target_active_quality - active_quality) <= prev_dif) {
        if (active_quality > target_active_quality) {
            q--;
        } else {
            q++;
        }
        active_quality = get_active_quality(
            q, rc->kf_boost, BOOST_KF_LOW, BOOST_KF_HIGH, kf_low_motion_minq_cqp, kf_high_motion_minq);
    }
    return q;
}

/**************************************************************************************************************
* This function finds the q for a selected active quality for base layer frames.
* The functionality is the reverse of get_kf_active_quality_tpl()
**************************************************************************************************************/
static int get_gfu_q_tpl(RATE_CONTROL* rc, int target_active_quality, EbBitDepth bit_depth) {
    const int* arfgf_low_motion_minq;
    const int* arfgf_high_motion_minq;
    ASSIGN_MINQ_TABLE(bit_depth, arfgf_low_motion_minq);
    ASSIGN_MINQ_TABLE(bit_depth, arfgf_high_motion_minq);

    int q              = rc->active_worst_quality;
    int active_quality = get_active_quality(
        q, rc->gfu_boost, BOOST_GF_LOW_TPL_LA, BOOST_GF_HIGH_TPL_LA, arfgf_low_motion_minq, arfgf_high_motion_minq);

    int prev_dif = abs(target_active_quality - active_quality);
    while (abs(target_active_quality - active_quality) > 4 && abs(target_active_quality - active_quality) <= prev_dif) {
        if (active_quality > target_active_quality) {
            q--;
        } else {
            q++;
        }
        active_quality = get_active_quality(
            q, rc->gfu_boost, BOOST_GF_LOW_TPL_LA, BOOST_GF_HIGH_TPL_LA, arfgf_low_motion_minq, arfgf_high_motion_minq);
    }
    return q;
}

static double av1_get_compression_ratio(PictureParentControlSet* ppcs, size_t encoded_frame_size) {
    int             upscaled_width          = ppcs->av1_cm->frm_size.superres_upscaled_width;
    int             height                  = ppcs->av1_cm->frm_size.frame_height; //cm->height;
    int             luma_pic_size           = upscaled_width * height;
    EbAv1SeqProfile profile                 = ppcs->scs->seq_header.seq_profile;
    int             pic_size_profile_factor = profile == MAIN_PROFILE ? 15 : (profile == HIGH_PROFILE ? 30 : 36);
    encoded_frame_size                      = (encoded_frame_size > 129 ? encoded_frame_size - 128 : 1);
    size_t uncompressed_frame_size          = (luma_pic_size * pic_size_profile_factor) >> 3;
    return uncompressed_frame_size / (double)encoded_frame_size;
}

static void av1_rc_compute_frame_size_bounds(PictureParentControlSet* ppcs, int frame_target,
                                             int* frame_under_shoot_limit, int* frame_over_shoot_limit) {
    EncodeContext* enc_ctx = ppcs->scs->enc_ctx;
    RATE_CONTROL*  rc      = &enc_ctx->rc;
    if (enc_ctx->rc_cfg.mode == AOM_Q) {
        int tolerance            = (int)AOMMAX(100, (int64_t)enc_ctx->recode_tolerance * ppcs->max_frame_size / 100);
        *frame_under_shoot_limit = ppcs->loop_count ? AOMMAX(ppcs->max_frame_size - tolerance, 0) : 0;
        *frame_over_shoot_limit  = AOMMIN(ppcs->max_frame_size + tolerance, INT_MAX);
    } else {
        // For very small rate targets where the fractional adjustment
        // may be tiny make sure there is at least a minimum range.
        assert(enc_ctx->recode_tolerance <= 100);
        int tolerance            = (int)AOMMAX(100, (int64_t)enc_ctx->recode_tolerance * frame_target / 100);
        *frame_under_shoot_limit = AOMMAX(frame_target - tolerance, 0);
        *frame_over_shoot_limit  = AOMMIN(frame_target + tolerance, rc->max_frame_bandwidth);
    }
}

// get overshoot regulated q based on q_low
static int get_regulated_q_overshoot(PictureParentControlSet* ppcs, int q_low, int q_high, int top_index,
                                     int bottom_index) {
    int width  = ppcs->av1_cm->frm_size.frame_width;
    int height = ppcs->av1_cm->frm_size.frame_height;

    av1_rc_update_rate_correction_factors(ppcs, width, height);

    int q_regulated = av1_rc_regulate_q(ppcs, bottom_index, AOMMAX(q_high, top_index), width, height);
    int retries     = 0;
    while (q_regulated < q_low && retries < 10) {
        av1_rc_update_rate_correction_factors(ppcs, width, height);
        q_regulated = av1_rc_regulate_q(ppcs, bottom_index, AOMMAX(q_high, top_index), width, height);
        retries++;
    }
    return q_regulated;
}

// get undershoot regulated q based on q_high
static int get_regulated_q_undershoot(PictureParentControlSet* ppcs, int q_high, int top_index, int bottom_index) {
    int width  = ppcs->av1_cm->frm_size.frame_width;
    int height = ppcs->av1_cm->frm_size.frame_height;

    av1_rc_update_rate_correction_factors(ppcs, width, height);
    int q_regulated = av1_rc_regulate_q(ppcs, bottom_index, top_index, width, height);

    int retries = 0;
    while (q_regulated > q_high && retries < 10) {
        av1_rc_update_rate_correction_factors(ppcs, width, height);
        q_regulated = av1_rc_regulate_q(ppcs, bottom_index, top_index, width, height);
        retries++;
    }
    return q_regulated;
}

// Function to test for conditions that indicate we should loop
// back and recode a frame.
static AOM_INLINE int recode_loop_test(PictureParentControlSet* ppcs, int high_limit, int low_limit, int q, int maxq,
                                       int minq) {
    EncodeContext* enc_ctx          = ppcs->scs->enc_ctx;
    RATE_CONTROL*  rc               = &enc_ctx->rc;
    int            frame_is_kfgfarf = svt_aom_frame_is_kf_gf_arf(ppcs);
    int            force_recode     = 0;
    if (ppcs->projected_frame_size >= rc->max_frame_bandwidth || enc_ctx->recode_loop == ALLOW_RECODE ||
        (frame_is_kfgfarf && enc_ctx->recode_loop >= ALLOW_RECODE_KFMAXBW)) {
        // TODO(agrange) high_limit could be greater than the scale-down threshold.
        if ((ppcs->projected_frame_size > high_limit && q < maxq) ||
            (ppcs->projected_frame_size < low_limit && q > minq)) {
            force_recode = 1;
        }
    }
    return force_recode;
}

static int av1_find_qindex(double desired_q, EbBitDepth bit_depth, int best_qindex, int worst_qindex) {
    assert(best_qindex <= worst_qindex);
    int low  = best_qindex;
    int high = worst_qindex;
    while (low < high) {
        int    mid   = (low + high) >> 1;
        double mid_q = svt_av1_convert_qindex_to_q(mid, bit_depth);
        if (mid_q < desired_q) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    assert(low == high);
    assert(svt_av1_convert_qindex_to_q(low, bit_depth) >= desired_q || low == worst_qindex);
    return low;
}

// This function works out whether we under- or over-shot
// our bitrate target and adjusts q as appropriate.  Also decides whether
// or not we should do another recode loop, indicated by *loop
void recode_loop_update_q(PictureParentControlSet* ppcs, bool* const loop, int* const q, int* const q_low,
                          int* const q_high, const int top_index, const int bottom_index, int* const undershoot_seen,
                          int* const overshoot_seen, int* const low_cr_seen, const int loop_count) {
    SequenceControlSet* scs           = ppcs->scs;
    EncodeContext*      enc_ctx       = scs->enc_ctx;
    RATE_CONTROL*       rc            = &enc_ctx->rc;
    RateControlCfg*     rc_cfg        = &enc_ctx->rc_cfg;
    int                 do_dummy_pack = (scs->enc_ctx->recode_loop >= ALLOW_RECODE_KFMAXBW &&
                         !(rc_cfg->mode == AOM_Q && scs->static_config.max_bit_rate == 0)) ||
        rc_cfg->min_cr > 0;
    if (do_dummy_pack) {
        svt_block_on_mutex(ppcs->pcs_total_rate_mutex);
        ppcs->projected_frame_size = (int)((ppcs->pcs_total_rate + (1 << (AV1_PROB_COST_SHIFT - 1))) >>
                                           AV1_PROB_COST_SHIFT) +
            (ppcs->frm_hdr.frame_type == KEY_FRAME ? 13 : 0);
        svt_release_mutex(ppcs->pcs_total_rate_mutex);
    } else {
        ppcs->projected_frame_size = 0;
    }
    *loop = false;
    if (scs->enc_ctx->recode_loop == ALLOW_RECODE_KFMAXBW && ppcs->frm_hdr.frame_type != KEY_FRAME) {
        // skip re-encode for inter frame when setting -recode-loop 1
        return;
    }

    if (rc_cfg->min_cr > 0) {
        double compression_ratio = av1_get_compression_ratio(ppcs, ppcs->projected_frame_size >> 3);
        double target_cr         = rc_cfg->min_cr / 100.0;
        if (compression_ratio < target_cr) {
            *low_cr_seen = 1;
            if (*q < rc->worst_quality) {
                double cr_ratio    = target_cr / compression_ratio;
                int    projected_q = AOMMAX(*q + 1, (int)(*q * cr_ratio * cr_ratio));
                *q                 = AOMMIN(AOMMIN(projected_q, *q + 32), rc->worst_quality);
                *q_low             = AOMMAX(*q, *q_low);
                *q_high            = AOMMAX(*q, *q_high);
                *loop              = true;
            }
        }
        if (*low_cr_seen) {
            return;
        }
    }
    // Used for capped CRF. Update the active worse quality
    if (rc_cfg->mode == AOM_Q && scs->static_config.max_bit_rate) {
        if (ppcs->temporal_layer_index > 0) {
            return;
        } else {
            capped_crf_reencode(ppcs, q);
        }
    }
    int last_q                 = *q;
    int frame_over_shoot_limit = 0, frame_under_shoot_limit = 0;
    av1_rc_compute_frame_size_bounds(ppcs, ppcs->this_frame_target, &frame_under_shoot_limit, &frame_over_shoot_limit);
    if (frame_over_shoot_limit == 0) {
        frame_over_shoot_limit = 1;
    }

    if (recode_loop_test(
            ppcs, frame_over_shoot_limit, frame_under_shoot_limit, *q, AOMMAX(*q_high, top_index), bottom_index)) {
        int width  = ppcs->av1_cm->frm_size.frame_width;
        int height = ppcs->av1_cm->frm_size.frame_height;
        // Is the projected frame size out of range and are we allowed
        // to attempt to recode.

        // Frame size out of permitted range:
        // Update correction factor & compute new Q to try...
        // Frame is too large
        if (ppcs->projected_frame_size > ppcs->this_frame_target) {
            // Special case if the projected size is > the max allowed.
            if (*q == *q_high && ppcs->projected_frame_size >= rc->max_frame_bandwidth) {
                double q_val_high_current = svt_av1_convert_qindex_to_q(*q_high, scs->static_config.encoder_bit_depth);
                double q_val_high_new     = q_val_high_current *
                    ((double)ppcs->projected_frame_size / rc->max_frame_bandwidth);
                *q_high = av1_find_qindex(
                    q_val_high_new, scs->static_config.encoder_bit_depth, rc->best_quality, rc->worst_quality);
            }
            // Raise Qlow as to at least the current value
            *q_low = AOMMIN(*q + 1, *q_high);

            if (*undershoot_seen || loop_count > 2 || (loop_count == 2 && !frame_is_intra_only(ppcs))) {
                av1_rc_update_rate_correction_factors(ppcs, width, height);

                *q = (*q_high + *q_low + 1) / 2;
            } else if (loop_count == 2 && frame_is_intra_only(ppcs)) {
                int q_mid       = (*q_high + *q_low + 1) / 2;
                int q_regulated = get_regulated_q_overshoot(ppcs, *q_low, *q_high, top_index, bottom_index);
                // Get 'q' in-between 'q_mid' and 'q_regulated' for a smooth
                // transition between loop_count < 2 and loop_count > 2.
                *q = (q_mid + q_regulated + 1) / 2;
            } else {
                *q = get_regulated_q_overshoot(ppcs, *q_low, *q_high, top_index, bottom_index);
            }

            *overshoot_seen = 1;
        } else {
            // Frame is too small
            *q_high = AOMMAX(*q - 1, *q_low);

            if (*overshoot_seen || loop_count > 2 || (loop_count == 2 && !frame_is_intra_only(ppcs))) {
                av1_rc_update_rate_correction_factors(ppcs, width, height);
                *q = (*q_high + *q_low) / 2;
            } else if (loop_count == 2 && frame_is_intra_only(ppcs)) {
                int q_mid       = (*q_high + *q_low) / 2;
                int q_regulated = get_regulated_q_undershoot(ppcs, *q_high, top_index, bottom_index);
                // Get 'q' in-between 'q_mid' and 'q_regulated' for a smooth
                // transition between loop_count < 2 and loop_count > 2.
                *q = (q_mid + q_regulated) / 2;
            } else {
                *q = get_regulated_q_undershoot(ppcs, *q_high, top_index, bottom_index);
            }

            *undershoot_seen = 1;
        }

        // Clamp Q to upper and lower limits:
        *q = clamp(*q, *q_low, *q_high);
    }

    *q    = clamp_qindex(scs, *q);
    *loop = (*q != last_q);
    // Used for capped CRF. Update the active worse quality based on the final assigned qindex.
    // cppcheck claims that `*loop == 0` is always true here, but that's a false positive based on the assumption that
    // the recode_loop_test is true branch is not taken.
    // cppcheck-suppress knownConditionTrueFalse
    if (rc_cfg->mode == AOM_Q && scs->static_config.max_bit_rate && *loop == 0 && ppcs->loop_count > 0) {
        if (ppcs->slice_type == I_SLICE) {
            rc->active_worst_quality = get_kf_q_tpl(rc, *q, scs->static_config.encoder_bit_depth);
        } else {
            rc->active_worst_quality = get_gfu_q_tpl(rc, *q, scs->static_config.encoder_bit_depth);
        }

        rc->active_worst_quality = clamp_qindex(scs, rc->active_worst_quality);
    }
}
