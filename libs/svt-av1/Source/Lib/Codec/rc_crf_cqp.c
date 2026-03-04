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
#include "utility.h"
#include "md_process.h"

#include "rc_process.h"

#define KB 400

static uint8_t NOINLINE clamp_qp(SequenceControlSet* scs, int qp) {
    int qmin = scs->static_config.min_qp_allowed;
    int qmax = scs->static_config.max_qp_allowed;
    return (uint8_t)CLIP3(qmin, qmax, qp);
}

static uint8_t NOINLINE clamp_qindex(SequenceControlSet* scs, int qindex) {
    int qmin = quantizer_to_qindex[scs->static_config.min_qp_allowed];
    int qmax = quantizer_to_qindex[scs->static_config.max_qp_allowed];
    return (uint8_t)CLIP3(qmin, qmax, qindex);
}

static EbReferenceObject* get_ref_obj(PictureControlSet* pcs, RefList ref_list, int idx) {
    return pcs->ref_pic_ptr_array[ref_list][idx]->object_ptr;
}

/******************************************************
 * svt_aom_crf_assign_max_rate
 * Assign the max frame size for capped VBR in base layer frames
 * Update the qindex and active worse quality based on the already
 *  spent bits in the sliding window
 ******************************************************/
static int32_t svt_aom_crf_assign_max_rate(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs                 = ppcs->scs;
    EncodeContext*      enc_ctx             = scs->enc_ctx;
    RATE_CONTROL*       rc                  = &enc_ctx->rc;
    int                 frames_in_sw        = (int)rc->rate_average_periodin_frames;
    int64_t             spent_bits_sw       = 0, available_bit_sw;
    int                 coded_frames_num_sw = 0;
    // Find the start and the end of the sliding window
    int32_t start_index = ((ppcs->picture_number / frames_in_sw) * frames_in_sw) % CODED_FRAMES_STAT_QUEUE_MAX_DEPTH;
    int32_t end_index   = start_index + frames_in_sw;
    frames_in_sw        = (scs->passes > 1)
               ? MIN(end_index, (int32_t)scs->twopass.stats_buf_ctx->total_stats->count) - start_index
               : frames_in_sw;
    int64_t max_bits_sw = (int64_t)(scs->static_config.max_bit_rate * ((double)frames_in_sw / scs->frame_rate));
    max_bits_sw += (max_bits_sw * scs->static_config.mbr_over_shoot_pct / 100);

    // Loop over the sliding window and calculated the spent bits
    for (int index = start_index; index < end_index; index++) {
        int32_t                   queue_entry_index = (index > CODED_FRAMES_STAT_QUEUE_MAX_DEPTH - 1)
                              ? index - CODED_FRAMES_STAT_QUEUE_MAX_DEPTH
                              : index;
        coded_frames_stats_entry* queue_entry_ptr   = rc->coded_frames_stat_queue[queue_entry_index];
        spent_bits_sw += (queue_entry_ptr->frame_total_bit_actual > 0) ? queue_entry_ptr->frame_total_bit_actual : 0;
        coded_frames_num_sw += (queue_entry_ptr->frame_total_bit_actual > 0) ? 1 : 0;
    }
    available_bit_sw       = MAX(max_bits_sw - spent_bits_sw, 0);
    int64_t max_frame_size = 0;
    // Based on the kf boost, calculate the frame size for I frames
    if (ppcs->slice_type == I_SLICE) {
        int kf_interval = scs->static_config.intra_period_length > 0
            ? MIN(frames_in_sw, scs->static_config.intra_period_length + 1)
            : frames_in_sw;
        max_frame_size  = svt_av1_calculate_boost_bits(kf_interval, rc->kf_boost, available_bit_sw);
        int kf_low_thr  = BOOST_KF_LOW + (BOOST_KF_HIGH - BOOST_KF_LOW) / 3;
        if (rc->kf_boost > kf_low_thr) {
            max_frame_size = max_frame_size * 14 / 10;
        }
#if DEBUG_RC_CAP_LOG
        printf("SW_POC:%lld\t%lld\t%lld\t%d\tboost:%d\n",
               ppcs->picture_number,
               max_bits_sw,
               available_bit_sw,
               max_frame_size,
               rc->kf_boost);
#endif
    }
    // Based on the gfu boost, calculate the frame size for I frames
    else if (ppcs->temporal_layer_index == 0) {
        int64_t gf_group_bits = available_bit_sw * (int64_t)(1 << ppcs->hierarchical_levels) /
            (frames_in_sw - coded_frames_num_sw);
        max_frame_size  = svt_av1_calculate_boost_bits((1 << ppcs->hierarchical_levels), rc->gfu_boost, gf_group_bits);
        int gfu_low_thr = BOOST_GF_LOW_TPL_LA + (BOOST_GF_HIGH_TPL_LA - BOOST_GF_LOW_TPL_LA) / 3;
        if (rc->gfu_boost > gfu_low_thr) {
            max_frame_size = max_frame_size * 12 / 10;
        }
#if DEBUG_RC_CAP_LOG
        printf("SW_POC:%lld\t%lld\t%lld\t%d\tboost:%d\n",
               ppcs->picture_number,
               gf_group_bits,
               available_bit_sw,
               max_frame_size,
               rc->gfu_boost);
#endif
    }
    // Increase the qindex based on the status of the spent bits in the window
    int remaining_frames       = frames_in_sw - coded_frames_num_sw;
    int available_bit_ratio    = (int)(100 * available_bit_sw / max_bits_sw);
    int available_frames_ratio = 100 * remaining_frames / frames_in_sw;
    int buff_lvl_step          = (OPTIMAL_BUFFER_LEVEL - CRITICAL_BUFFER_LEVEL);
    int adjustment             = 0;
    if (available_bit_ratio <= OPTIMAL_BUFFER_LEVEL) {
        if (available_bit_ratio > CRITICAL_BUFFER_LEVEL) {
            int max_adjustment = (available_bit_ratio + 20 < available_frames_ratio) ? rc->active_worst_quality
                                                                                     : rc->active_worst_quality / 2;
            // Adjust up from assigned QP.
            if (available_bit_ratio < available_frames_ratio + 10) {
                adjustment = (int)(max_adjustment * (OPTIMAL_BUFFER_LEVEL - available_bit_ratio) / buff_lvl_step);
            }
        } else {
            // Set to worst_quality if buffer is below critical level.
            adjustment = rc->active_worst_quality;
        }
    }
#if DEBUG_RC_CAP_LOG
    printf("SW_POC:%lld\t%lld\t%lld\t%d%%\t%d%%\tadj:\t%d\n",
           ppcs->picture_number,
           max_bits_sw,
           available_bit_sw,
           available_bit_ratio,
           available_frames_ratio,
           adjustment);
#endif
    // Increase the active_worse_quality based on the adjustment
    if (ppcs->temporal_layer_index == 0) {
        rc->active_worst_quality += (adjustment / 2);
    }
    // Decrease the active_worse_quality where undershoot happens and active_worst_quality is greater than the input QP
    if (available_bit_ratio > available_frames_ratio + 20 && available_frames_ratio < 10 &&
        rc->active_worst_quality > quantizer_to_qindex[scs->static_config.qp]) {
        rc->active_worst_quality -= rc->active_worst_quality / 10;
    }
    rc->active_worst_quality = CLIP3(quantizer_to_qindex[scs->static_config.qp],
                                     quantizer_to_qindex[scs->static_config.max_qp_allowed],
                                     rc->active_worst_quality);

    // clip the max frame size to 32 bits
    ppcs->max_frame_size = (int)CLIP3(1, (0xFFFFFFFFull >> 1), (uint64_t)max_frame_size);
    // The target is set to 80% of the max.
    ppcs->this_frame_target = ppcs->max_frame_size * 8 / 10;

    return adjustment;
}

