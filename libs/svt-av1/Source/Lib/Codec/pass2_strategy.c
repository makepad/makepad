/*
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 3-Clause Clear License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */
#include <stdint.h>

#include "definitions.h"
#include "pass2_strategy.h"
#include "rc_process.h"
#include "firstpass.h"
#include "sequence_control_set.h"
#include "entropy_coding.h"
#include "pd_process.h"

// Calculate a modified Error used in distributing bits between easier and
// harder frames.
static double calculate_modified_err(TWO_PASS* twopass, const FIRSTPASS_STATS* this_frame) {
    FIRSTPASS_STATS* stats = twopass->stats_buf_ctx->total_stats;
    if (stats == NULL) {
        return 0;
    }
    return (double)this_frame->stat_struct.total_num_bits;
}

// Resets the first pass file to the given position using a relative seek from
// the current position.
static void reset_fpf_position(TWO_PASS* p, const FIRSTPASS_STATS* position) {
    p->stats_in = position;
}

static int input_stats(TWO_PASS* p, FIRSTPASS_STATS* fps) {
    if (p->stats_in >= p->stats_buf_ctx->stats_in_end) {
        return EOF;
    }

    *fps = *p->stats_in;
    ++p->stats_in;
    return 1;
}

static void subtract_stats(FIRSTPASS_STATS* section, FIRSTPASS_STATS* frame) {
    section->frame -= frame->frame;
    section->coded_error -= frame->coded_error;
    section->count -= frame->count;
    section->duration -= frame->duration;
}

// This function returns the maximum target rate per frame.
static int frame_max_bits(RATE_CONTROL* rc, EncodeContext* enc_ctx) {
    int64_t max_bits = (int64_t)rc->avg_frame_bandwidth * enc_ctx->two_pass_cfg.vbrmax_section / 100;
    return (int)CLIP3(0, rc->max_frame_bandwidth, max_bits);
}

static const double q_pow_term[(QINDEX_RANGE >> 5) + 1] = {0.65, 0.70, 0.75, 0.80, 0.85, 0.90, 0.95, 0.95, 0.95};
#define ERR_DIVISOR 96.0

static double calc_correction_factor(double err_per_mb, int q) {
    double error_term = err_per_mb / ERR_DIVISOR;
    int    index      = q >> 5;
    // Adjustment to power term based on qindex
    double power_term = q_pow_term[index] + (((q_pow_term[index + 1] - q_pow_term[index]) * (q % 32)) / 32.0);
    assert(error_term >= 0.0);
    return fclamp(pow(error_term, power_term), 0.05, 5.0);
}

static int qbpm_enumerator(int rate_err_tol) {
    return 1250000 + ((300000 * AOMMIN(75, AOMMAX(rate_err_tol - 25, 0))) / 75);
}

// Similar to find_qindex_by_rate() function in ratectrl.c, but includes
// calculation of a correction_factor.
static int find_qindex_by_rate_with_correction(int desired_bits_per_mb, EbBitDepth bit_depth, double error_per_mb,
                                               double group_weight_factor, int rate_err_tol, int best_qindex,
                                               int worst_qindex) {
    assert(best_qindex <= worst_qindex);
    int low  = best_qindex;
    int high = worst_qindex;

    while (low < high) {
        int    mid             = (low + high) >> 1;
        double mid_factor      = calc_correction_factor(error_per_mb, mid);
        double q               = svt_av1_convert_qindex_to_q(mid, bit_depth);
        int    enumerator      = qbpm_enumerator(rate_err_tol);
        int    mid_bits_per_mb = (int)((enumerator * mid_factor * group_weight_factor) / q);

        if (mid_bits_per_mb > desired_bits_per_mb) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    return low;
}

/*!\brief Choose a target maximum Q for a group of frames
 *
 * \ingroup rate_control
 *
 * This function is used to estimate a suitable maximum Q for a
 * group of frames. Inititally it is called to get a crude estimate
 * for the whole clip. It is then called for each ARF/GF group to get
 * a revised estimate for that group.
 *
 * \param[in]    cpi                 Top-level encoder structure
 * \param[in]    av_frame_err        The average per frame coded error score
 *                                   for frames making up this section/group.
 * \param[in]    inactive_zone       Used to mask off /ignore part of the
 *                                   frame. The most common use case is where
 *                                   a wide format video (e.g. 16:9) is
 *                                   letter-boxed into a more square format.
 *                                   Here we want to ignore the bands at the
 *                                   top and bottom.
 * \param[in]    av_target_bandwidth The target bits per frame
 * \param[in]    group_weight_factor A correction factor allowing the algorithm
 *                                   to correct for errors over time.
 *
 * \return The maximum Q for frames in the group.
 */
static int get_twopass_worst_quality(PictureParentControlSet* pcs, double section_err, double inactive_zone,
                                     int section_target_bandwidth, double group_weight_factor) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    RateControlCfg*     rc_cfg  = &enc_ctx->rc_cfg;
    uint32_t            mb_cols;
    uint32_t            mb_rows;
    if (scs->first_pass_downsample) {
        mb_cols = 2 * (scs->max_input_luma_width + 16 - 1) / 16;
        mb_rows = 2 * (scs->max_input_luma_height + 16 - 1) / 16;
    } else {
        mb_cols = (scs->max_input_luma_width + 16 - 1) / 16;
        mb_rows = (scs->max_input_luma_height + 16 - 1) / 16;
    }
    inactive_zone = fclamp(inactive_zone, 0.0, 1.0);

    if (section_target_bandwidth <= 0) {
        return rc->worst_quality; // Highest value allowed
    }

    int num_mbs = mb_cols * mb_rows;
    //(oxcf->resize_cfg.resize_mode != RESIZE_NONE)
    //    ? cpi->initial_mbs
    //    : cpi->common.mi_params.MBs;
    int    active_mbs              = AOMMAX(1, num_mbs - (int)(num_mbs * inactive_zone));
    double av_err_per_mb           = section_err / active_mbs;
    int    target_norm_bits_per_mb = (int)(((uint64_t)section_target_bandwidth << BPER_MB_NORMBITS) / active_mbs);

    int rate_err_tol = AOMMIN(rc_cfg->under_shoot_pct, rc_cfg->over_shoot_pct);

    // Try and pick a max Q that will be high enough to encode the
    // content at the given rate.
    int q = find_qindex_by_rate_with_correction(target_norm_bits_per_mb,
                                                scs->encoder_bit_depth,
                                                av_err_per_mb,
                                                group_weight_factor,
                                                rate_err_tol,
                                                rc->best_quality,
                                                rc->worst_quality);
    return q;
}

static void accumulate_this_frame_stats(FIRSTPASS_STATS* stats, double mod_frame_err, GF_GROUP_STATS* gf_stats) {
    gf_stats->gf_group_err += mod_frame_err;
    gf_stats->gf_group_raw_error += stats->coded_error;
}

// Calculate the total bits to allocate in this GF/ARF group.
/*!\brief Calculates the bit target for this GF/ARF group
 *
 * \ingroup rate_control
 *
 * Calculates the total bits to allocate in this GF/ARF group.
 *
 * \param[in]    cpi              Top-level encoder structure
 * \param[in]    gf_group_err     Cumulative coded error score for the
 *                                frames making up this group.
 *
 * \return The target total number of bits for this GF/ARF group.
 */
