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

#include "definitions.h"
#include "enc_handle.h"
#include "rc_process.h"
#include "sequence_control_set.h"
#include "pcs.h"
#include "utility.h"

#include "rc_results.h"
#include "rc_tasks.h"

#include "svt_log.h"
#include "rd_cost.h"
#include "lambda_rate_tables.h"
#include "pass2_strategy.h"
#include "segmentation.h"

#include "pd_results.h"
#include "src_ops_process.h"
#include "enc_mode_config.h"

// Specifies the weights of the ref frame in calculating qindex of non base layer frames
const int svt_av1_non_base_qindex_weight_ref[EB_MAX_TEMPORAL_LAYERS] = {100, 100, 100, 100, 100, 100};
// Specifies the weights of the worst quality in calculating qindex of non base layer frames
const int svt_av1_non_base_qindex_weight_wq[EB_MAX_TEMPORAL_LAYERS] = {100, 100, 300, 100, 100, 100};

const double svt_av1_tpl_hl_islice_div_factor[EB_MAX_TEMPORAL_LAYERS]     = {1, 2, 2, 1, 1, 0.7};
const double svt_av1_tpl_hl_base_frame_div_factor[EB_MAX_TEMPORAL_LAYERS] = {1, 3, 3, 2, 1, 1};

const double svt_av1_r0_weight[3]                = {0.75 /* I_SLICE */, 0.9 /* BASE */, 1 /* NON-BASE */};
const double svt_av1_qp_scale_compress_weight[4] = {1, 1.125, 1.25, 1.375};

static uint8_t NOINLINE clamp_qp(SequenceControlSet* scs, int qp) {
    int qmin = scs->static_config.min_qp_allowed;
    int qmax = scs->static_config.max_qp_allowed;
    return (uint8_t)CLIP3(qmin, qmax, qp);
}

int svt_aom_frame_is_kf_gf_arf(PictureParentControlSet* ppcs) {
    return frame_is_intra_only(ppcs) || ppcs->update_type == SVT_AV1_ARF_UPDATE ||
        ppcs->update_type == SVT_AV1_GF_UPDATE;
}

static EbReferenceObject* get_ref_obj(PictureControlSet* pcs, RefList ref_list, int idx) {
    return pcs->ref_pic_ptr_array[ref_list][idx]->object_ptr;
}

// intra_perc will be set to the % of intra area in two nearest ref frames
static void get_ref_intra_percentage(PictureControlSet* pcs, uint8_t* intra_perc) {
    assert(intra_perc != NULL);
    if (pcs->slice_type == I_SLICE) {
        *intra_perc = 100;
        return;
    }

    uint8_t            iperc      = 0;
    uint8_t            ref_cnt    = 0;
    EbReferenceObject* ref_obj_l0 = get_ref_obj(pcs, REF_LIST_0, 0);
    if (ref_obj_l0->slice_type != I_SLICE) {
        iperc = ref_obj_l0->intra_coded_area;
        ref_cnt++;
    }
    if (pcs->slice_type == B_SLICE && pcs->ppcs->ref_list1_count_try) {
        EbReferenceObject* ref_obj_l1 = get_ref_obj(pcs, REF_LIST_1, 0);
        if (ref_obj_l1->slice_type != I_SLICE) {
            iperc += ref_obj_l1->intra_coded_area;
            ref_cnt++;
        }
    }

    if (ref_cnt) {
        *intra_perc = iperc / ref_cnt;
    } else {
        *intra_perc = 0;
    }
}

// skip_area will be set to the % of skipped area in two nearest ref frames
static void get_ref_skip_percentage(PictureControlSet* pcs, uint8_t* skip_area) {
    assert(skip_area != NULL);
    if (pcs->slice_type == I_SLICE) {
        *skip_area = 0;
        return;
    }

    EbReferenceObject* ref_obj_l0 = get_ref_obj(pcs, REF_LIST_0, 0);
    uint8_t            skip_perc  = ref_obj_l0->skip_coded_area;
    if (pcs->slice_type == B_SLICE && pcs->ppcs->ref_list1_count_try) {
        EbReferenceObject* ref_obj_l1 = get_ref_obj(pcs, REF_LIST_1, 0);
        skip_perc += ref_obj_l1->skip_coded_area;

        // if have two frames, divide the skip_perc by 2 to get the avg skip area
        skip_perc >>= 1;
    }
    *skip_area = skip_perc;
}