static int svt_av1_frame_type_qdelta(RATE_CONTROL* rc, rate_factor_level rf_lvl, int q, int bit_depth,
                                     int sc_content_detected) {
    FrameType frame_type  = (rf_lvl == KF_STD) ? KEY_FRAME : INTER_FRAME;
    double    rate_factor = svt_av1_rate_factor_deltas[rf_lvl];
    if (rf_lvl == GF_ARF_LOW) {
        rate_factor -= (0 - 2) * 0.1;
        rate_factor = AOMMAX(rate_factor, 1.0);
    }
    return svt_av1_compute_qdelta_by_rate(rc, frame_type, q, rate_factor, bit_depth, sc_content_detected);
}

static void adjust_active_best_and_worst_quality(PictureParentControlSet* ppcs, RATE_CONTROL* rc,
                                                 rate_factor_level rf_level, int* active_worst, int* active_best) {
    int                 active_best_quality  = *active_best;
    int                 active_worst_quality = *active_worst;
    SequenceControlSet* scs                  = ppcs->scs;
    int                 bit_depth            = scs->static_config.encoder_bit_depth;

    // Static forced key frames Q restrictions dealt with elsewhere.
    if (!frame_is_intra_only(ppcs)) {
        int qdelta = svt_av1_frame_type_qdelta(rc, rf_level, active_worst_quality, bit_depth, ppcs->sc_class1);
        active_worst_quality = AOMMAX(active_worst_quality + qdelta, active_best_quality);
    }

    active_best_quality  = clamp(active_best_quality, rc->best_quality, rc->worst_quality);
    active_worst_quality = clamp(active_worst_quality, active_best_quality, rc->worst_quality);

    *active_best  = active_best_quality;
    *active_worst = active_worst_quality;
}

/******************************************************
 * crf_qindex_calc
 * Assign the q_index per frame.
 * Used in the one pass encoding with tpl stats
 ******************************************************/