static int64_t calculate_total_gf_group_bits(PictureParentControlSet* pcs, double gf_group_err) {
    SequenceControlSet* scs      = pcs->scs;
    EncodeContext*      enc_ctx  = scs->enc_ctx;
    RATE_CONTROL*       rc       = &enc_ctx->rc;
    TWO_PASS*           twopass  = &scs->twopass;
    int                 max_bits = frame_max_bits(rc, enc_ctx);
    int64_t             total_group_bits;
    // Calculate the bits to be allocated to the group as a whole.
    if ((twopass->kf_group_bits > 0) && (twopass->kf_group_error_left > 0)) {
        int64_t kf_group_bits;
        if (scs->lap_rc &&
            (scs->lad_mg + 1) * (1 << scs->static_config.hierarchical_levels) <
                scs->static_config.intra_period_length) {
            kf_group_bits = (int64_t)twopass->kf_group_bits * MIN(pcs->frames_in_sw, rc->frames_to_key) /
                rc->frames_to_key;
        } else {
            kf_group_bits = twopass->kf_group_bits;
        }
        total_group_bits = (int64_t)(kf_group_bits * (gf_group_err / twopass->kf_group_error_left));
    } else {
        total_group_bits = 0;
    }

    // Clamp odd edge cases.
    total_group_bits = (total_group_bits < 0)         ? 0
        : (total_group_bits > twopass->kf_group_bits) ? twopass->kf_group_bits
                                                      : total_group_bits;

    // Clip based on user supplied data rate variability limit.
    if (total_group_bits > (int64_t)max_bits * rc->baseline_gf_interval) {
        total_group_bits = (int64_t)max_bits * rc->baseline_gf_interval;
    }
    twopass->kf_group_bits = AOMMAX(twopass->kf_group_bits - total_group_bits, 0);
    return total_group_bits;
}

/****************************************************************************************************
* Allocate the rate per frame based on the rate allocation of previous pass with same prediction
* structure and using cross multiplying
****************************************************************************************************/
static void av1_gop_bit_allocation_same_pred(PictureParentControlSet* pcs, int64_t gf_group_bits,
                                             GF_GROUP_STATS gf_stats) {
    // For key frames the frame target rate is already set
    int frame_index = (pcs->slice_type == I_SLICE) ? 1 : 0;
    for (int idx = frame_index; idx < pcs->gf_interval; ++idx) {
        assert(gf_stats.gf_group_err != 0);
        pcs->gf_group[idx]->base_frame_target = (int)(gf_group_bits * pcs->gf_group[idx]->stat_struct.total_num_bits /
                                                      gf_stats.gf_group_err);
    }
}

// Allocate bits to each frame in a GF / ARF group
static double layer_fraction[MAX_ARF_LAYERS + 1] = {1.0, 0.80, 0.7, 0.60, 0.60, 1.0, 1.0};

static void allocate_gf_group_bits(PictureParentControlSet* pcs, RATE_CONTROL* rc, int64_t gf_group_bits,
                                   int gf_arf_bits, int gf_interval, int key_frame, int use_arf) {
    int64_t total_group_bits = gf_group_bits;
    int     base_frame_bits;
    int     layer_frames[MAX_ARF_LAYERS + 1] = {0};

    // Subtract the extra bits set aside for ARF frames from the Group Total
    if (use_arf || !key_frame) {
        total_group_bits -= gf_arf_bits;
    }

    if (rc->baseline_gf_interval) {
        base_frame_bits = (int)(total_group_bits / rc->baseline_gf_interval);
    } else {
        base_frame_bits = 1;
    }

    // For key frames the frame target rate is already set
    int frame_index = key_frame ? 1 : 0;

    // Check the number of frames in each layer in case we have a
    // non standard group length.
    int max_arf_layer = pcs->hierarchical_levels;
    for (int idx = frame_index; idx < pcs->gf_interval; ++idx) {
        if ((pcs->gf_group[idx]->update_type == SVT_AV1_ARF_UPDATE) ||
            (pcs->gf_group[idx]->update_type == SVT_AV1_INTNL_ARF_UPDATE)) {
            layer_frames[pcs->gf_group[idx]->layer_depth]++;
        }
    }
    if (rc->baseline_gf_interval < (gf_interval >> 1)) {
        for (int idx = frame_index; idx < pcs->gf_interval; ++idx) {
            if (pcs->gf_group[idx]->update_type == SVT_AV1_ARF_UPDATE) {
                layer_frames[pcs->gf_group[idx]->layer_depth] += 1;
            }
            if (pcs->gf_group[idx]->update_type == SVT_AV1_INTNL_ARF_UPDATE) {
                layer_frames[pcs->gf_group[idx]->layer_depth] += 2;
            }
        }
    }
    // Allocate extra bits to each ARF layer
    int layer_extra_bits[MAX_ARF_LAYERS + 1] = {0};
    for (int i = 1; i <= max_arf_layer; ++i) {
        if (layer_frames[i]) { // to make sure there is a picture with the depth
            double fraction     = (i == max_arf_layer) ? 1.0 : layer_fraction[i];
            layer_extra_bits[i] = (int)((gf_arf_bits * fraction) / AOMMAX(1, layer_frames[i]));
            gf_arf_bits -= (int)(gf_arf_bits * fraction);
        }
    }

    // Now combine ARF layer and baseline bits to give total bits for each frame.
    int arf_extra_bits;
    for (int idx = frame_index; idx < pcs->gf_interval; ++idx) {
        switch (pcs->gf_group[idx]->update_type) {
        case SVT_AV1_ARF_UPDATE:
        case SVT_AV1_INTNL_ARF_UPDATE:
            arf_extra_bits                        = layer_extra_bits[pcs->gf_group[idx]->layer_depth];
            pcs->gf_group[idx]->base_frame_target = base_frame_bits + arf_extra_bits;
            break;
        case SVT_AV1_INTNL_OVERLAY_UPDATE:
        case SVT_AV1_OVERLAY_UPDATE:
            pcs->gf_group[idx]->base_frame_target = 0;
            break;
        default:
            pcs->gf_group[idx]->base_frame_target = base_frame_bits;
            break;
        }
    }
}

#define RC_FACTOR_MIN_GOP_CONST 0.5
#define RC_FACTOR_MIN_1P_VBR 1
#define RC_FACTOR_MIN 0.75
#define RC_FACTOR_MAX 2

static INLINE void set_baseline_gf_interval(PictureParentControlSet* pcs, int arf_position) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    if (frame_is_intra_only(pcs) && pcs->idr_flag) {
        rc->baseline_gf_interval = MAX(arf_position - 1, 1);
    } else {
        rc->baseline_gf_interval = pcs->gf_interval;
    }
}

// initialize GF_GROUP_STATS
static void init_gf_stats(GF_GROUP_STATS* gf_stats) {
    gf_stats->gf_group_err                  = 0.0;
    gf_stats->gf_stat_struct.poc            = 0;
    gf_stats->gf_stat_struct.total_num_bits = 1;
    gf_stats->gf_stat_struct.qindex         = 172;
    gf_stats->gf_stat_struct.worst_qindex   = 172;
    gf_stats->gf_group_raw_error            = 0.0;
    gf_stats->gf_group_skip_pct             = 0.0;
    gf_stats->gf_group_inactive_zone_rows   = 0.0;
}