// hp_area will be set to the % of hp area in two nearest ref frames
static void get_ref_hp_percentage(PictureControlSet* pcs, int16_t* hp_area) {
    assert(hp_area != NULL);
    if (pcs->slice_type == I_SLICE) {
        *hp_area = -1;
        return;
    }

    EbReferenceObject* ref_obj_l0 = get_ref_obj(pcs, REF_LIST_0, 0);
    int8_t             hp_perc_l0 = ref_obj_l0->slice_type == I_SLICE ? -1 : ref_obj_l0->hp_coded_area;

    int8_t hp_perc_l1 = -1;
    if (pcs->slice_type == B_SLICE && pcs->ppcs->ref_list1_count_try) {
        EbReferenceObject* ref_obj_l1 = get_ref_obj(pcs, REF_LIST_1, 0);
        hp_perc_l1                    = ref_obj_l1->slice_type == I_SLICE ? -1 : ref_obj_l1->hp_coded_area;
    }
    if (hp_perc_l0 == -1 && hp_perc_l1 == -1) {
        *hp_area = -1;
    } else if (hp_perc_l1 == -1) {
        *hp_area = hp_perc_l0;
    } else if (hp_perc_l0 == -1) {
        *hp_area = hp_perc_l1;
    } else {
        *hp_area = (hp_perc_l0 + hp_perc_l1) >> 1;
    }
}

static void free_private_data_list(EbBufferHeaderType* p) {
    EbPrivDataNode* p_node = (EbPrivDataNode*)p->p_app_private;
    while (p_node) {
        if (p_node->node_type != PRIVATE_DATA && p_node->node_type != ROI_MAP_EVENT) {
            EB_FREE(p_node->data);
        }
        EbPrivDataNode* p_tmp = p_node;
        p_node                = p_node->next;
        EB_FREE(p_tmp);
    }
    p->p_app_private = NULL;
}

typedef struct RateControlContext {
    EbFifo* rate_control_input_tasks_fifo_ptr;
    EbFifo* rate_control_output_results_fifo_ptr;
    EbFifo* picture_decision_results_output_fifo_ptr;
} RateControlContext;

EbErrorType svt_aom_rate_control_coded_frames_stats_context_ctor(coded_frames_stats_entry* entry_ptr,
                                                                 uint64_t                  picture_number) {
    entry_ptr->picture_number         = picture_number;
    entry_ptr->frame_total_bit_actual = -1;

    return EB_ErrorNone;
}

static void rate_control_context_dctor(EbPtr p) {
    EbThreadContext*    thread_ctx = (EbThreadContext*)p;
    RateControlContext* obj        = (RateControlContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

EbErrorType svt_aom_rate_control_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                              int me_port_index) {
    RateControlContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = rate_control_context_dctor;

    context_ptr->rate_control_input_tasks_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->rate_control_tasks_resource_ptr, 0);
    context_ptr->rate_control_output_results_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->rate_control_results_resource_ptr, 0);
    context_ptr->picture_decision_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_decision_results_resource_ptr, me_port_index);

    return EB_ErrorNone;
}

double svt_av1_convert_qindex_to_q(int32_t qindex, EbBitDepth bit_depth) {
    // Convert the index to a real Q value (scaled down to match old Q values)
    switch (bit_depth) {
    case EB_EIGHT_BIT:
        return svt_aom_ac_quant_qtx(qindex, 0, bit_depth) / 4.0;
    case EB_TEN_BIT:
        return svt_aom_ac_quant_qtx(qindex, 0, bit_depth) / 16.0;
    case EB_TWELVE_BIT:
        return svt_aom_ac_quant_qtx(qindex, 0, bit_depth) / 64.0;
    default:
        assert(0 && "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT or EB_TWELVE_BIT");
        return -1.0;
    }
}

int32_t svt_av1_compute_qdelta(double qstart, double qtarget, EbBitDepth bit_depth) {
    int32_t start_index  = MAXQ;
    int32_t target_index = MAXQ;
    int32_t i;

    // Convert the average q value to an index.
    for (i = MINQ; i < MAXQ; ++i) {
        start_index = i;
        if (svt_av1_convert_qindex_to_q(i, bit_depth) >= qstart) {
            break;
        }
    }

    // Convert the q target to an index
    for (i = MINQ; i < MAXQ; ++i) {
        target_index = i;
        if (svt_av1_convert_qindex_to_q(i, bit_depth) >= qtarget) {
            break;
        }
    }

    return target_index - start_index;
}

int svt_av1_get_cqp_kf_boost_from_r0(double r0, int frames_to_key, EbInputResolution input_resolution) {
    double factor;
    // when frames_to_key not available, it is set to -1. In this case the factor is set to average of min and max
    if (frames_to_key == -1) {
        factor = (10.0 + 4.0) / 2;
    } else {
        factor = sqrt((double)frames_to_key);
        factor = AOMMIN(factor, 10.0);
        factor = AOMMAX(factor, 4.0);
    }
    // calculate boost based on resolution
    return input_resolution <= INPUT_SIZE_720p_RANGE ? (int)rint(3 * (75.0 + 17.0 * factor) / r0)
                                                     : (int)rint(4 * (75.0 + 17.0 * factor) / r0);
}

int svt_av1_get_gfu_boost_from_r0_lap(double min_factor, double max_factor, double r0, int frames_to_key) {
    double factor = sqrt((double)frames_to_key);
    factor        = AOMMIN(factor, max_factor);
    factor        = AOMMAX(factor, min_factor);
    factor        = 200.0 + 10.0 * factor;
    return (int)rint(factor / r0);
}