static int crf_qindex_calc(PictureControlSet* pcs, RATE_CONTROL* rc, int qindex) {
    PictureParentControlSet* ppcs                 = pcs->ppcs;
    SequenceControlSet*      scs                  = ppcs->scs;
    int                      cq_level             = qindex;
    int                      active_best_quality  = 0;
    int                      active_worst_quality = qindex;
    rc->arf_q                                     = 0;

    uint8_t temporal_layer      = ppcs->temporal_layer_index;
    uint8_t hierarchical_levels = ppcs->hierarchical_levels;
    int     leaf_frame          = ppcs->is_highest_layer;
    int     is_intrl_arf_boost  = (temporal_layer > 0 && !leaf_frame);

    rate_factor_level rf_level = (frame_is_intra_only(ppcs)) ? KF_STD
        : (temporal_layer == 0)                              ? GF_ARF_STD
        : !leaf_frame                                        ? GF_ARF_LOW
                                                             : INTER_NORMAL;

    int bit_depth = scs->static_config.encoder_bit_depth;

    // Set qindex calc method; r0-based using qstep or ref-frame based
    bool use_qstep_based_q_calc = ppcs->r0_qps;
    // Since many frames can be processed at the same time, storing/using arf_q in rc param is not sufficient and will create a run to run.
    // So, for each frame, arf_q is updated based on the qp of its references.
    rc->arf_q = MAX(rc->arf_q, pcs->ref_base_q_idx[REF_LIST_0][0]);
    if (pcs->slice_type == B_SLICE && ppcs->ref_list1_count_try) {
        rc->arf_q = MAX(rc->arf_q, pcs->ref_base_q_idx[REF_LIST_1][0]);
    }
#if DEBUG_QP_SCALING
    SVT_DEBUG("Frame %llu, temp. level %i, active worst quality %i, qstep based calc %i\n",
              pcs->picture_number,
              pcs->temporal_layer_index,
              active_worst_quality,
              use_qstep_based_q_calc);
    SVT_DEBUG("  ref1 q %i, ref2 q %i, arf q %i\n",
              pcs->ref_base_q_idx[REF_LIST_0][0],
              (pcs->slice_type == B_SLICE) ? pcs->ref_base_q_idx[REF_LIST_1][0] : 0,
              rc->arf_q);
#endif
    // r0 scaling
    // TPL may only look at a subset of available pictures in tpl group, which may affect the r0 calcuation.
    // As a result, we defined a factor to adjust r0 (to compensate for TPL not using all available frames).
    if (frame_is_intra_only(ppcs)) {
        if (ppcs->tpl_ctrls.r0_adjust_factor) {
            ppcs->r0 /= ppcs->tpl_ctrls.r0_adjust_factor;
        }
        // Scale r0 based on the GOP structure
        ppcs->r0 /= svt_av1_tpl_hl_islice_div_factor[hierarchical_levels];

        // when frames_to_key not available, i.e. in 1 pass encoding
        rc->kf_boost  = svt_av1_get_cqp_kf_boost_from_r0(ppcs->r0, -1, scs->input_resolution);
        int max_boost = ppcs->used_tpl_frame_num * KB;
        rc->kf_boost  = AOMMIN(rc->kf_boost, max_boost);

#if DEBUG_QP_SCALING
        SVT_DEBUG("  r0 %f, adj. factor %f, hier levels, %i, islice div factor %f, kf boost %i\n",
                  ppcs->r0,
                  ppcs->tpl_ctrls.r0_adjust_factor,
                  hierarchical_levels,
                  svt_av1_tpl_hl_islice_div_factor[hierarchical_levels],
                  rc->kf_boost);
#endif
    } else {
        if (use_qstep_based_q_calc) {
            if (ppcs->tpl_ctrls.r0_adjust_factor) {
                ppcs->r0 /= ppcs->tpl_ctrls.r0_adjust_factor;
                // Scale r0 based on the GOP structure
                ppcs->r0 /= svt_av1_tpl_hl_base_frame_div_factor[hierarchical_levels];
            }
        }
        int    num_stats_required_for_gfu_boost = ppcs->tpl_group_size + (1 << hierarchical_levels);
        double min_boost_factor                 = 1 << (hierarchical_levels >> 1);
        if (hierarchical_levels & 1) {
            min_boost_factor *= CONST_SQRT2;
        }
        rc->gfu_boost = svt_av1_get_gfu_boost_from_r0_lap(
            min_boost_factor, MAX_GFUBOOST_FACTOR, ppcs->r0, num_stats_required_for_gfu_boost);
#if DEBUG_QP_SCALING
        SVT_DEBUG("  r0 %f, adj. factor %f, hier levels %i, frame div factor %f, gfu boost %i\n",
                  ppcs->r0,
                  ppcs->tpl_ctrls.r0_adjust_factor,
                  hierarchical_levels,
                  svt_av1_tpl_hl_base_frame_div_factor[hierarchical_levels],
                  rc->gfu_boost);
#endif
    }

    if (use_qstep_based_q_calc) {
        unsigned int r0_weight_idx = !frame_is_intra_only(ppcs) + !!temporal_layer;
        assert(r0_weight_idx <= 2);
        double weight = svt_av1_r0_weight[r0_weight_idx];
        // adjust the weight for base layer frames with shorter minigops
        if (scs->lad_mg && !frame_is_intra_only(ppcs) &&
            (ppcs->tpl_group_size < (uint32_t)(2 << hierarchical_levels))) {
            weight = MIN(weight + 0.1, 1);
        }

        double qstep_ratio = sqrt(ppcs->r0) * weight *
            svt_av1_qp_scale_compress_weight[scs->static_config.qp_scale_compress_strength];
        if (scs->static_config.qp_scale_compress_strength) {
            // clamp qstep_ratio so it doesn't get past the weight value
            qstep_ratio = MIN(weight, qstep_ratio);
        }

        int qindex_from_qstep_ratio = svt_av1_get_q_index_from_qstep_ratio(qindex, qstep_ratio, bit_depth);
#if DEBUG_QP_SCALING
        SVT_DEBUG("  qstep based calc: r0 weight %f, qstep ratio %f, qindex from qstep ratio %i\n",
                  weight,
                  qstep_ratio,
                  qindex_from_qstep_ratio);
#endif
        if (!frame_is_intra_only(ppcs)) {
            rc->arf_q = qindex_from_qstep_ratio;
        }
        active_best_quality  = clamp(qindex_from_qstep_ratio, rc->best_quality, qindex);
        active_worst_quality = (active_best_quality + (3 * active_worst_quality) + 2) / 4;
    } else {
        active_best_quality = cq_level;

        if (is_intrl_arf_boost && !frame_is_intra_only(ppcs) && !leaf_frame) {
            EbReferenceObject* ref_obj_l0 = get_ref_obj(pcs, REF_LIST_0, 0);
            EbReferenceObject* ref_obj_l1 = NULL;
            if (pcs->slice_type == B_SLICE && ppcs->ref_list1_count_try) {
                ref_obj_l1 = get_ref_obj(pcs, REF_LIST_1, 0);
            }

            uint8_t ref_tmp_layer = ref_obj_l0->tmp_layer_idx;
            if (pcs->slice_type == B_SLICE && ppcs->ref_list1_count_try) {
                ref_tmp_layer = MAX(ref_tmp_layer, ref_obj_l1->tmp_layer_idx);
            }
            active_best_quality    = rc->arf_q;
            int8_t tmp_layer_delta = (int8_t)temporal_layer - (int8_t)ref_tmp_layer;
            if (rf_level == GF_ARF_LOW) {
                int w1 = svt_av1_non_base_qindex_weight_ref[hierarchical_levels];
                int w2 = svt_av1_non_base_qindex_weight_wq[hierarchical_levels];

#if DEBUG_QP_SCALING
                SVT_DEBUG("  w1 %i, w2 %i, w1 ref intra pct %i\n", w1, w2, w1 + pcs->ref_intra_percentage);
#endif
                if (temporal_layer > 0 && hierarchical_levels == 5) {
                    w1 += pcs->ref_intra_percentage;
                }

                while (tmp_layer_delta--) {
                    active_best_quality = (w1 * active_best_quality + (w2 * cq_level) + ((w1 + w2) / 2)) / (w1 + w2);
                }
            }
#if DEBUG_QP_SCALING
            SVT_DEBUG("  ref based calc: ref tmp layer %i, delta %i\n", ref_tmp_layer, tmp_layer_delta);
#endif
        }
    }

#if DEBUG_QP_SCALING
    SVT_DEBUG(
        "  before tmp layer adj: abq %i, awq %i, arf_q %i\n", active_best_quality, active_worst_quality, rc->arf_q);
#endif
    if (temporal_layer) {
        active_best_quality = MAX(active_best_quality, rc->arf_q);
    }
#if DEBUG_QP_SCALING
    SVT_DEBUG("  after tmp layer adj: abq %i, awq %i\n", active_best_quality, active_worst_quality);
#endif
    adjust_active_best_and_worst_quality(ppcs, rc, rf_level, &active_worst_quality, &active_best_quality);
#if DEBUG_QP_SCALING
    SVT_DEBUG("  after adj: abq %i, awq %i\n", active_best_quality, active_worst_quality);
#endif
    ppcs->top_index    = active_worst_quality;
    ppcs->bottom_index = active_best_quality;
    assert(ppcs->top_index <= rc->worst_quality && ppcs->top_index >= rc->best_quality);
    assert(ppcs->bottom_index <= rc->worst_quality && ppcs->bottom_index >= rc->best_quality);
    return active_best_quality;
}