/***********************************************************************************
* calculate_gf_stats()
* calculate the gf group stat by looping over frames within the gf
************************************************************************************/
static void calculate_gf_stats(PictureParentControlSet* ppcs, GF_GROUP_STATS* gf_stats, FIRSTPASS_STATS* this_frame,
                               int* use_alt_ref) {
    SequenceControlSet*    scs     = ppcs->scs;
    RATE_CONTROL*          rc      = &scs->enc_ctx->rc;
    TWO_PASS*              twopass = &scs->twopass;
    FIRSTPASS_STATS        next_frame;
    const FIRSTPASS_STATS* start_pos = twopass->stats_in;

    init_gf_stats(gf_stats);

    // Load stats for the current frame.
    double mod_frame_err = calculate_modified_err(twopass, this_frame);

    // Note the error of the frame at the start of the group. This will be
    // the GF frame error if we code a normal gf.

    // If this is a key frame or the overlay from a previous arf then
    // the error score / cost of this frame has already been accounted for.
    // There is no overlay support for now
    if (frame_is_intra_only(ppcs)) {
        gf_stats->gf_group_err -= mod_frame_err;
        gf_stats->gf_group_raw_error -= this_frame->coded_error;
    }
    gf_stats->gf_stat_struct = this_frame->stat_struct;
    int i                    = 0;
    while (i < ppcs->gf_interval) {
        ++i;
        // Accumulate error score of frames in this gf group.
        mod_frame_err = calculate_modified_err(twopass, this_frame);
        // accumulate stats for this frame
        accumulate_this_frame_stats(this_frame, mod_frame_err, gf_stats);

        // read in the next frame
        if (EOF == input_stats(twopass, &next_frame)) {
            break;
        }
        *this_frame = next_frame;
    }

    // Was the group length constrained by the requirement for a new KF?
    rc->constrained_gf_group = (i >= rc->frames_to_key) ? 1 : 0;
    *use_alt_ref             = (i > 2);
    set_baseline_gf_interval(ppcs, i);
    // Reset the file position.
    reset_fpf_position(twopass, start_pos);
}

/***********************************************************************************
 * calculate_active_worst_quality()
 * Calculate an estimate of the maxq needed for the group.
 ************************************************************************************/
static void calculate_active_worst_quality(PictureParentControlSet* ppcs, GF_GROUP_STATS gf_stats) {
    SequenceControlSet* scs        = ppcs->scs;
    RATE_CONTROL*       rc         = &scs->enc_ctx->rc;
    FrameInfo*          frame_info = &scs->enc_ctx->frame_info;

    // Calculate an estimate of the maxq needed for the group.
    // We are more agressive about correcting for sections
    // where there could be significant overshoot than for easier
    // sections where we do not wish to risk creating an overshoot
    // of the allocated bit budget.
    if (rc->baseline_gf_interval > 1) {
        int    vbr_group_bits_per_frame = (int)(rc->gf_group_bits / rc->baseline_gf_interval);
        double group_av_err             = gf_stats.gf_group_raw_error / rc->baseline_gf_interval;
        double group_av_skip_pct        = gf_stats.gf_group_skip_pct / rc->baseline_gf_interval;
        double group_av_inactive_zone   = ((gf_stats.gf_group_inactive_zone_rows * 2) /
                                         (rc->baseline_gf_interval * (double)frame_info->mb_rows));

        int tmp_q;
        // rc factor is a weight factor that corrects for local rate control drift.
        double  rc_factor = 1.0;
        int64_t bits      = scs->static_config.target_bit_rate; //oxcf->target_bandwidth;

        if (bits > 0) {
            int rate_error;

            rate_error = (int)((rc->vbr_bits_off_target * 100) / bits);
            rate_error = clamp(rate_error, -100, 100);
            if (rate_error > 0) {
                double rc_factor_min = scs->static_config.gop_constraint_rc ? RC_FACTOR_MIN_GOP_CONST
                    : (scs->static_config.pass == ENC_SINGLE_PASS)          ? RC_FACTOR_MIN_1P_VBR
                                                                            : RC_FACTOR_MIN;
                rc_factor            = AOMMAX(rc_factor_min, (double)(100 - rate_error) / 100.0);
            } else {
                rc_factor = AOMMIN(RC_FACTOR_MAX, (double)(100 - rate_error) / 100.0);
            }
        }
        tmp_q = get_twopass_worst_quality(
            ppcs, group_av_err, (group_av_skip_pct + group_av_inactive_zone), vbr_group_bits_per_frame, rc_factor);
        if (scs->twopass.passes == 2) {
            int     ref_qindex           = gf_stats.gf_stat_struct.worst_qindex;
            double  ref_q                = svt_av1_convert_qindex_to_q(ref_qindex, scs->encoder_bit_depth);
            int64_t ref_gf_group_bits    = (int64_t)(gf_stats.gf_group_err);
            int64_t target_gf_group_bits = rc->gf_group_bits;
            {
                int low  = rc->best_quality;
                int high = rc->worst_quality;

                while (low < high) {
                    int    mid      = (low + high) >> 1;
                    double q        = svt_av1_convert_qindex_to_q(mid, scs->encoder_bit_depth);
                    int    mid_bits = (int)(ref_gf_group_bits * ref_q * rc_factor / q);

                    if (mid_bits > target_gf_group_bits) {
                        low = mid + 1;
                    } else {
                        high = mid;
                    }
                }
                tmp_q = low;
            }
        }
        rc->active_worst_quality = AOMMAX(tmp_q, rc->active_worst_quality >> 1);
    }
}

static void av1_gop_bit_allocation(PictureParentControlSet* ppcs, RATE_CONTROL* rc, int is_key_frame, int gf_interval,
                                   int use_arf, int64_t gf_group_bits) {
    // Calculate the extra bits to be used for boosted frame(s)
    int gf_arf_bits = svt_av1_calculate_boost_bits(rc->baseline_gf_interval, rc->gfu_boost, gf_group_bits);
    // Allocate bits to each of the frames in the GF group.
    allocate_gf_group_bits(ppcs, rc, gf_group_bits, gf_arf_bits, gf_interval, is_key_frame, use_arf);
}

/*!\brief Assign rate to the GF group or mini gop.
 *
 * \ingroup gf_group_algo
 * This function assigns setup the various parameters regarding bit-allocation and quality setup.
 *
 * \param[in]    pcs         PictureParentControlSet
 * \param[in]    this_frame      First pass statistics structure
 *
 * \return Nothing is returned. Instead, enc_ctx->gf_group is changed.
 */
static void gf_group_rate_assingment(PictureParentControlSet* pcs, FIRSTPASS_STATS* this_frame) {
    SequenceControlSet*    scs       = pcs->scs;
    EncodeContext*         enc_ctx   = scs->enc_ctx;
    RATE_CONTROL*          rc        = &enc_ctx->rc;
    TWO_PASS*              twopass   = &scs->twopass;
    const FIRSTPASS_STATS* start_pos = twopass->stats_in;
    GF_GROUP_STATS         gf_stats;
    int                    use_alt_ref;
    calculate_gf_stats(pcs, &gf_stats, this_frame, &use_alt_ref);

    // Calculate the bits to be allocated to the gf/arf group as a whole
    rc->gf_group_bits = calculate_total_gf_group_bits(pcs, gf_stats.gf_group_err);
    // Calculate an estimate of the maxq needed for the group.
    calculate_active_worst_quality(pcs, gf_stats);

    // Adjust KF group bits and error remaining.
    twopass->kf_group_error_left -= (int64_t)gf_stats.gf_group_err;

    // Reset the file position.
    reset_fpf_position(twopass, start_pos);
    if (twopass->passes == 2 && scs->static_config.pass == ENC_SECOND_PASS) {
        av1_gop_bit_allocation_same_pred(pcs, rc->gf_group_bits, gf_stats);
    } else {
        av1_gop_bit_allocation(pcs,
                               rc,
                               pcs->frm_hdr.frame_type == KEY_FRAME,
                               (1 << scs->static_config.hierarchical_levels),
                               use_alt_ref,
                               rc->gf_group_bits);
    }
}