int svt_av1_rc_bits_per_mb(FrameType frame_type, int qindex, double correction_factor, int bit_depth,
                           int is_screen_content_type) {
    double q = svt_av1_convert_qindex_to_q(qindex, bit_depth);
    int    enumerator;
    if (is_screen_content_type) {
        enumerator = (frame_type == KEY_FRAME) ? 1000000 : 750000;
    } else {
        enumerator = (frame_type == KEY_FRAME) ? 1400000 : 1000000;
    }
    assert(correction_factor <= MAX_BPB_FACTOR && correction_factor >= MIN_BPB_FACTOR);

    // q based adjustment to baseline enumerator
    return (int)(enumerator * correction_factor / q);
}

static int find_qindex_by_rate(int desired_bits_per_mb, int bit_depth, FrameType frame_type, int is_screen_content_type,
                               int best_qindex, int worst_qindex) {
    assert(best_qindex <= worst_qindex);
    int low  = best_qindex;
    int high = worst_qindex;
    while (low < high) {
        int mid             = (low + high) >> 1;
        int mid_bits_per_mb = svt_av1_rc_bits_per_mb(frame_type, mid, 1.0, bit_depth, is_screen_content_type);
        if (mid_bits_per_mb > desired_bits_per_mb) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    assert(low == high);
    assert(svt_av1_rc_bits_per_mb(frame_type, low, 1.0, bit_depth, is_screen_content_type) <= desired_bits_per_mb ||
           low == worst_qindex);
    return low;
}

int svt_av1_compute_qdelta_by_rate(RATE_CONTROL* rc, FrameType frame_type, int qindex, double rate_target_ratio,
                                   int bit_depth, int is_screen_content_type) {
    // Look up the current projected bits per block for the base index
    int base_bits_per_mb = svt_av1_rc_bits_per_mb(frame_type, qindex, 1.0, bit_depth, is_screen_content_type);

    // Find the target bits per mb based on the base value and given ratio.
    int target_bits_per_mb = (int)(rate_target_ratio * base_bits_per_mb);

    int target_index = find_qindex_by_rate(
        target_bits_per_mb, bit_depth, frame_type, is_screen_content_type, rc->best_quality, rc->worst_quality);
    return target_index - qindex;
}

const double svt_av1_rate_factor_deltas[RATE_FACTOR_LEVELS] = {
    1.00, // INTER_NORMAL
    1.00, // INTER_LOW
    1.00, // INTER_HIGH
    1.50, // GF_ARF_LOW
    2.00, // GF_ARF_STD
    2.00, // KF_STD
};

const rate_factor_level svt_av1_rate_factor_levels[SVT_AV1_FRAME_UPDATE_TYPES] = {
    KF_STD, // KF_UPDATE
    INTER_NORMAL, // LF_UPDATE
    GF_ARF_STD, // GF_UPDATE
    GF_ARF_STD, // ARF_UPDATE
    INTER_NORMAL, // OVERLAY_UPDATE
    INTER_NORMAL, // INTNL_OVERLAY_UPDATE
    GF_ARF_LOW, // INTNL_ARF_UPDATE
};

int svt_av1_get_q_index_from_qstep_ratio(int leaf_qindex, double qstep_ratio, int bit_depth) {
    double leaf_qstep   = svt_aom_dc_quant_qtx(leaf_qindex, 0, bit_depth);
    double target_qstep = leaf_qstep * qstep_ratio;
    int    qindex;
    if (qstep_ratio < 1.0) {
        for (qindex = leaf_qindex; qindex > MINQ; --qindex) {
            double qstep = svt_aom_dc_quant_qtx(qindex, 0, bit_depth);
            if (qstep <= target_qstep) {
                break;
            }
        }
    } else {
        for (qindex = leaf_qindex; qindex <= MAXQ; ++qindex) {
            double qstep = svt_aom_dc_quant_qtx(qindex, 0, bit_depth);
            if (qstep >= target_qstep) {
                break;
            }
        }
    }
    return qindex;
}

// Returns the default rd multiplier for inter frames for a given qindex.
// The function here is a first pass estimate based on data from
// a previous Vizer run
static double def_inter_rd_multiplier(int qindex) {
    return 3.2 + 0.0015 * qindex;
}

// Returns the default rd multiplier for ARF/Golden Frames for a given qindex.
// The function here is a first pass estimate based on data from
// a previous Vizer run
static double def_arf_rd_multiplier(int qindex) {
    return 3.25 + 0.0015 * qindex;
}

// Returns the default rd multiplier for key frames for a given qindex.
// The function here is a first pass estimate based on data from
// a previous Vizer run
static double def_kf_rd_multiplier(int qindex) {
    return 3.3 + 0.0015 * qindex;
}

int svt_aom_compute_rd_mult_based_on_qindex(EbBitDepth bit_depth, SvtAv1FrameUpdateType update_type, int qindex) {
    int     q = svt_aom_dc_quant_qtx(qindex, 0, bit_depth);
    int64_t rdmult;

    // Scale rdmult based on frame type
    if (update_type == SVT_AV1_KF_UPDATE) {
        rdmult = (int64_t)(def_kf_rd_multiplier(q) * q * q);
    } else if (update_type == SVT_AV1_GF_UPDATE || update_type == SVT_AV1_ARF_UPDATE) {
        rdmult = (int64_t)(def_arf_rd_multiplier(q) * q * q);
    } else {
        rdmult = (int64_t)(def_inter_rd_multiplier(q) * q * q);
    }

    switch (bit_depth) {
    case EB_EIGHT_BIT:
        break;
    case EB_TEN_BIT:
        rdmult = ROUND_POWER_OF_TWO(rdmult, 4);
        break;
    case EB_TWELVE_BIT:
        rdmult = ROUND_POWER_OF_TWO(rdmult, 8);
        break;
    default:
        assert(0 && "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT or EB_TWELVE_BIT");
        return -1;
    }

    return rdmult > 0 ? (int)AOMMIN(rdmult, INT_MAX) : 1;
}

static const int rd_frame_type_factor[2][SVT_AV1_FRAME_UPDATE_TYPES] = {{150, 180, 150, 150, 180, 180, 150},
                                                                        {128, 144, 128, 128, 144, 144, 128}};
#define RTC_KF_LAMBDA_BOOST 100

static uint32_t update_lambda(PictureControlSet* pcs, uint8_t q_index, uint8_t me_q_index, EbBitDepth bit_depth,
                              int64_t rdmult) {
    PictureParentControlSet* ppcs       = pcs->ppcs;
    FrameType                frame_type = ppcs->frm_hdr.frame_type;
    // To set gf_update_type based on current TL vs. the max TL (e.g. for 5L, max TL is 4)
    uint8_t temporal_layer_index = pcs->scs->use_flat_ipp ? 0 : ppcs->temporal_layer_index;
    uint8_t max_temporal_layer   = pcs->scs->use_flat_ipp ? 0 : ppcs->hierarchical_levels;

    // Update rdmult based on the frame's position in the miniGOP
    uint8_t gf_update_type = frame_type == KEY_FRAME ? SVT_AV1_KF_UPDATE
        : temporal_layer_index == 0                  ? SVT_AV1_ARF_UPDATE
        : temporal_layer_index < max_temporal_layer  ? SVT_AV1_INTNL_ARF_UPDATE
                                                     : SVT_AV1_LF_UPDATE;
    rdmult                 = (rdmult * rd_frame_type_factor[bit_depth != EB_EIGHT_BIT][gf_update_type]) >> 7;
    if (pcs->scs->static_config.rtc && frame_type == KEY_FRAME) {
        rdmult = (rdmult * RTC_KF_LAMBDA_BOOST) >> 7;
    }
    if (pcs->scs->stats_based_sb_lambda_modulation) {
        int factor = 128;
        if (pcs->scs->static_config.rtc) {
            int qdiff = me_q_index - ppcs->frm_hdr.quantization_params.base_q_idx;
            if (qdiff < 0) {
                factor = (qdiff <= -4) ? 100 : 115;
            }
        } else if (ppcs->frm_hdr.delta_q_params.delta_q_present || ppcs->r0_delta_qp_md) {
            int qdiff = q_index - ppcs->frm_hdr.quantization_params.base_q_idx;
            if (qdiff < 0) {
                factor = (qdiff <= -8) ? 90 : 115;
            } else if (qdiff > 0) {
                factor = (qdiff <= 8) ? 135 : 150;
            }
        } else {
            int qdiff = me_q_index - ppcs->frm_hdr.quantization_params.base_q_idx;
            if (qdiff < 0) {
                factor = (qdiff <= -4) ? 100 : 115;
            } else if (qdiff > 0) {
                factor = (qdiff <= 4) ? 135 : 150;
            }
        }

        rdmult = (rdmult * factor) >> 7;
    }
    return (uint32_t)rdmult;
}

/*
 * Set the sse lambda based on the bit_depth, then update based on frame position.
 */
uint32_t svt_aom_compute_rd_mult(PictureControlSet* pcs, uint8_t q_index, uint8_t me_q_index, EbBitDepth bit_depth) {
    // Always use q_index for the derivation of the initial rdmult (i.e. don't use me_q_index)
    int64_t rdmult = svt_aom_compute_rd_mult_based_on_qindex(bit_depth, pcs->ppcs->update_type, q_index);

    return update_lambda(pcs, q_index, me_q_index, bit_depth, rdmult);
}

uint32_t svt_aom_compute_fast_lambda(PictureControlSet* pcs, uint8_t q_index, uint8_t me_q_index,
                                     EbBitDepth bit_depth) {
    // Always use q_index for the derivation of the initial rdmult (i.e. don't use me_q_index)
    int64_t rdmult = bit_depth == EB_EIGHT_BIT ? av1_lambda_mode_decision8_bit_sad[q_index]
                                               : av1lambda_mode_decision10_bit_sad[q_index];

    return update_lambda(pcs, q_index, me_q_index, bit_depth, rdmult);
}

void svt_aom_lambda_assign(PictureControlSet* pcs, uint32_t* fast_lambda, uint32_t* full_lambda, EbBitDepth bit_depth,
                           uint8_t qp_index, bool multiply_lambda) {
    if (bit_depth == EB_EIGHT_BIT) {
        *full_lambda = svt_aom_compute_rd_mult(pcs, qp_index, qp_index, bit_depth);
        *fast_lambda = av1_lambda_mode_decision8_bit_sad[qp_index];
    } else if (bit_depth == EB_TEN_BIT) {
        *full_lambda = svt_aom_compute_rd_mult(pcs, qp_index, qp_index, bit_depth);
        *fast_lambda = av1lambda_mode_decision10_bit_sad[qp_index];
        if (multiply_lambda) {
            *full_lambda *= 16;
            *fast_lambda *= 4;
        }
    } else if (bit_depth == EB_TWELVE_BIT) {
        *full_lambda = svt_aom_compute_rd_mult(pcs, qp_index, qp_index, bit_depth);
        *fast_lambda = av1lambda_mode_decision12_bit_sad[qp_index];
    } else {
        assert(0);
    }

    // NM: To be done: tune lambda based on the picture type and layer.
    uint64_t scale_factor = pcs->scs->static_config.lambda_scale_factors[pcs->ppcs->update_type];
    *full_lambda          = (uint32_t)((*full_lambda * scale_factor) >> 7);
    *fast_lambda          = (uint32_t)((*fast_lambda * scale_factor) >> 7);
}

/******************************************************************************
* svt_av1_compute_deltaq
* Compute delta-q based on the q, bitdepth and cyclic refresh parameters
*******************************************************************************/
int svt_av1_compute_deltaq(PictureParentControlSet* ppcs, int q, double rate_ratio_qdelta) {
    SequenceControlSet* scs       = ppcs->scs;
    RATE_CONTROL*       rc        = &scs->enc_ctx->rc;
    int                 bit_depth = scs->static_config.encoder_bit_depth;

    rate_factor_level rf_lvl     = svt_av1_rate_factor_levels[ppcs->update_type];
    FrameType         frame_type = (rf_lvl == KF_STD) ? KEY_FRAME : INTER_FRAME;

    int deltaq = svt_av1_compute_qdelta_by_rate(rc, frame_type, q, rate_ratio_qdelta, bit_depth, ppcs->sc_class1);
    deltaq     = AOMMAX(deltaq, -ppcs->cyclic_refresh.max_qdelta_perc * q / 100);

    if (scs->enc_ctx->rc_cfg.mode != AOM_CBR) {
        // RA uses a scale factor of 4 for the deltaQ range. Found it beneficial for low delay to have a larger deltaQ range, so we scale by 8
        deltaq = AOMMIN(deltaq, 9 * 8 - 1);
        deltaq = AOMMAX(deltaq, -9 * 8 + 1);
    }
    return deltaq;
}

void svt_av1_rc_init(SequenceControlSet* scs) {
    EncodeContext*  enc_ctx = scs->enc_ctx;
    RATE_CONTROL*   rc      = &enc_ctx->rc;
    RateControlCfg* rc_cfg  = &enc_ctx->rc_cfg;
    int             i;
    if (rc_cfg->mode == AOM_CBR) {
        rc->avg_frame_qindex[KEY_FRAME]   = rc_cfg->worst_allowed_q;
        rc->avg_frame_qindex[INTER_FRAME] = rc_cfg->worst_allowed_q;
        rc->last_q[KEY_FRAME]             = rc_cfg->worst_allowed_q;
        rc->last_q[INTER_FRAME]           = rc_cfg->worst_allowed_q;
    } else {
        rc->avg_frame_qindex[KEY_FRAME]   = (rc_cfg->worst_allowed_q + rc_cfg->best_allowed_q) / 2;
        rc->avg_frame_qindex[INTER_FRAME] = (rc_cfg->worst_allowed_q + rc_cfg->best_allowed_q) / 2;
        rc->last_q[KEY_FRAME]             = (rc_cfg->worst_allowed_q + rc_cfg->best_allowed_q) / 2;
        rc->last_q[INTER_FRAME]           = (rc_cfg->worst_allowed_q + rc_cfg->best_allowed_q) / 2;
    }
    rc->buffer_level    = rc->starting_buffer_level;
    rc->bits_off_target = rc->starting_buffer_level;

    rc->rolling_target_bits = rc->avg_frame_bandwidth;
    rc->rolling_actual_bits = rc->avg_frame_bandwidth;
    rc->total_actual_bits   = 0;
    rc->total_target_bits   = 0;

    rc->frames_since_key      = 8; // Sensible default for first frame.
    rc->this_key_frame_forced = 0;
    for (i = 0; i < MAX_TEMPORAL_LAYERS + 1; ++i) {
        rc->rate_correction_factors[i] = 0.7;
    }
    if (rc_cfg->mode != AOM_CBR) {
        rc->rate_correction_factors[KF_STD] = 1.0;
    }
    rc->baseline_gf_interval = 1 << scs->static_config.hierarchical_levels;

    // Set absolute upper and lower quality limits
    rc->worst_quality = rc_cfg->worst_allowed_q;
    rc->best_quality  = rc_cfg->best_allowed_q;
    if (rc_cfg->mode != AOM_Q) {
        double frame_rate = (double)scs->static_config.frame_rate_numerator /
            (double)scs->static_config.frame_rate_denominator;
        // Each frame can have a different duration, as the frame rate in the source
        // isn't guaranteed to be constant. The frame rate prior to the first frame
        // encoded in the second pass is a guess. However, the sum duration is not.
        // It is calculated based on the actual durations of all frames from the
        // first pass.
        svt_av1_new_framerate(scs, frame_rate);
    }
    // current and previous average base layer ME distortion
    rc->cur_avg_base_me_dist  = 0;
    rc->prev_avg_base_me_dist = 0;
    rc->avg_frame_low_motion  = 0;
}

/*********************************************************************************************
* Reset rate_control_param into default values
***********************************************************************************************/
static void rc_param_reset(RateControlIntervalParamContext* rc_param) {
    rc_param->size                     = -1;
    rc_param->processed_frame_number   = 0;
    rc_param->vbr_bits_off_target      = 0;
    rc_param->vbr_bits_off_target_fast = 0;
    rc_param->rate_error_estimate      = 0;
    rc_param->total_actual_bits        = 0;
    rc_param->total_target_bits        = 0;
    rc_param->extend_minq              = 0;
    rc_param->extend_maxq              = 0;
    rc_param->extend_minq_fast         = 0;
}

void svt_aom_update_rc_counts(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs     = ppcs->scs;
    EncodeContext*      enc_ctx = scs->enc_ctx;
    RATE_CONTROL*       rc      = &enc_ctx->rc;
    if (ppcs->frm_hdr.showable_frame) {
        // If this is a show_existing_frame with a source other than altref,
        // or if it is not a displayed forward keyframe, the keyframe update
        // counters were incremented when it was originally encoded.
        rc->frames_since_key++;
        rc->frames_to_key--;
    }
}

/****************************************************************************************
* reset_rc_param
* reset RC related variable in PPCS
*****************************************************************************************/
void reset_rc_param(PictureParentControlSet* ppcs) {
    ppcs->loop_count      = 0;
    ppcs->overshoot_seen  = 0;
    ppcs->undershoot_seen = 0;
}

/******************************************************
 * rc_init_frame_stats
 * Initializes frame statistics for rate control:
 * - Generates r0/beta values
 * - Initializes cyclic refresh
 * - Calculates reference frame statistics
 * - Calculates ME distortion
 * - Sets rate averaging period
 ******************************************************/
static void rc_init_frame_stats(PictureControlSet* pcs, SequenceControlSet* scs) {
    RATE_CONTROL*            rc   = &scs->enc_ctx->rc;
    PictureParentControlSet* ppcs = pcs->ppcs;
    // Get r0
    if (ppcs->r0_gen) {
        svt_aom_generate_r0beta(ppcs);
    }

    if (scs->static_config.aq_mode && scs->super_block_size == 64 && scs->enc_ctx->rc_cfg.mode == AOM_CBR) {
        svt_aom_cyclic_refresh_init(ppcs);
    }

    // Get reference frame statistics
    get_ref_intra_percentage(pcs, &pcs->ref_intra_percentage);
    get_ref_skip_percentage(pcs, &pcs->ref_skip_percentage);
    get_ref_hp_percentage(pcs, &pcs->ref_hp_percentage);

    // Set rate averaging period
    if (scs->passes > 1 && scs->static_config.max_bit_rate) {
        rc->rate_average_periodin_frames = (uint64_t)scs->twopass.stats_buf_ctx->total_stats->count;
    } else {
        rc->rate_average_periodin_frames = 60;
    }
    rc->rate_average_periodin_frames = MIN(rc->rate_average_periodin_frames, MAX_RATE_AVG_PERIOD);

    // Store the avg ME distortion
    if (ppcs->slice_type != I_SLICE) {
        rc->prev_avg_base_me_dist = rc->cur_avg_base_me_dist;
        uint64_t avg_me_dist      = 0;
        for (int b64_idx = 0; b64_idx < ppcs->b64_total_count; ++b64_idx) {
            avg_me_dist += ppcs->me_64x64_distortion[b64_idx];
        }
        avg_me_dist /= ppcs->b64_total_count;
        rc->cur_avg_base_me_dist = (uint32_t)avg_me_dist;
    }
}

// Calculate the number of bits to assign to boosted frames in a group.
int svt_av1_calculate_boost_bits(int frame_count, int boost, int64_t total_group_bits) {
    int allocation_chunks;

    // return 0 for invalid inputs (could arise e.g. through rounding errors)
    if (!boost || (total_group_bits <= 0)) {
        return 0;
    }

    if (frame_count <= 0) {
        return (int)(AOMMIN(total_group_bits, INT_MAX));
    }

    allocation_chunks = (frame_count * 100) + boost;

    // Prevent overflow.
    if (boost > 1023) {
        int divisor = boost >> 10;
        boost /= divisor;
        allocation_chunks /= divisor;
    }

    // Calculate the number of extra bits for use in the boosted frame or frames.
    return AOMMAX((int)(((int64_t)boost * total_group_bits) / allocation_chunks), 0);
}

/******************************************************
 * rc_handle_superres
 * Handles superres processing for 1-pass encoding:
 * - Determines superres parameters
 * - Re-initializes ME segments if needed
 * - Releases PA reference objects
 * Returns true if superres triggered (early exit needed)
 ******************************************************/
static bool rc_handle_superres(PictureControlSet* pcs, RateControlContext* context_ptr,
                               EbObjectWrapper* rate_control_tasks_wrapper_ptr) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    SequenceControlSet*      scs  = pcs->scs;

    if (scs->static_config.pass != ENC_SINGLE_PASS) {
        return false;
    }

    if (scs->static_config.superres_mode <= SUPERRES_RANDOM) {
        return false;
    }

    // Determine denom and scale down picture by selected denom
    svt_aom_init_resize_picture(scs, ppcs);
    if (ppcs->frame_superres_enabled || ppcs->frame_resize_enabled) {
        // Reset gm based on super-res on/off
        bool super_res_off = ppcs->frame_superres_enabled == false && scs->static_config.resize_mode == RESIZE_NONE;
        svt_aom_set_gm_controls(ppcs, svt_aom_derive_gm_level(ppcs, super_res_off));

        // Initialize Segments as picture decision process
        ppcs->me_segments_completion_count = 0;
        ppcs->me_processed_b64_count       = 0;

        for (uint32_t segment_index = 0; segment_index < ppcs->me_segments_total_count; ++segment_index) {
            // Get Empty Results Object
            EbObjectWrapper* out_results_wrapper;
            svt_get_empty_object(context_ptr->picture_decision_results_output_fifo_ptr, &out_results_wrapper);

            PictureDecisionResults* out_results = (PictureDecisionResults*)out_results_wrapper->object_ptr;
            out_results->pcs_wrapper            = ppcs->p_pcs_wrapper_ptr;
            out_results->segment_index          = segment_index;
            out_results->task_type              = TASK_SUPERRES_RE_ME;
            // Post the Full Results Object
            svt_post_full_object(out_results_wrapper);
        }

        // Release Rate Control Tasks
        svt_release_object(rate_control_tasks_wrapper_ptr);
        return true; // Signal early exit
    }

    // PA ref objs are no longer needed if super-res isn't performed on current frame
    if (ppcs->tpl_ctrls.enable) {
        if (ppcs->temporal_layer_index == 0) {
            for (uint32_t i = 0; i < ppcs->tpl_group_size; i++) {
                if (svt_aom_is_incomp_mg_frame(ppcs->tpl_group[i])) {
                    if (ppcs->tpl_group[i]->ext_mg_id == ppcs->ext_mg_id + 1) {
                        svt_aom_release_pa_reference_objects(scs, ppcs->tpl_group[i]);
                    }
                } else {
                    if (ppcs->tpl_group[i]->ext_mg_id == ppcs->ext_mg_id) {
                        svt_aom_release_pa_reference_objects(scs, ppcs->tpl_group[i]);
                    }
                }
            }
        }
    } else {
        svt_aom_release_pa_reference_objects(scs, ppcs);
    }

    return false;
}