/******************************************************
 * non_base_boost
 * Compute a non-base frame boost.
 ******************************************************/
static int8_t non_base_boost(PictureControlSet* pcs) {
    int8_t             q_boost      = 0;
    EbReferenceObject* ref_obj_l0   = get_ref_obj(pcs, REF_LIST_0, 0);
    uint32_t           l0_was_intra = 0;
    if (ref_obj_l0->slice_type != I_SLICE) {
        for (uint32_t sb_index = 0; sb_index < pcs->sb_total_count; sb_index++) {
            l0_was_intra += ref_obj_l0->sb_intra[sb_index];
        }
    }
    if (l0_was_intra) {
        int8_t intra_percentage = (l0_was_intra * 100) / pcs->sb_total_count;
        q_boost                 = intra_percentage >> 2;
    }
    return q_boost;
}

/******************************************************
 * cqp_qindex_calc
 * Assign the q_index per frame.
 * Used in the one pass encoding with no look ahead
 ******************************************************/
static int cqp_qindex_calc(PictureControlSet* pcs, int qindex) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    SequenceControlSet*      scs  = ppcs->scs;
    if (scs->allintra) {
        return qindex;
    }
    if (scs->use_flat_ipp && pcs->slice_type != I_SLICE) {
        return qindex;
    }
    int q;
    int bit_depth = scs->static_config.encoder_bit_depth;