/*!\brief Variable initialization for lap_rc
 *
 * This function initialized some variable for lap_rc by looping over the look ahead
 *
 */
static void lap_rc_init(PictureParentControlSet* pcs, FIRSTPASS_STATS this_frame) {
    SequenceControlSet*    scs                  = pcs->scs;
    EncodeContext*         enc_ctx              = scs->enc_ctx;
    TWO_PASS*              twopass              = &scs->twopass;
    int                    num_stats            = 0;
    double                 modified_error_total = 0.0;
    double                 coded_error_total    = 0.0;
    const FIRSTPASS_STATS* start_position       = twopass->stats_in;
    FIRSTPASS_STATS        this_frame_ref       = this_frame;

    // loop over the look ahead and calculate the coded error
    while (twopass->stats_in <= twopass->stats_buf_ctx->stats_in_end) {
        // Accumulate total number of stats available till end of the look ahead
        num_stats++;
        // Accumulate error.
        coded_error_total += this_frame.coded_error;
        // Load the next frame's stats.
        if (input_stats(twopass, &this_frame) == EOF) {
            break;
        }
    }
    // Calculate modified_error_min and modified_error_max which is needed in modified_error_total
    // calculation
    double avg_error = coded_error_total / DOUBLE_DIVIDE_CHECK(num_stats);

    twopass->modified_error_min = (avg_error * enc_ctx->two_pass_cfg.vbrmin_section) / 100;
    twopass->modified_error_max = (avg_error * enc_ctx->two_pass_cfg.vbrmax_section) / 100;
    reset_fpf_position(twopass, start_position);
    this_frame = this_frame_ref;

    // loop over the look ahead and calculate the modified_error_total
    while (twopass->stats_in <= twopass->stats_buf_ctx->stats_in_end) {
        // Accumulate error.
        modified_error_total += calculate_modified_err(twopass, &this_frame);
        // Load the next frame's stats.
        if (input_stats(twopass, &this_frame) == EOF) {
            break;
        }
    }

    twopass->modified_error_left = modified_error_total;
    twopass->bits_left += (int64_t)(num_stats * (scs->static_config.target_bit_rate / scs->new_framerate));
    reset_fpf_position(twopass, start_position);
}

/*!\brief calculating group_error for lap_rc
 *
 * This function calculates group error for lap_rc by looping over the look ahead
 *
 */
static double lap_rc_group_error_calc(PictureParentControlSet* pcs, FIRSTPASS_STATS this_frame) {
    SequenceControlSet*    scs                  = pcs->scs;
    TWO_PASS*              twopass              = &scs->twopass;
    int                    num_stats            = 0;
    double                 modified_error_total = 0.0;
    const FIRSTPASS_STATS* start_position       = twopass->stats_in;

    // loop over the look ahead and calculate the modified_error_total
    while (twopass->stats_in <= twopass->stats_buf_ctx->stats_in_end && num_stats < scs->enc_ctx->rc.frames_to_key) {
        num_stats++;
        // Accumulate error.
        modified_error_total += calculate_modified_err(twopass, &this_frame);
        // Load the next frame's stats.
        if (input_stats(twopass, &this_frame) == EOF) {
            break;
        }
    }
    reset_fpf_position(twopass, start_position);
    return modified_error_total;
}

/*!\brief Sets frames to key.
 *
 * \ingroup gf_group_algo
 * This function sets the frames to key for different scenarios
 *
 * \param[in]    this_frame       Pointer to first pass stats
 * \param[out]   kf_group_err     The total error in the KF group
 * \param[in]    num_frames_to_detect_scenecut Maximum lookahead frames.
 *
 * \return       Number of frames to the next key.
 */
static void set_kf_interval_variables(PictureParentControlSet* pcs, FIRSTPASS_STATS* this_frame, double* kf_group_err,
                                      int num_frames_to_detect_scenecut) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    TWO_PASS*           twopass = &scs->twopass;

    int frames_to_key = 0;
    if (num_frames_to_detect_scenecut == 0) {
        return;
    }

    while (twopass->stats_in <= twopass->stats_buf_ctx->stats_in_end && frames_to_key < num_frames_to_detect_scenecut) {
        // Accumulate total number of stats available till next key frame

        // Accumulate kf group error.
        if (kf_group_err != NULL) {
            *kf_group_err += calculate_modified_err(twopass, this_frame);
        }

        ++frames_to_key;
        if (input_stats(twopass, this_frame) == EOF) {
            break;
        }
    }
    if (scs->lap_rc && pcs->end_of_sequence_region) {
        pcs->rate_control_param_ptr->end_of_seq_seen = 1;
    }
    if (scs->lap_rc && !pcs->end_of_sequence_region) {
        rc->frames_to_key = scs->static_config.intra_period_length + 1;
    } else {
        rc->frames_to_key = AOMMIN((scs->static_config.intra_period_length + 1), frames_to_key);
    }
}

static int64_t get_kf_group_bits(PictureParentControlSet* pcs, double kf_group_err) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    TWO_PASS*           twopass = &scs->twopass;
    int64_t             kf_group_bits;
    if (scs->lap_rc && pcs->frames_in_sw < scs->static_config.intra_period_length && !pcs->end_of_sequence_region) {
        kf_group_bits = (int64_t)rc->frames_to_key * rc->avg_frame_bandwidth;
    } else {
        kf_group_bits = (int64_t)(twopass->bits_left * (kf_group_err / twopass->modified_error_left));
    }

    return kf_group_bits;
}

#define MAX_KF_BITS_INTERVAL_SINGLE_PASS 5