// Process packetization feedback: update RC parameters and release resources.
static void rc_process_packetization_feedback(PictureParentControlSet* ppcs,
                                              const EbObjectWrapper* restrict rate_control_tasks_wrapper_ptr) {
    SequenceControlSet* scs      = ppcs->scs;
    RateControlTasks*   rc_tasks = (RateControlTasks*)rate_control_tasks_wrapper_ptr->object_ptr;

    // Prevent double counting frames with overlay
    if (!ppcs->is_overlay) {
        svt_block_on_mutex(scs->enc_ctx->rc_param_queue_mutex);
        ppcs->rate_control_param_ptr->processed_frame_number++;

        // check if all the frames in the interval have arrived
        if (ppcs->rate_control_param_ptr->size == ppcs->rate_control_param_ptr->processed_frame_number) {
            rc_param_reset(ppcs->rate_control_param_ptr);
        }
        svt_release_mutex(scs->enc_ctx->rc_param_queue_mutex);
    }

    if (scs->enc_ctx->rc_cfg.mode == AOM_Q) {
        // Queue variables
        if (scs->static_config.max_bit_rate) {
            svt_av1_coded_frames_stat_calc(ppcs);
        }
    } else {
        if (scs->static_config.gop_constraint_rc) {
            svt_av1_rc_postencode_update_gop_const(ppcs);
            // Qindex calculating
            if (scs->enc_ctx->rc_cfg.mode == AOM_VBR) {
                svt_av1_twopass_postencode_update_gop_const(ppcs);
            }
        } else {
            svt_av1_rc_postencode_update(ppcs);
            // Qindex calculating
            if (scs->enc_ctx->rc_cfg.mode == AOM_VBR) {
                svt_av1_twopass_postencode_update(ppcs);
            }
        }
        svt_aom_update_rc_counts(ppcs);
    }

    // Release the ParentPictureControlSet
    if (ppcs->y8b_wrapper) {
        // y8b needs to get decremented at the same time of regular input
        svt_release_object(ppcs->y8b_wrapper);
    }

    // free private data list before release input picture buffer
    free_private_data_list((EbBufferHeaderType*)ppcs->input_pic_wrapper->object_ptr);

    svt_release_object(ppcs->input_pic_wrapper);
    svt_release_object(ppcs->scs_wrapper);
    svt_release_object(rc_tasks->pcs_wrapper);
}