#if TUNE_CQP_CHROMA_SSIM
    int active_worst_quality = qindex;
    if (pcs->temporal_layer_index == 0) {
        double qratio_grad = ppcs->hierarchical_levels <= 4 ? 0.3 : 0.2;
        double qstep_ratio = (0.2 + (1.0 - (double)active_worst_quality / MAXQ) * qratio_grad) *
            qp_scale_compress_weight[scs->static_config.qp_scale_compress_strength];
        q = scs->cqp_base_q = svt_av1_get_q_index_from_qstep_ratio(active_worst_quality, qstep_ratio, bit_depth);
    } else if (ppcs->is_ref && pcs->temporal_layer_index < ppcs->hierarchical_levels) {
        int this_height = ppcs->temporal_layer_index + 1;
        int arf_q       = scs->cqp_base_q;
        while (this_height > 1) {
            arf_q = (arf_q + active_worst_quality + 1) / 2;
            --this_height;
        }
        q = arf_q;
    } else {
        q = active_worst_quality;
    }
#else
    double q_val = svt_av1_convert_qindex_to_q(qindex, bit_depth);

    int offset_idx = -1;
    if (!ppcs->is_ref) {
        offset_idx = -1;
    } else if (ppcs->idr_flag) {
        offset_idx = 0;
    } else {
        offset_idx = MIN(pcs->temporal_layer_index + 1, FIXED_QP_OFFSET_COUNT - 1);
    }

    double q_val_target = (offset_idx == -1)
        ? q_val
        : MAX(q_val - (q_val * percents[ppcs->hierarchical_levels <= 4][offset_idx] / 100), 0.0);

    if (scs->static_config.pred_structure == LOW_DELAY) {
        if (ppcs->temporal_layer_index) {
            int8_t boost = non_base_boost(pcs);
            if (boost) {
                q_val_target = MAX(0, q_val_target - (boost * q_val_target) / 100);
            }
        }
    }
    q = qindex + svt_av1_compute_qdelta(q_val, q_val_target, bit_depth);
#endif

    return q;
}

/******************************************************
 * svt_av1_rc_calc_qindex_crf_cqp
 * Calculates qindex for CRF/CQP (AOM_Q) mode:
 * - QP scaling (CRF vs CQP)
 * - Fixed qindex offsets
 * - Extended CRF range
 * - Luminance QP bias
 * - S-frame QP offset
 * - Chroma qindex calculation
 * - Max rate assignment and segmentation
 ******************************************************/