/*****************************************************************************/
// kf_group_rate_assingment
// Rate assignment for the next kf group
// Only works for key frames, and scene change is not detected for now
/*****************************************************************************/
static void kf_group_rate_assingment(PictureParentControlSet* pcs, FIRSTPASS_STATS this_frame) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    TWO_PASS*           twopass = &scs->twopass;
    FIRSTPASS_STATS     next_frame;
    av1_zero(next_frame);

    rc->frames_since_key = 0;

    const FIRSTPASS_STATS*       start_position = twopass->stats_in;
    int                          kf_bits        = 0;
    double                       kf_mod_err;
    double                       kf_group_err          = 0.0;
    int                          frames_to_key_clipped = INT_MAX;
    int64_t                      kf_group_bits_clipped = INT64_MAX;

    twopass->kf_group_bits       = 0; // Total bits available to kf group
    twopass->kf_group_error_left = 0; // Group modified error score.
    kf_mod_err                   = calculate_modified_err(twopass, &this_frame);
    set_kf_interval_variables(pcs, &this_frame, &kf_group_err, scs->static_config.intra_period_length + 1);

    // Calculate the number of bits that should be assigned to the kf group.
    if ((twopass->bits_left > 0 && twopass->modified_error_left > 0.0) || scs->lap_rc) {
        // Maximum number of bits for a single normal frame (not key frame).
        int max_bits = frame_max_bits(rc, enc_ctx);

        // Maximum number of bits allocated to the key frame group.
        int64_t max_grp_bits;

        // Default allocation based on bits left and relative
        // complexity of the section.
        twopass->kf_group_bits = get_kf_group_bits(pcs, kf_group_err /*, kf_group_avg_error*/);
        // Clip based on maximum per frame rate defined by the user.
        max_grp_bits = (int64_t)max_bits * (int64_t)rc->frames_to_key;
        if (twopass->kf_group_bits > max_grp_bits) {
            twopass->kf_group_bits = max_grp_bits;
        }
    } else {
        twopass->kf_group_bits = 0;
    }
    twopass->kf_group_bits = AOMMAX(0, twopass->kf_group_bits);
    if (scs->lap_rc) {
        // For 1 PASS VBR, as the lookahead is moving, the bits left is recalculated for the next KF. The second term is added again as it is part of look ahead of the next KF
        twopass->bits_left -= (twopass->kf_group_bits +
                               (int64_t)(((int64_t)pcs->frames_in_sw - (int64_t)rc->frames_to_key) *
                                         (scs->static_config.target_bit_rate / scs->new_framerate)));
    } else {
        twopass->bits_left = AOMMAX(twopass->bits_left - twopass->kf_group_bits, 0);
    }
    if (scs->lap_rc) {
        // In the case of single pass based on LAP, frames to  key may have an
        // inaccurate value, and hence should be clipped to an appropriate
        // interval.
        frames_to_key_clipped = (int)(MAX_KF_BITS_INTERVAL_SINGLE_PASS * scs->new_framerate);
        // This variable calculates the bits allocated to kf_group with a clipped
        // frames_to_key.
        if (rc->frames_to_key > frames_to_key_clipped) {
            kf_group_bits_clipped = (int64_t)((double)twopass->kf_group_bits * frames_to_key_clipped /
                                              rc->frames_to_key);
        }
    }
    // Reset the first pass file position.
    reset_fpf_position(twopass, start_position);

    // Store the zero motion percentage
    twopass->kf_zeromotion_pct = 0;
    // Work out how many bits to allocate for the key frame itself.
    // In case of LAP enabled for VBR, if the frames_to_key value is
    // very high, we calculate the bits based on a clipped value of
    // frames_to_key.
    if (twopass->passes == 2) {
        kf_bits = (int)(twopass->kf_group_bits * (twopass->stats_in - 1)->stat_struct.total_num_bits / kf_group_err);
    } else {
        kf_bits = svt_av1_calculate_boost_bits(AOMMIN(rc->frames_to_key, frames_to_key_clipped) - 1,
                                               rc->kf_boost,
                                               AOMMIN(twopass->kf_group_bits, kf_group_bits_clipped));
    }

    twopass->kf_group_bits -= kf_bits;

    // Save the bits to spend on the key frame.
    pcs->base_frame_target = kf_bits;

    // Note the total error score of the kf group minus the key frame itself.
    twopass->kf_group_error_left = (int64_t)(kf_group_err - kf_mod_err);

    // Adjust the count of total modified error left.
    // The count of bits left is adjusted elsewhere based on real coded frame
    // sizes.
    twopass->modified_error_left -= kf_group_err;
}

#define DEFAULT_GRP_WEIGHT 1.0

static int get_section_target_bandwidth(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    TWO_PASS*           twopass = &scs->twopass;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    int                 section_target_bandwidth;
    int                 frames_left = (int)(twopass->stats_buf_ctx->total_stats->count - pcs->picture_number);
    if (scs->lap_rc) {
        section_target_bandwidth = (int)rc->avg_frame_bandwidth;
    } else {
        section_target_bandwidth = (int)(twopass->bits_left / frames_left);
    }
    return section_target_bandwidth;
}

static void process_first_pass_stats(PictureParentControlSet* pcs, FIRSTPASS_STATS* this_frame) {
    SequenceControlSet* scs     = pcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    TWO_PASS*           twopass = &scs->twopass;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    RateControlCfg*     rc_cfg  = &enc_ctx->rc_cfg;
    if (pcs->picture_number == 0 && twopass->stats_buf_ctx->total_stats && twopass->stats_buf_ctx->total_left_stats) {
        if (scs->lap_rc) {
            /*
       * Accumulate total_stats using available limited number of stats,
       * and assign it to total_left_stats.
       */
            *twopass->stats_buf_ctx->total_left_stats = *twopass->stats_buf_ctx->total_stats;
        }
        // Special case code for first frame.
        int    section_target_bandwidth = get_section_target_bandwidth(pcs);
        double section_length           = twopass->stats_buf_ctx->total_left_stats->count;
        double section_error            = twopass->stats_buf_ctx->total_left_stats->coded_error / section_length;
        int    tmp_q;
        if (scs->passes == 2) {
            int     ref_qindex           = twopass->stats_buf_ctx->stats_in_start->stat_struct.worst_qindex;
            double  ref_q                = svt_av1_convert_qindex_to_q(ref_qindex, scs->encoder_bit_depth);
            int64_t ref_gf_group_bits    = (int64_t)(twopass->stats_buf_ctx->total_stats->stat_struct.total_num_bits);
            int64_t target_gf_group_bits = twopass->bits_left;
            {
                int low  = rc->best_quality;
                int high = rc->worst_quality;

                while (low < high) {
                    int    mid      = (low + high) >> 1;
                    double q        = svt_av1_convert_qindex_to_q(mid, scs->encoder_bit_depth);
                    int    mid_bits = (int)(ref_gf_group_bits * ref_q / q);

                    if (mid_bits > target_gf_group_bits) {
                        low = mid + 1;
                    } else {
                        high = mid;
                    }
                }
                tmp_q = low;
            }
        } else {
            tmp_q = get_twopass_worst_quality(pcs, section_error, 0, section_target_bandwidth, DEFAULT_GRP_WEIGHT);
        }

        rc->active_worst_quality          = tmp_q;
        rc->avg_frame_qindex[INTER_FRAME] = tmp_q;
        rc->avg_frame_qindex[KEY_FRAME]   = (tmp_q + rc_cfg->best_allowed_q) / 2;
    }

    if (input_stats(twopass, this_frame) == EOF) {
        return;
    }

    // Update the total stats remaining structure.
    if (twopass->stats_buf_ctx->total_left_stats) {
        subtract_stats(twopass->stats_buf_ctx->total_left_stats, this_frame);
    }
}

// Calculates is new gf group and stores in pcs->is_new_gf_group
// For P pictures in the incomplete minigops, since there is no order, we search all of them and set the flag accordingly
static void is_new_gf_group(PictureParentControlSet* pcs) {
    pcs->is_new_gf_group = 0;
    if (!svt_aom_is_incomp_mg_frame(pcs)) {
        pcs->is_new_gf_group = pcs->gf_update_due;
    } else {
        for (int pic_i = 0; pic_i < pcs->gf_interval; ++pic_i) {
            // For P-pictures, since the pictures might get released and replaced by other pictures, we check the POC difference
            if (pcs->gf_group[pic_i] &&
                (int)ABS((int64_t)pcs->gf_group[pic_i]->picture_number - (int64_t)pcs->picture_number) <=
                    pcs->gf_interval &&
                svt_aom_is_incomp_mg_frame(pcs->gf_group[pic_i]) && pcs->gf_group[pic_i]->gf_update_due) {
                pcs->is_new_gf_group = 1;
            }
        }
        if (pcs->is_new_gf_group) {
            for (int pic_i = 0; pic_i < pcs->gf_interval; ++pic_i) {
                if (pcs->gf_group[pic_i]) {
                    pcs->gf_group[pic_i]->gf_update_due = 0;
                }
            }
        }
    }
}