void* svt_aom_rate_control_kernel(void* input_ptr) {
    // Context
    EbThreadContext*         thread_ctx  = (EbThreadContext*)input_ptr;
    RateControlContext*      context_ptr = (RateControlContext*)thread_ctx->priv;
    SequenceControlSet*      scs;
    PictureControlSet*       pcs;
    PictureParentControlSet* ppcs;

    for (;;) {
        // Get RateControl Task
        EbObjectWrapper* rate_control_tasks_wrapper_ptr;
        EB_GET_FULL_OBJECT(context_ptr->rate_control_input_tasks_fifo_ptr, &rate_control_tasks_wrapper_ptr);

        RateControlTasks*    rc_tasks                = (RateControlTasks*)rate_control_tasks_wrapper_ptr->object_ptr;
        RateControlTaskTypes task_type               = rc_tasks->task_type;
        bool                 is_superres_recode_task = (task_type == RC_INPUT_SUPERRES_RECODE) ? true : false;

        // Modify these for different temporal layers later
        switch (task_type) {
        case RC_INPUT_SUPERRES_RECODE:
            assert(scs->static_config.superres_mode == SUPERRES_QTHRESH ||
                   scs->static_config.superres_mode == SUPERRES_AUTO);
            // intentionally reuse code in RC_INPUT
        case RC_INPUT:
            pcs  = (PictureControlSet*)rc_tasks->pcs_wrapper->object_ptr;
            ppcs = pcs->ppcs;
            scs  = pcs->scs;

            rc_init_frame_stats(pcs, scs);

            if (!is_superres_recode_task) {
                ppcs->blk_lambda_tuning = false;
            }
            reset_rc_param(ppcs);

            if (ppcs->is_overlay) {
                // overlay: ppcs->picture_qp has been updated by altref RC_INPUT
            } else {
                if (scs->enc_ctx->rc_cfg.mode == AOM_Q) {
                    svt_av1_rc_calc_qindex_crf_cqp(pcs, scs);
                    svt_aom_setup_segmentation(pcs, scs);
                } else {
                    if (!is_superres_recode_task) {
                        svt_av1_rc_process_rate_allocation(pcs, scs);
                    }
                    svt_av1_rc_calc_qindex_rate_control(pcs, scs);
                }
                ppcs->picture_qp = clamp_qp(scs, (ppcs->frm_hdr.quantization_params.base_q_idx + 2) >> 2);
            }

            if (ppcs->is_alt_ref) {
                // overlay use the same QP with alt_ref, to align with
                // rate_control_param_queue update code in below RC_PACKETIZATION_FEEDBACK_RESULT.
                PictureParentControlSet* overlay_ppcs     = ppcs->overlay_ppcs_ptr;
                overlay_ppcs->picture_qp                  = ppcs->picture_qp;
                overlay_ppcs->frm_hdr.quantization_params = ppcs->frm_hdr.quantization_params;
            }

            if (!is_superres_recode_task) {
                if (rc_handle_superres(pcs, context_ptr, rate_control_tasks_wrapper_ptr)) {
                    break;
                }
            }

            svt_av1_rc_init_sb_qindex(pcs, scs);

            // Get Empty Rate Control Results Buffer
            EbObjectWrapper* rc_results_wrapper;
            svt_get_empty_object(context_ptr->rate_control_output_results_fifo_ptr, &rc_results_wrapper);
            RateControlResults* rc_results = (RateControlResults*)rc_results_wrapper->object_ptr;
            rc_results->pcs_wrapper        = rc_tasks->pcs_wrapper;
            rc_results->superres_recode    = is_superres_recode_task;

            // Post Full Rate Control Results
            svt_post_full_object(rc_results_wrapper);

            // Release Rate Control Tasks
            svt_release_object(rate_control_tasks_wrapper_ptr);

            break;

        case RC_PACKETIZATION_FEEDBACK_RESULT:
            ppcs = (PictureParentControlSet*)rc_tasks->pcs_wrapper->object_ptr;
            scs  = ppcs->scs;

            rc_process_packetization_feedback(ppcs, rate_control_tasks_wrapper_ptr);

            // Release Rate Control Tasks
            svt_release_object(rate_control_tasks_wrapper_ptr);
            break;

        default:
            pcs = (PictureControlSet*)rc_tasks->pcs_wrapper->object_ptr;
            scs = pcs->scs;

            break;
        }
    }

    return NULL;
}