void svt_av1_rc_calc_qindex_crf_cqp(PictureControlSet* pcs, SequenceControlSet* scs) {
    RATE_CONTROL*            rc       = &scs->enc_ctx->rc;
    PictureParentControlSet* ppcs     = pcs->ppcs;
    QuantizationParams*      q_params = &ppcs->frm_hdr.quantization_params;

    uint8_t scs_qp = ppcs->is_startup_gop ? clamp_qp(scs, scs->static_config.qp + scs->static_config.startup_qp_offset)
                                          : (uint8_t)scs->static_config.qp;
    int     scs_qindex = clamp_qindex(scs, quantizer_to_qindex[scs_qp] + scs->static_config.extended_crf_qindex_offset);

    // if RC mode is 0, fixed QP is used
    // QP scaling based on POC number for Flat IPPP structure
    if (ppcs->seq_param_changed) {
        rc->active_worst_quality = scs_qindex;
    }

    int32_t new_qindex = scs_qindex;
    if (ppcs->qp_on_the_fly) {
        new_qindex = quantizer_to_qindex[ppcs->picture_qp];
    } else {
        if (scs->enable_qp_scaling_flag) {
            // if CRF
            if (ppcs->tpl_ctrls.enable) {
                if (pcs->picture_number == 0) {
                    rc->active_worst_quality = scs_qindex;
                    svt_av1_rc_init(scs);
                }
                new_qindex = crf_qindex_calc(pcs, rc, rc->active_worst_quality);
            } else { // if CQP
                new_qindex = cqp_qindex_calc(pcs, scs_qindex);
            }
            new_qindex = clamp_qindex(scs, new_qindex);
        }

        if (scs->static_config.use_fixed_qindex_offsets) {
            if (scs->static_config.use_fixed_qindex_offsets == 1) {
                new_qindex = scs_qindex;
            }
            if (!frame_is_intra_only(ppcs)) {
                new_qindex += scs->static_config.qindex_offsets[pcs->temporal_layer_index];
            } else {
                new_qindex += scs->static_config.key_frame_qindex_offset;
            }
            new_qindex = clamp_qindex(scs, new_qindex);
        }

        // Extended CRF range (63.25 - 70), add offset to compress QP scaling
        if (scs->static_config.qp == MAX_QP_VALUE && scs->static_config.extended_crf_qindex_offset) {
            new_qindex += (MAXQ - new_qindex) * scs->static_config.extended_crf_qindex_offset / 56;
            new_qindex = clamp_qindex(scs, new_qindex);
        }

        // Luminance QP bias: gives more bitrate to darker scenes
        if (scs->static_config.luminance_qp_bias) {
            new_qindex += (int32_t)rint(
                -pow((255 - ppcs->avg_luma) /
                         (1024.0 / (pcs->temporal_layer_index * 4 * (0.01 * scs->static_config.luminance_qp_bias))),
                     0.5) *
                (new_qindex / 8.0));
            new_qindex = clamp_qindex(scs, new_qindex);
        }

        // S-frame QP offset
        if (ppcs->sframe_qp_offset) {
            int new_qp = clamp_qp(scs, ((new_qindex + 2) >> 2) + ppcs->sframe_qp_offset);
            new_qindex = clamp_qindex(scs, quantizer_to_qindex[new_qp]);
        }

        if (scs->enable_qp_scaling_flag) {
            // max bit rate is only active for 1 pass CRF
            if (scs->static_config.max_bit_rate) {
                new_qindex += svt_aom_crf_assign_max_rate(ppcs);
                new_qindex = clamp_qindex(scs, new_qindex);
            }
        }
    }

    // Calculate chroma qindex
    int32_t chroma_qindex = new_qindex;
    if (frame_is_intra_only(ppcs)) {
        chroma_qindex += scs->static_config.key_frame_chroma_qindex_offset;
    } else {
        chroma_qindex += scs->static_config.chroma_qindex_offsets[pcs->temporal_layer_index];
    }

    if (scs->static_config.tune == TUNE_IQ) {
        // Constant chroma boost with gradual ramp-down for very high qindex levels
        chroma_qindex -= CLIP3(0, 16, new_qindex / 2 - 14);
    }
    chroma_qindex = clamp_qindex(scs, chroma_qindex);

    // Calculate chroma delta q for Cb and Cr
    q_params->delta_q_dc[1] = q_params->delta_q_ac[1] = CLIP3(-64, 63, chroma_qindex - new_qindex);
    q_params->delta_q_dc[2] = q_params->delta_q_ac[2] = CLIP3(-64, 63, chroma_qindex - new_qindex);

    q_params->base_q_idx = new_qindex;
}

/**************************************************************************************************************
 * capped_crf_reencode()
 * This function performs re-encoding for capped CRF. It adjusts the QP, and active_worst_quality
 **************************************************************************************************************/