void svt_aom_process_rc_stat(PictureParentControlSet* pcs) {
    SequenceControlSet* scs     = pcs->scs;
    TWO_PASS*           twopass = &scs->twopass;
    FIRSTPASS_STATS     this_frame;
    av1_zero(this_frame);
    process_first_pass_stats(pcs, &this_frame);

    // Keyframe and section processing.
    if (frame_is_intra_only(pcs) && pcs->idr_flag) {
        if (scs->lap_rc) {
            lap_rc_init(pcs, this_frame);
        }
        // Rate assignment for the next kf group
        kf_group_rate_assingment(pcs, this_frame);
    }
    // Define a new GF/ARF group. (Should always enter here for key frames).
    is_new_gf_group(pcs);
    if (pcs->is_new_gf_group) {
        // For 1 pass VBR, as the look ahead moves, the kf_group_error_left changes. It is because in modified_error calculation, av_weight and av_err depand on total_stats, which gets updated.
        // So, we recalculate kf_group_error_left for each mini gop except the first one after KF
        if (!(frame_is_intra_only(pcs) && pcs->idr_flag) && scs->lap_rc) {
            twopass->kf_group_error_left = (int)lap_rc_group_error_calc(pcs, this_frame);
        }

        gf_group_rate_assingment(pcs, &this_frame);
    }
}

// Max rate target for 1080P and below encodes under normal circumstances
// (1920 * 1080 / (16 * 16)) * MAX_MB_RATE bits per MB
#define MAX_MB_RATE 250
#define MAXRATE_1080P 2025000

static void av1_rc_update_framerate(SequenceControlSet* scs) {
    EncodeContext* enc_ctx    = scs->enc_ctx;
    RATE_CONTROL*  rc         = &enc_ctx->rc;
    FrameInfo*     frame_info = &enc_ctx->frame_info;
    int            vbr_max_bits;
    int            MBs = frame_info->num_mbs;

    rc->avg_frame_bandwidth = (int)(scs->static_config.target_bit_rate / scs->new_framerate);
    // A maximum bitrate for a frame is defined.
    // The baseline for this aligns with HW implementations that
    // can support decode of 1080P content up to a bitrate of MAX_MB_RATE bits
    // per 16x16 MB (averaged over a frame). However this limit is extended if
    // a very high rate is given on the command line or the rate cannot
    // be achieved because of a user specified max q (e.g. when the user
    // specifies lossless encode.
    vbr_max_bits            = (int)(((int64_t)rc->avg_frame_bandwidth * enc_ctx->two_pass_cfg.vbrmax_section) / 100);
    vbr_max_bits            = AOMMIN(vbr_max_bits, INT_MAX);
    rc->max_frame_bandwidth = AOMMAX(AOMMAX((MBs * MAX_MB_RATE), MAXRATE_1080P), vbr_max_bits);
}

// from aom encoder.c
void svt_av1_new_framerate(SequenceControlSet* scs, double framerate) {
    scs->new_framerate = framerate < 0.1 ? 30 : framerate;
    av1_rc_update_framerate(scs);
}

void svt_aom_set_rc_param(SequenceControlSet* scs) {
    EncodeContext* enc_ctx    = scs->enc_ctx;
    FrameInfo*     frame_info = &enc_ctx->frame_info;

    if (scs->first_pass_downsample) {
        frame_info->frame_width  = scs->max_input_luma_width << 1;
        frame_info->frame_height = scs->max_input_luma_height << 1;
        frame_info->mb_cols      = ((scs->max_input_luma_width + 16 - 1) / 16) << 1;
        frame_info->mb_rows      = ((scs->max_input_luma_height + 16 - 1) / 16) << 1;
    } else {
        frame_info->frame_width  = scs->max_input_luma_width;
        frame_info->frame_height = scs->max_input_luma_height;
        frame_info->mb_cols      = (scs->max_input_luma_width + 16 - 1) / 16;
        frame_info->mb_rows      = (scs->max_input_luma_height + 16 - 1) / 16;
    }
    frame_info->num_mbs   = frame_info->mb_cols * frame_info->mb_rows;
    frame_info->bit_depth = scs->static_config.encoder_bit_depth;
    // input config  from options
    enc_ctx->two_pass_cfg.vbrmin_section = scs->static_config.vbr_min_section_pct;
    enc_ctx->two_pass_cfg.vbrmax_section = scs->static_config.vbr_max_section_pct;
    enc_ctx->rc_cfg.mode                 = scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_VBR
                        ? AOM_VBR
                        : (scs->static_config.rate_control_mode == SVT_AV1_RC_MODE_CBR ? AOM_CBR : AOM_Q);
    enc_ctx->rc_cfg.best_allowed_q       = (int32_t)quantizer_to_qindex[scs->static_config.min_qp_allowed];
    enc_ctx->rc_cfg.worst_allowed_q      = (int32_t)quantizer_to_qindex[scs->static_config.max_qp_allowed];

    if (scs->static_config.gop_constraint_rc) {
        enc_ctx->rc_cfg.over_shoot_pct  = 0;
        enc_ctx->rc_cfg.under_shoot_pct = 0;
    } else {
        enc_ctx->rc_cfg.over_shoot_pct  = scs->static_config.over_shoot_pct;
        enc_ctx->rc_cfg.under_shoot_pct = scs->static_config.under_shoot_pct;
    }
    bool is_vbr                              = enc_ctx->rc_cfg.mode == AOM_VBR;
    enc_ctx->rc_cfg.maximum_buffer_size_ms   = is_vbr ? 240000 : scs->static_config.maximum_buffer_size_ms;
    enc_ctx->rc_cfg.starting_buffer_level_ms = is_vbr ? 60000 : scs->static_config.starting_buffer_level_ms;
    enc_ctx->rc_cfg.optimal_buffer_level_ms  = is_vbr ? 60000 : scs->static_config.optimal_buffer_level_ms;

    // todo: to expose to a cli parameter
    enc_ctx->rc_cfg.max_intra_bitrate_pct = 300;
    enc_ctx->rc_cfg.max_inter_bitrate_pct = 0; // default

    enc_ctx->sf_cfg.sframe_dist = scs->static_config.sframe_dist;
    enc_ctx->sf_cfg.sframe_mode = scs->static_config.sframe_mode;
}

/******************************************************
 * Read Stat from File
 * reads StatStruct per frame from the file and stores under pcs
 ******************************************************/
static void read_stat_from_file(SequenceControlSet* scs) {
    TWO_PASS*        twopass                                = &scs->twopass;
    FIRSTPASS_STATS* this_frame                             = (FIRSTPASS_STATS*)twopass->stats_in;
    uint64_t         total_num_bits                         = 0;
    uint64_t         previous_num_bits[MAX_TEMPORAL_LAYERS] = {0};
    while (this_frame < twopass->stats_buf_ctx->stats_in_end) {
        if (this_frame->stat_struct.total_num_bits == 0) {
            this_frame->stat_struct.total_num_bits = previous_num_bits[this_frame->stat_struct.temporal_layer_index];
        }
        previous_num_bits[this_frame->stat_struct.temporal_layer_index] = this_frame->stat_struct.total_num_bits;
        total_num_bits += this_frame->stat_struct.total_num_bits;
        this_frame++;
    }
    twopass->stats_buf_ctx->total_stats->stat_struct.total_num_bits = total_num_bits;
}