void capped_crf_reencode(PictureParentControlSet* ppcs, int* const q) {
    SequenceControlSet* scs          = ppcs->scs;
    EncodeContext*      enc_ctx      = scs->enc_ctx;
    RATE_CONTROL*       rc           = &enc_ctx->rc;
    int                 frames_in_sw = (int)rc->rate_average_periodin_frames;

    int64_t spent_bits_sw       = 0, available_bit_sw;
    int     coded_frames_num_sw = 0;
    // Find the start and the end of the sliding window
    int32_t start_index = ((ppcs->picture_number / frames_in_sw) * frames_in_sw) % CODED_FRAMES_STAT_QUEUE_MAX_DEPTH;
    int32_t end_index   = start_index + frames_in_sw;
    frames_in_sw        = (scs->passes > 1)
               ? MIN(end_index, (int32_t)scs->twopass.stats_buf_ctx->total_stats->count) - start_index
               : frames_in_sw;
    int64_t max_bits_sw = (int64_t)(scs->static_config.max_bit_rate * (double)frames_in_sw / scs->frame_rate);
    max_bits_sw += max_bits_sw * scs->static_config.mbr_over_shoot_pct / 100;
    // Loop over the sliding window and calculated the spent bits
    for (int index = start_index; index < end_index; index++) {
        int32_t                   queue_entry_index = (index > CODED_FRAMES_STAT_QUEUE_MAX_DEPTH - 1)
                              ? index - CODED_FRAMES_STAT_QUEUE_MAX_DEPTH
                              : index;
        coded_frames_stats_entry* queue_entry_ptr   = rc->coded_frames_stat_queue[queue_entry_index];
        spent_bits_sw += (queue_entry_ptr->frame_total_bit_actual > 0) ? queue_entry_ptr->frame_total_bit_actual : 0;
        coded_frames_num_sw += (queue_entry_ptr->frame_total_bit_actual > 0) ? 1 : 0;
    }
    available_bit_sw = MAX(max_bits_sw - spent_bits_sw, 0);

    int remaining_frames       = frames_in_sw - coded_frames_num_sw;
    int available_bit_ratio    = (int)(100 * available_bit_sw / max_bits_sw);
    int available_frames_ratio = 100 * remaining_frames / frames_in_sw;

    int worst_quality = quantizer_to_qindex[scs->static_config.max_qp_allowed];
    if (*q < worst_quality && ppcs->projected_frame_size > ppcs->max_frame_size && ppcs->temporal_layer_index == 0) {
        int     ref_qindex  = rc->active_worst_quality;
        double  ref_q       = svt_av1_convert_qindex_to_q(ref_qindex, scs->encoder_bit_depth);
        int64_t ref_bits    = (int64_t)(ppcs->projected_frame_size);
        int64_t target_bits = ppcs->max_frame_size;
        int     low         = rc->best_quality;
        int     high        = rc->worst_quality;

        while (low < high) {
            int    mid      = (low + high) >> 1;
            double q_tmp1   = svt_av1_convert_qindex_to_q(mid, scs->encoder_bit_depth);
            int    mid_bits = (int)(ref_bits * ref_q / q_tmp1);

            if (mid_bits > target_bits) {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        int tmp_q = low;

        rc->active_worst_quality = clamp_qindex(scs, tmp_q);
#if DEBUG_RC_CAP_LOG
        if (ppcs->temporal_layer_index <= 0) {
            SVT_DEBUG("Reencode POC:%lld\tQindex:%d\t%d\t%d\tWorseActive%d\t%d\t%d\n",
                      ppcs->picture_number,
                      ppcs->frm_hdr.quantization_params.base_q_idx,
                      ppcs->projected_frame_size,
                      ppcs->max_frame_size,
                      rc->active_worst_quality,
                      ppcs->bottom_index,
                      ppcs->top_index);
        }
#endif
        ppcs->top_index = rc->active_worst_quality;
        ppcs->q_high    = rc->active_worst_quality;
    }
    // Decrease the active worse quality based on the projected frame size and max frame size
    else if (ppcs->projected_frame_size < ppcs->max_frame_size && ppcs->temporal_layer_index == 0 &&
             ppcs->loop_count == 0 && rc->active_worst_quality > quantizer_to_qindex[scs->static_config.qp] &&
             (available_bit_ratio > available_frames_ratio)) {
        if (ppcs->projected_frame_size < ppcs->max_frame_size / 3) {
            rc->active_worst_quality -= rc->active_worst_quality / 5;
        } else if (ppcs->projected_frame_size < ppcs->max_frame_size / 2) {
            rc->active_worst_quality -= rc->active_worst_quality / 8;
        } else if (ppcs->projected_frame_size < 2 * ppcs->max_frame_size / 3) {
            rc->active_worst_quality -= rc->active_worst_quality / 12;
        }

        rc->active_worst_quality = CLIP3(quantizer_to_qindex[scs->static_config.qp],
                                         quantizer_to_qindex[scs->static_config.max_qp_allowed],
                                         rc->active_worst_quality);
    }
}

/************************************************************************************************
* Calculates the stat of coded frames over the averaging period
*************************************************************************************************/
void svt_av1_coded_frames_stat_calc(PictureParentControlSet* ppcs) {
    bool                move_slide_window_flag = true;
    SequenceControlSet* scs                    = ppcs->scs;
    EncodeContext*      enc_ctx                = scs->enc_ctx;
    RATE_CONTROL*       rc                     = &enc_ctx->rc;
    // Determine offset from the Head Ptr
    int32_t queue_entry_index =
        (int32_t)(ppcs->picture_number -
                  rc->coded_frames_stat_queue[rc->coded_frames_stat_queue_head_index]->picture_number);
    queue_entry_index += rc->coded_frames_stat_queue_head_index;
    queue_entry_index = (queue_entry_index > CODED_FRAMES_STAT_QUEUE_MAX_DEPTH - 1)
        ? queue_entry_index - CODED_FRAMES_STAT_QUEUE_MAX_DEPTH
        : queue_entry_index;

    coded_frames_stats_entry* queue_entry_ptr = rc->coded_frames_stat_queue[queue_entry_index];
    queue_entry_ptr->frame_total_bit_actual   = ppcs->total_num_bits;
    queue_entry_ptr->picture_number           = ppcs->picture_number;
    queue_entry_ptr->end_of_sequence_flag     = ppcs->end_of_sequence_flag;

    move_slide_window_flag = true;
    while (move_slide_window_flag) {
        {
            bool end_of_sequence_flag = true;
            // Check if the sliding window condition is valid
            uint32_t queue_entry_index_temp = rc->coded_frames_stat_queue_head_index;
            if (rc->coded_frames_stat_queue[queue_entry_index_temp]->frame_total_bit_actual != -1) {
                end_of_sequence_flag = rc->coded_frames_stat_queue[queue_entry_index_temp]->end_of_sequence_flag;
            } else {
                end_of_sequence_flag = false;
            }
            while (move_slide_window_flag && !end_of_sequence_flag &&
                   queue_entry_index_temp < rc->coded_frames_stat_queue_head_index + rc->rate_average_periodin_frames) {
                uint32_t queue_entry_index_temp2 = (queue_entry_index_temp > CODED_FRAMES_STAT_QUEUE_MAX_DEPTH - 1)
                    ? queue_entry_index_temp - CODED_FRAMES_STAT_QUEUE_MAX_DEPTH
                    : queue_entry_index_temp;

                move_slide_window_flag = move_slide_window_flag &&
                    (rc->coded_frames_stat_queue[queue_entry_index_temp2]->frame_total_bit_actual != -1);

                if (rc->coded_frames_stat_queue[queue_entry_index_temp2]->frame_total_bit_actual != -1) {
                    // check if it is the last frame. If we have reached the last frame, we would output the buffered frames in the Queue.
                    end_of_sequence_flag = rc->coded_frames_stat_queue[queue_entry_index_temp2]->end_of_sequence_flag;
                } else {
                    end_of_sequence_flag = false;
                }
                queue_entry_index_temp++;
            }
        }
        if (move_slide_window_flag) {
            //get a new entry spot
            queue_entry_ptr = rc->coded_frames_stat_queue[rc->coded_frames_stat_queue_head_index];
#if DEBUG_RC_CAP_LOG
            uint32_t queue_entry_index_temp = rc->coded_frames_stat_queue_head_index;
            // This is set to false, so the last frame would go inside the loop
            bool     end_of_sequence_flag    = false;
            uint32_t frames_in_sw            = 0;
            uint64_t total_bit_actual_per_sw = 0;

            while (!end_of_sequence_flag &&
                   queue_entry_index_temp < rc->coded_frames_stat_queue_head_index + rc->rate_average_periodin_frames) {
                frames_in_sw++;

                uint32_t queue_entry_index_temp2 = (queue_entry_index_temp > CODED_FRAMES_STAT_QUEUE_MAX_DEPTH - 1)
                    ? queue_entry_index_temp - CODED_FRAMES_STAT_QUEUE_MAX_DEPTH
                    : queue_entry_index_temp;

                total_bit_actual_per_sw += rc->coded_frames_stat_queue[queue_entry_index_temp2]->frame_total_bit_actual;
                end_of_sequence_flag = rc->coded_frames_stat_queue[queue_entry_index_temp2]->end_of_sequence_flag;

                queue_entry_index_temp++;
            }
            assert(frames_in_sw > 0);
            if (frames_in_sw == (uint32_t)rc->rate_average_periodin_frames) {
                if (queue_entry_ptr->picture_number % rc->rate_average_periodin_frames == 0) {
                    uint64_t avg_bit_rate_kbps = (uint64_t)(((double)total_bit_actual_per_sw * scs->frame_rate) /
                                                            ((double)frames_in_sw * 1000.0));
                    rc->max_bit_actual_per_gop = MAX(rc->max_bit_actual_per_gop, avg_bit_rate_kbps);
                    rc->min_bit_actual_per_gop = MIN(rc->min_bit_actual_per_gop, avg_bit_rate_kbps);
                    SVT_LOG("POC:%d\t%.0f\t%.2f%% \n",
                            (int)queue_entry_ptr->picture_number,
                            (double)avg_bit_rate_kbps,
                            100.0 *
                                    ((double)total_bit_actual_per_sw * scs->frame_rate /
                                     ((double)frames_in_sw * MAX((double)scs->static_config.max_bit_rate, 1.0))) -
                                100.0);

                    SVT_LOG("\n%d GopMax\t", (int32_t)rc->max_bit_actual_per_gop);
                    SVT_LOG("%d GopMin\n", (int32_t)rc->min_bit_actual_per_gop);
                }
            }
#endif
            // Reset the Queue Entry
            queue_entry_ptr->picture_number += CODED_FRAMES_STAT_QUEUE_MAX_DEPTH;
            queue_entry_ptr->frame_total_bit_actual = -1;

            // Increment the Queue head Ptr
            rc->coded_frames_stat_queue_head_index = (rc->coded_frames_stat_queue_head_index ==
                                                      CODED_FRAMES_STAT_QUEUE_MAX_DEPTH - 1)
                ? 0
                : rc->coded_frames_stat_queue_head_index + 1;

            queue_entry_ptr = (rc->coded_frames_stat_queue[rc->coded_frames_stat_queue_head_index]);
        }
    }
}