void svt_av1_init_single_pass_lap(SequenceControlSet* scs) {
    TWO_PASS*      twopass = &scs->twopass;
    EncodeContext* enc_ctx = scs->enc_ctx;
    if (!twopass->stats_buf_ctx->stats_in_end) {
        return;
    }

    svt_aom_set_rc_param(scs);

    // This variable monitors how far behind the second ref update is lagging.

    twopass->bits_left           = 0;
    twopass->modified_error_min  = 0.0;
    twopass->modified_error_max  = 0.0;
    twopass->modified_error_left = 0.0;

    // Reset the vbr bits off target counters
    enc_ctx->rc.vbr_bits_off_target      = 0;
    enc_ctx->rc.vbr_bits_off_target_fast = 0;

    enc_ctx->rc.rate_error_estimate = 0;

    // Static sequence monitor variables.
    twopass->kf_zeromotion_pct = 100;
}

void svt_av1_init_second_pass(SequenceControlSet* scs) {
    TWO_PASS*      twopass = &scs->twopass;
    EncodeContext* enc_ctx = scs->enc_ctx;

    double           frame_rate;
    FIRSTPASS_STATS* stats;

    if (!twopass->stats_buf_ctx->stats_in_end) {
        return;
    }
    {
        svt_av1_twopass_zero_stats(twopass->stats_buf_ctx->stats_in_end);
        FIRSTPASS_STATS* this_frame     = (FIRSTPASS_STATS*)scs->twopass.stats_in;
        uint64_t         total_num_bits = 0;

        while (this_frame < scs->twopass.stats_buf_ctx->stats_in_end) {
            if (twopass->stats_buf_ctx->stats_in_end != NULL) {
                svt_av1_accumulate_stats(twopass->stats_buf_ctx->stats_in_end, this_frame);
            }
            total_num_bits += this_frame->stat_struct.total_num_bits;
            this_frame++;
        }
        twopass->stats_buf_ctx->stats_in_end->stat_struct.total_num_bits = total_num_bits;
    }
    svt_aom_set_rc_param(scs);
    stats                                     = twopass->stats_buf_ctx->total_stats;
    *stats                                    = *twopass->stats_buf_ctx->stats_in_end;
    *twopass->stats_buf_ctx->total_left_stats = *stats;

    frame_rate = 10000000.0 * stats->count / stats->duration;
    // Each frame can have a different duration, as the frame rate in the source
    // isn't guaranteed to be constant. The frame rate prior to the first frame
    // encoded in the second pass is a guess. However, the sum duration is not.
    // It is calculated based on the actual durations of all frames from the
    // first pass.
    svt_av1_new_framerate(scs, frame_rate);
    twopass->bits_left = (int64_t)(stats->duration * (int64_t)scs->static_config.target_bit_rate / 10000000.0);
    read_stat_from_file(scs);

    // Scan the first pass file and calculate a modified total error based upon
    // the bias/power function used to allocate bits.
    {
        double                 avg_error            = stats->coded_error / DOUBLE_DIVIDE_CHECK(stats->count);
        const FIRSTPASS_STATS* s                    = twopass->stats_in;
        double                 modified_error_total = 0.0;
        twopass->modified_error_min                 = (avg_error * enc_ctx->two_pass_cfg.vbrmin_section) / 100;
        twopass->modified_error_max                 = (avg_error * enc_ctx->two_pass_cfg.vbrmax_section) / 100;
        while (s < twopass->stats_buf_ctx->stats_in_end) {
            modified_error_total += calculate_modified_err(twopass, s);
            ++s;
        }
        twopass->modified_error_left = modified_error_total;
    }

    // Reset the vbr bits off target counters
    enc_ctx->rc.vbr_bits_off_target      = 0;
    enc_ctx->rc.vbr_bits_off_target_fast = 0;
    enc_ctx->rc.rate_error_estimate      = 0;

    // Static sequence monitor variables.
    twopass->kf_zeromotion_pct = 100;
}

/*********************************************************************************************
 * Update the internal RC and TWO_PASS struct stats based on the received feedback
 ***********************************************************************************************/
void svt_av1_twopass_postencode_update_gop_const(PictureParentControlSet* ppcs) {
    SequenceControlSet*              scs          = ppcs->scs;
    EncodeContext*                   enc_cont     = scs->enc_ctx;
    RATE_CONTROL*                    rc           = &enc_cont->rc;
    RateControlCfg*                  rc_cfg       = &enc_cont->rc_cfg;
    RateControlIntervalParamContext* rc_param_ptr = ppcs->rate_control_param_ptr;

    // VBR correction is done through rc->vbr_bits_off_target. Based on the
    // sign of this value, a limited % adjustment is made to the target rate
    // of subsequent frames, to try and push it back towards 0. This method
    // is designed to prevent extreme behaviour at the end of a clip
    // or group of frames.
    rc_param_ptr->vbr_bits_off_target += ppcs->base_frame_target - ppcs->projected_frame_size;
    // Target vs actual bits for this arf group.
    int rate_error_estimate_target = 0;
    // Calculate the pct rc error.
    if (rc_param_ptr->total_actual_bits) {
        if (rc_param_ptr->total_target_bits) {
            rate_error_estimate_target = (int)((rc_param_ptr->vbr_bits_off_target * 100) /
                                               rc_param_ptr->total_target_bits);
        }
        rc_param_ptr->rate_error_estimate = (int)((rc_param_ptr->vbr_bits_off_target * 100) /
                                                  rc_param_ptr->total_actual_bits);
        rc_param_ptr->rate_error_estimate = clamp(rc_param_ptr->rate_error_estimate, -100, 100);
    } else {
        rc_param_ptr->rate_error_estimate = 0;
    }

    // Update the active best quality pyramid.
    if (!ppcs->is_overlay) {
        int pyramid_level = ppcs->layer_depth;
        int i;
        for (i = pyramid_level; i <= MAX_ARF_LAYERS; ++i) {
            rc->active_best_quality[i] = ppcs->frm_hdr.quantization_params.base_q_idx;
        }
    }

    // If the rate control is drifting consider adjustment to min or maxq.
    if (!ppcs->is_overlay) {
        int maxq_adj_limit = rc->worst_quality - rc->active_worst_quality;
        int minq_adj_limit = MINQ_ADJ_LIMIT;

        // Undershoot.
        if (rc_param_ptr->rate_error_estimate > rc_cfg->under_shoot_pct) {
            --rc_param_ptr->extend_maxq;
            if (rc_param_ptr->rolling_target_bits >= rc_param_ptr->rolling_actual_bits) {
                ++rc_param_ptr->extend_minq;
            }
            // Overshoot.
        } else if (rc_param_ptr->rate_error_estimate < -rc_cfg->over_shoot_pct) {
            --rc_param_ptr->extend_minq;
            if (rc_param_ptr->rolling_target_bits < rc_param_ptr->rolling_actual_bits) {
                rc_param_ptr->extend_maxq += (scs->is_short_clip) ? rate_error_estimate_target < -100 ? 10 : 2 : 1;
            }
        } else {
            // Adjustment for extreme local overshoot.
            if (ppcs->projected_frame_size > (2 * ppcs->base_frame_target) &&
                ppcs->projected_frame_size > (2 * rc->avg_frame_bandwidth)) {
                ++rc_param_ptr->extend_maxq;
            }

            // Unwind undershoot or overshoot adjustment.
            if (rc_param_ptr->rolling_target_bits < rc_param_ptr->rolling_actual_bits) {
                --rc_param_ptr->extend_minq;
            } else if (rc_param_ptr->rolling_target_bits > rc_param_ptr->rolling_actual_bits) {
                --rc_param_ptr->extend_maxq;
            }
            if (scs->is_short_clip) {
                if (rc_param_ptr->extend_minq > minq_adj_limit / 3) {
                    rc_param_ptr->extend_minq -= 5;
                }
                if (rc_param_ptr->extend_maxq < -maxq_adj_limit / 3) {
                    rc_param_ptr->extend_maxq += 5;
                }
            }
        }
        if (scs->is_short_clip) {
            rc_param_ptr->extend_minq = clamp(rc_param_ptr->extend_minq, -minq_adj_limit / 4, minq_adj_limit);
        } else {
            rc_param_ptr->extend_minq = clamp(rc_param_ptr->extend_minq, 0, minq_adj_limit);
        }
        if (!scs->is_short_clip) {
            rc_param_ptr->extend_maxq = clamp(rc_param_ptr->extend_maxq, 0, maxq_adj_limit);
        }

        // If there is a big and undexpected undershoot then feed the extra
        // bits back in quickly. One situation where this may happen is if a
        // frame is unexpectedly almost perfectly predicted by the ARF or GF
        // but not very well predcited by the previous frame.
        if (!svt_aom_frame_is_kf_gf_arf(ppcs) && !ppcs->is_overlay) {
            int fast_extra_thresh = ppcs->base_frame_target / HIGH_UNDERSHOOT_RATIO;
            if (ppcs->projected_frame_size < fast_extra_thresh && rc_param_ptr->rate_error_estimate > 0) {
                rc_param_ptr->vbr_bits_off_target_fast += fast_extra_thresh - ppcs->projected_frame_size;
                rc_param_ptr->vbr_bits_off_target_fast = AOMMIN(rc_param_ptr->vbr_bits_off_target_fast,
                                                                (4 * rc->avg_frame_bandwidth));

                // Fast adaptation of minQ if necessary to use up the extra bits.
                if (rc->avg_frame_bandwidth) {
                    rc_param_ptr->extend_minq_fast = (int)(rc_param_ptr->vbr_bits_off_target_fast * 8 /
                                                           rc->avg_frame_bandwidth);
                }
                rc_param_ptr->extend_minq_fast = AOMMIN(rc_param_ptr->extend_minq_fast,
                                                        minq_adj_limit - rc_param_ptr->extend_minq);
            } else if (rc_param_ptr->vbr_bits_off_target_fast) {
                rc_param_ptr->extend_minq_fast = AOMMIN(rc_param_ptr->extend_minq_fast,
                                                        minq_adj_limit - rc_param_ptr->extend_minq);
            } else {
                rc_param_ptr->extend_minq_fast = 0;
            }
        }
    }
}

void svt_av1_twopass_postencode_update(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    TWO_PASS*           twopass = &scs->twopass;
    RateControlCfg*     rc_cfg  = &enc_ctx->rc_cfg;

    // VBR correction is done through rc->vbr_bits_off_target. Based on the
    // sign of this value, a limited % adjustment is made to the target rate
    // of subsequent frames, to try and push it back towards 0. This method
    // is designed to prevent extreme behaviour at the end of a clip
    // or group of frames.
    rc->vbr_bits_off_target += ppcs->base_frame_target - ppcs->projected_frame_size;
    // Target vs actual bits for this arf group.
    int rate_error_estimate_target = 0;
    // Calculate the pct rc error.
    if (rc->total_actual_bits) {
        if (rc->total_target_bits) {
            rate_error_estimate_target = (int)((rc->vbr_bits_off_target * 100) / rc->total_target_bits);
        }
        rc->rate_error_estimate = (int)((rc->vbr_bits_off_target * 100) / rc->total_actual_bits);
        rc->rate_error_estimate = clamp(rc->rate_error_estimate, -100, 100);
    } else {
        rc->rate_error_estimate = 0;
    }

    // Update the active best quality pyramid.
    if (!ppcs->is_overlay) {
        int pyramid_level = ppcs->layer_depth;
        int i;
        for (i = pyramid_level; i <= MAX_ARF_LAYERS; ++i) {
            rc->active_best_quality[i] = ppcs->frm_hdr.quantization_params.base_q_idx;
        }
    }

    // If the rate control is drifting consider adjustment to min or maxq.
    if (!ppcs->is_overlay) {
        int maxq_adj_limit = rc->worst_quality - rc->active_worst_quality;
        int minq_adj_limit = MINQ_ADJ_LIMIT;

        // Undershoot.
        if (rc->rate_error_estimate > rc_cfg->under_shoot_pct) {
            --twopass->extend_maxq;
            if (rc->rolling_target_bits >= rc->rolling_actual_bits) {
                ++twopass->extend_minq;
            }
            // Overshoot.
        } else if (rc->rate_error_estimate < -rc_cfg->over_shoot_pct) {
            --twopass->extend_minq;
            if (rc->rolling_target_bits < rc->rolling_actual_bits) {
                twopass->extend_maxq += (scs->is_short_clip) ? rate_error_estimate_target < -100 ? 10 : 2 : 1;
            }
        } else {
            // Adjustment for extreme local overshoot.
            if (ppcs->projected_frame_size > (2 * ppcs->base_frame_target) &&
                ppcs->projected_frame_size > (2 * rc->avg_frame_bandwidth)) {
                ++twopass->extend_maxq;
            }

            // Unwind undershoot or overshoot adjustment.
            if (rc->rolling_target_bits < rc->rolling_actual_bits) {
                --twopass->extend_minq;
            } else if (rc->rolling_target_bits > rc->rolling_actual_bits) {
                --twopass->extend_maxq;
            }
        }

        twopass->extend_minq = clamp(twopass->extend_minq, 0, minq_adj_limit);
        if (!scs->is_short_clip) {
            twopass->extend_maxq = clamp(twopass->extend_maxq, 0, maxq_adj_limit);
        }

        // If there is a big and undexpected undershoot then feed the extra
        // bits back in quickly. One situation where this may happen is if a
        // frame is unexpectedly almost perfectly predicted by the ARF or GF
        // but not very well predcited by the previous frame.
        if (!svt_aom_frame_is_kf_gf_arf(ppcs) && !ppcs->is_overlay) {
            int fast_extra_thresh = ppcs->base_frame_target / HIGH_UNDERSHOOT_RATIO;
            if (ppcs->projected_frame_size < fast_extra_thresh && rc->rate_error_estimate > 0) {
                rc->vbr_bits_off_target_fast += fast_extra_thresh - ppcs->projected_frame_size;
                rc->vbr_bits_off_target_fast = AOMMIN(rc->vbr_bits_off_target_fast, (4 * rc->avg_frame_bandwidth));

                // Fast adaptation of minQ if necessary to use up the extra bits.
                if (rc->avg_frame_bandwidth) {
                    twopass->extend_minq_fast = (int)(rc->vbr_bits_off_target_fast * 8 / rc->avg_frame_bandwidth);
                }
                twopass->extend_minq_fast = AOMMIN(twopass->extend_minq_fast, minq_adj_limit - twopass->extend_minq);
            } else if (rc->vbr_bits_off_target_fast) {
                twopass->extend_minq_fast = AOMMIN(twopass->extend_minq_fast, minq_adj_limit - twopass->extend_minq);
            } else {
                twopass->extend_minq_fast = 0;
            }
        }
    }
}
