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
#include "inv_transforms.h"
#include "me_context.h"
#include "utility.h"

#include "rc_process.h"
#include "resize.h"

// These functions use formulaic calculations to make playing with the
// quantizer tables easier. If necessary they can be replaced by lookup
// tables if and when things settle down in the experimental Bitstream
int32_t svt_av1_convert_qindex_to_q_fp8(int32_t qindex, EbBitDepth bit_depth) {
    // Convert the index to a real Q value (scaled down to match old Q values)
    switch (bit_depth) {
    case EB_EIGHT_BIT:
        return svt_aom_ac_quant_qtx(qindex, 0, bit_depth) << 6; // / 4.0;
    case EB_TEN_BIT:
        return svt_aom_ac_quant_qtx(qindex, 0, bit_depth) << 4; // / 16.0;
    case EB_TWELVE_BIT:
        return svt_aom_ac_quant_qtx(qindex, 0, bit_depth) << 3; // / 64.0;
    default:
        assert(0 && "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT or EB_TWELVE_BIT");
        return -1;
    }
}

int32_t svt_av1_compute_qdelta_fp(int32_t qstart_fp8, int32_t qtarget_fp8, EbBitDepth bit_depth) {
    int32_t start_index  = MAXQ;
    int32_t target_index = MAXQ;
    int32_t i;

    // Convert the average q value to an index.
    for (i = MINQ; i < MAXQ; ++i) {
        start_index = i;
        if (svt_av1_convert_qindex_to_q_fp8(i, bit_depth) >= qstart_fp8) {
            break;
        }
    }

    // Convert the q target to an index
    for (i = MINQ; i < MAXQ; ++i) {
        target_index = i;
        if (svt_av1_convert_qindex_to_q_fp8(i, bit_depth) >= qtarget_fp8) {
            break;
        }
    }

    return target_index - start_index;
}

int variance_comp_int(const void* a, const void* b) {
    return (int)*(uint16_t*)a - *(uint16_t*)b;
}

#define VAR_BOOST_MAX_DELTAQ_RANGE 80
#define VAR_BOOST_MAX_QSTEP_RATIO_BOOST 8

#define SUPERBLOCK_SIZE 64
#define SUBBLOCK_SIZE 8
#define SUBBLOCKS_IN_SB_DIM (SUPERBLOCK_SIZE / SUBBLOCK_SIZE)
#define SUBBLOCKS_IN_SB (SUBBLOCKS_IN_SB_DIM * SUBBLOCKS_IN_SB_DIM)
#define SUBBLOCKS_IN_OCTILE (SUBBLOCKS_IN_SB / 8)

static int av1_get_deltaq_sb_variance_boost(uint8_t base_q_idx, uint16_t* variances, uint8_t strength,
                                            EbBitDepth bit_depth, uint8_t octile, uint8_t curve) {
    // boost q_index based on empirical visual testing, strength 2
    // variance     qstep_ratio boost (@ base_q_idx 255)
    // 256          1
    // 64           1.481
    // 16           2.192
    // 4            3.246
    // 1            4.806

    // copy sb 8x8 variance values to an array for ordering
    uint16_t ordered_variances[64];
    memcpy(&ordered_variances, variances + ME_TIER_ZERO_PU_8x8_0, sizeof(uint16_t) * 64);
    qsort(&ordered_variances, 64, sizeof(uint16_t), variance_comp_int);

    // Sample three 8x8 variance values: at the specified octile, previous octile,
    // and next octile. Make sure we use the last subblock in each octile as the
    // representative of the octile.
    assert(octile >= 1 && octile <= 8);
    int mid_idx = octile * SUBBLOCKS_IN_OCTILE - 1;
    int low_idx = AOMMAX(SUBBLOCKS_IN_OCTILE - 1, mid_idx - SUBBLOCKS_IN_OCTILE);
    int upp_idx = AOMMIN(SUBBLOCKS_IN_SB - 1, mid_idx + SUBBLOCKS_IN_OCTILE);

    // Weigh the three variances in a 1:2:1 ratio, with rounding (the +2 term).
    // This allows for smoother delta-q transitions among superblocks with
    // mixed-variance features.
    int variance = (ordered_variances[low_idx] + ordered_variances[mid_idx] * 2 + ordered_variances[upp_idx] + 2) / 4;

#if DEBUG_VAR_BOOST
    SVT_INFO("64x64 variance: %d\n", variances[ME_TIER_ZERO_PU_64x64]);
    SVT_INFO("8x8 min %d, 1st oct %d, median %d, max %d\n",
             ordered_variances[0],
             ordered_variances[7],
             ordered_variances[31],
             ordered_variances[63]);
    SVT_INFO("8x8 variances\n");
    uint16_t* variances_row = variances + ME_TIER_ZERO_PU_8x8_0;

    for (int row = 0; row < 8; row++) {
        SVT_INFO("%5d %5d %5d %5d %5d %5d %5d %5d\n",
                 variances_row[0],
                 variances_row[1],
                 variances_row[2],
                 variances_row[3],
                 variances_row[4],
                 variances_row[5],
                 variances_row[6],
                 variances_row[7]);
        variances_row += 8;
    }
#endif

    // variance = 0 areas are either completely flat patches or very fine gradients
    // SVT-AV1 doesn't have enough resolution to tell them apart, so let's assume they're not flat and boost them
    if (variance == 0) {
        variance = 1;
    }

    // compute a boost based on a fast-growing formula
    // high and medium variance sbs essentially get no boost, while increasingly lower variance sbs get stronger boosts
    assert(strength >= 1 && strength <= 4);
    double              qstep_ratio = 0;
    static const double strengths[] = {0, 0.65, 1.1, 1.6, 2.5};

    switch (curve) {
    case 1: /* 1: low-medium contrast boosting curve */
        qstep_ratio = 0.25 * strength * (-log2((double)variance) + 8) + 1;
        break;
    case 2: /* 2: still picture curve, tuned for SSIMULACRA2 performance on CID22 */
        qstep_ratio = 0.15 * strength * (-log2((double)variance) + 10) + 1;
        break;
    default: /* 0: default q step ratio curve */
        qstep_ratio = pow(1.018, strengths[strength] * (-10 * log2((double)variance) + 80));
        break;
    }
    qstep_ratio = CLIP3(1, VAR_BOOST_MAX_QSTEP_RATIO_BOOST, qstep_ratio);

    int32_t base_q   = svt_av1_convert_qindex_to_q_fp8(base_q_idx, bit_depth);
    int32_t target_q = (int32_t)(base_q / qstep_ratio);
    int32_t boost    = 0;

    switch (curve) {
    case 2: /* still picture boost, tuned for SSIMULACRA2 performance on CID22 */
        boost = (int32_t)((base_q_idx + 544) * -svt_av1_compute_qdelta_fp(base_q, target_q, bit_depth) / (255 + 1024));
        break;
    default: /* curve 0 & 1 boost (default) */
        boost = (int32_t)((base_q_idx + 40) * -svt_av1_compute_qdelta_fp(base_q, target_q, bit_depth) / (255 + 40));
        break;
    }
    boost = AOMMIN(VAR_BOOST_MAX_DELTAQ_RANGE, boost);

#if DEBUG_VAR_BOOST
    SVT_INFO("Variance: %d, Strength: %d, Q-step ratio: %f, Boost: %d, Base q: %d, Target q: %d\n",
             variance,
             strength,
             qstep_ratio,
             boost,
             base_q,
             target_q);
#endif

    return boost;
}

void svt_av1_variance_adjust_qp(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    SequenceControlSet*      scs  = ppcs->scs;

    ppcs->frm_hdr.delta_q_params.delta_q_present = 1;

    // super res pictures scaled with different sb count, should use sb_total_count for each picture
    uint16_t sb_cnt = scs->sb_total_count;
    if (ppcs->frame_superres_enabled || ppcs->frame_resize_enabled) {
        sb_cnt = ppcs->b64_total_count;
    }

    uint8_t min_qindex = MAXQ;
    uint8_t max_qindex = MINQ;

#if DEBUG_VAR_BOOST_STATS
    SVT_DEBUG("TPL/CQP SB qindex, frame %llu, temp. level %i\n", pcs->picture_number, pcs->temporal_layer_index);

    for (uint32_t sb_addr = 0; sb_addr < sb_cnt; ++sb_addr) {
        SuperBlock* sb_ptr = pcs->sb_ptr_array[sb_addr];

        SVT_DEBUG("%4d ", sb_ptr->qindex);

        if (pcs->frame_width <= (sb_ptr->org_x + 64)) {
            SVT_DEBUG("\n");
        }
    }
    SVT_DEBUG("VAQ qindex boost, frame %llu, temp. level %i\n", pcs->picture_number, pcs->temporal_layer_index);
#endif
    for (uint32_t sb_addr = 0; sb_addr < sb_cnt; ++sb_addr) {
        SuperBlock* sb_ptr = pcs->sb_ptr_array[sb_addr];

        // adjust deltaq based on sb variance, with lower variance resulting in a lower qindex
        int boost = av1_get_deltaq_sb_variance_boost(ppcs->frm_hdr.quantization_params.base_q_idx,
                                                     ppcs->variance[sb_addr],
                                                     scs->static_config.variance_boost_strength,
                                                     scs->static_config.encoder_bit_depth,
                                                     scs->static_config.variance_octile,
                                                     scs->static_config.variance_boost_curve);
#if DEBUG_VAR_BOOST_STATS
        SVT_DEBUG("%4d ", boost);

        if (pcs->frame_width <= (sb_ptr->org_x + 64)) {
            SVT_DEBUG("\n");
        }
#endif
        // don't clamp qindex on valid deltaq range yet
        // we'll do it after adjusting frame qp to maximize deltaq frame range
        // q_index 0 is lossless, and is currently not supported in SVT-AV1
        sb_ptr->qindex = CLIP3(1, MAXQ, sb_ptr->qindex - boost);

        // record last seen min and max qindexes for frame qp readjusting
        min_qindex = AOMMIN(min_qindex, sb_ptr->qindex);
        max_qindex = AOMMAX(max_qindex, sb_ptr->qindex);
    }

    // normalize and clamp frame qindex value to maximize deltaq range
    int range                 = max_qindex - min_qindex;
    range                     = AOMMIN(range, VAR_BOOST_MAX_DELTAQ_RANGE);
    int normalized_base_q_idx = (int)min_qindex + (range >> 1);

#if DEBUG_VAR_BOOST_QP
    SVT_INFO("previous qidx %d, min_qidx %d, max_qidx %d, delta_q_res %d, normalized qidx %d, range %d\n",
             ppcs->frm_hdr.quantization_params.base_q_idx,
             min_qindex,
             max_qindex,
             ppcs->frm_hdr.delta_q_params.delta_q_res,
             normalized_base_q_idx,
             range);
#endif
#if DEBUG_VAR_BOOST_STATS
    SVT_DEBUG(
        "Total CQP/CRF + VAQ qindex, frame %llu, temp. level %i\n", pcs->picture_number, pcs->temporal_layer_index);
#endif

    // normalize sb qindex values
    for (uint32_t sb_addr = 0; sb_addr < sb_cnt; ++sb_addr) {
        SuperBlock* sb_ptr = pcs->sb_ptr_array[sb_addr];

        int offset = (int)sb_ptr->qindex - normalized_base_q_idx;
        offset     = AOMMIN(offset, VAR_BOOST_MAX_DELTAQ_RANGE >> 1);
        offset     = AOMMAX(offset, -VAR_BOOST_MAX_DELTAQ_RANGE >> 1);

        // q_index 0 is lossless, and is currently not supported in SVT-AV1
        uint8_t normalized_qindex = CLIP3(1, MAXQ, normalized_base_q_idx + offset);
#if DEBUG_VAR_BOOST_STATS
        SVT_DEBUG("%4d ", normalized_qindex);

        if (pcs->frame_width <= (sb_ptr->org_x + 64)) {
            SVT_DEBUG("\n");
        }
#endif

#if DEBUG_VAR_BOOST_QP
        SVT_INFO("  sb %d qindex: previous %d, normalized %d\n", sb_addr, sb_ptr->qindex, normalized_qindex);
#endif
        sb_ptr->qindex = normalized_qindex;
    }
}

/******************************************************
 * cyclic_sb_qp_derivation
 * Calculates the QP per SB based on the ME statistics
 * used in one pass encoding
 * only works for sb size  = 64
 ******************************************************/
static const int BOOST_MAX = 10;
// Maximum rate target ratio for setting segment delta-qp.
#define CR_MAX_RATE_TARGET_RATIO 4.0

static void cyclic_sb_qp_derivation(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    CyclicRefresh*           cr   = &ppcs->cyclic_refresh;

    ppcs->frm_hdr.delta_q_params.delta_q_present = 1;

    uint64_t avg_me_dist = ppcs->norm_me_dist;

    cr->actual_num_seg1_sbs = 0;
    cr->actual_num_seg2_sbs = 0;
    uint64_t seg2_dist      = 0;
    for (uint32_t b64_idx = 0; b64_idx < ppcs->b64_total_count; ++b64_idx) {
        int diff_dist = (int)(ppcs->me_8x8_distortion[b64_idx] - avg_me_dist);
        if (b64_idx >= cr->sb_start && b64_idx < cr->sb_end && diff_dist < 0) {
            seg2_dist += ppcs->me_8x8_distortion[b64_idx];
            cr->actual_num_seg2_sbs++;
        } else if (b64_idx >= cr->sb_start && b64_idx < cr->sb_end) {
            cr->actual_num_seg1_sbs++;
        }
    }
    if (!ppcs->sc_class1 && cr->actual_num_seg2_sbs) {
        seg2_dist    = seg2_dist / cr->actual_num_seg2_sbs;
        uint64_t dev = (avg_me_dist - seg2_dist) * 100 / avg_me_dist;
        // Quadratic Scaling; boost = BOOST_MAX * (dev/100)^2
        int boost = (int)(BOOST_MAX * dev * dev / (100 * 100)); // = /10000
        cr->rate_boost_fac += boost;
    }
    int delta1 = svt_av1_compute_deltaq(ppcs, ppcs->frm_hdr.quantization_params.base_q_idx, cr->rate_ratio_qdelta);
    int delta2 = svt_av1_compute_deltaq(
        ppcs,
        ppcs->frm_hdr.quantization_params.base_q_idx,
        AOMMIN(CR_MAX_RATE_TARGET_RATIO, 0.1 * cr->rate_boost_fac * cr->rate_ratio_qdelta));
    cr->qindex_delta[CR_SEGMENT_ID_BASE]   = 0;
    cr->qindex_delta[CR_SEGMENT_ID_BOOST1] = delta1;
    cr->qindex_delta[CR_SEGMENT_ID_BOOST2] = delta2;

    for (uint32_t b64_idx = 0; b64_idx < ppcs->b64_total_count; ++b64_idx) {
        int         diff_dist = (int)(ppcs->me_8x8_distortion[b64_idx] - avg_me_dist);
        SuperBlock* sb        = pcs->sb_ptr_array[b64_idx];
        int         offset    = 0;
        if (b64_idx >= cr->sb_start && b64_idx < cr->sb_end && diff_dist < 0) {
            offset = cr->qindex_delta[CR_SEGMENT_ID_BOOST2];

        } else if (b64_idx >= cr->sb_start && b64_idx < cr->sb_end) {
            offset = cr->qindex_delta[CR_SEGMENT_ID_BOOST1];
        }
        sb->qindex = CLIP3(1, MAXQ, ((int16_t)ppcs->frm_hdr.quantization_params.base_q_idx + (int16_t)offset));
    }
}

/*
* Derives a qindex per 64x64 using ME distortions (to be used for lambda modulation only; not at Q/Q-1)
*/
static void generate_b64_me_qindex_map(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs                            = pcs->ppcs;
    static const int         min_offset[MAX_TEMPORAL_LAYERS] = {-8, -8, -8, -8, -8, -8};
    static const int         max_offset[MAX_TEMPORAL_LAYERS] = {8, 8, 8, 8, 8, 8};
    if (pcs->slice_type != I_SLICE &&
        (min_offset[ppcs->temporal_layer_index] != 0 || max_offset[ppcs->temporal_layer_index] != 0)) {
        uint64_t avg_me_dist = 0;
        uint64_t min_dist    = (uint64_t)~0;
        uint64_t max_dist    = 0;

        for (uint32_t b64_idx = 0; b64_idx < ppcs->b64_total_count; ++b64_idx) {
            avg_me_dist += ppcs->me_8x8_cost_variance[b64_idx];
            min_dist = MIN(ppcs->me_8x8_cost_variance[b64_idx], min_dist);
            max_dist = MAX(ppcs->me_8x8_cost_variance[b64_idx], max_dist);
        }
        avg_me_dist /= ppcs->b64_total_count;

        for (uint32_t b64_idx = 0; b64_idx < ppcs->b64_total_count; ++b64_idx) {
            int diff_dist = (int)(ppcs->me_8x8_cost_variance[b64_idx] - avg_me_dist);
            int offset    = 0;
            if (diff_dist <= 0) {
                offset = (min_dist != avg_me_dist)
                    ? min_offset[ppcs->temporal_layer_index] * diff_dist / (int)(min_dist - avg_me_dist)
                    : 0;
            } else {
                offset = (max_dist != avg_me_dist)
                    ? max_offset[ppcs->temporal_layer_index] * diff_dist / (int)(max_dist - avg_me_dist)
                    : 0;
            }

            offset                      = AOMMIN(offset, 9 * 4 - 1);
            offset                      = AOMMAX(offset, -9 * 4 + 1);
            pcs->b64_me_qindex[b64_idx] = (uint8_t)CLIP3(
                1, MAXQ, ppcs->frm_hdr.quantization_params.base_q_idx + offset);
        }
    } else {
        for (uint32_t b64_idx = 0; b64_idx < ppcs->b64_total_count; ++b64_idx) {
            pcs->b64_me_qindex[b64_idx] = ppcs->frm_hdr.quantization_params.base_q_idx;
        }
    }
}

static int svt_av1_get_deltaq_offset(EbBitDepth bit_depth, int qindex, double beta, bool is_intra) {
    assert(beta > 0.0);
    int q = svt_aom_dc_quant_qtx(qindex, 0, bit_depth);
    int newq;
    // use a less aggressive action when lowering the q for non I_slice
    if (!is_intra && beta > 1) {
        newq = (int)rint(q / sqrt(sqrt(beta)));
    } else {
        newq = (int)rint(q / sqrt(beta));
    }
    int orig_qindex = qindex;
    if (newq == q) {
        return 0;
    }
    if (newq < q) {
        while (qindex > MINQ) {
            qindex--;
            q = svt_aom_dc_quant_qtx(qindex, 0, bit_depth);
            if (newq >= q) {
                break;
            }
        }
    } else {
        while (qindex < MAXQ) {
            qindex++;
            q = svt_aom_dc_quant_qtx(qindex, 0, bit_depth);
            if (newq <= q) {
                break;
            }
        }
    }
    return qindex - orig_qindex;
}

static void sb_setup_lambda(PictureControlSet* pcs, SuperBlock* sb_ptr) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    SequenceControlSet*      scs  = ppcs->scs;

    int mi_col = sb_ptr->org_x / 4;
    int mi_row = sb_ptr->org_y / 4;

    int mi_col_sr = coded_to_superres_mi(mi_col, ppcs->superres_denom);
    assert(ppcs->enhanced_unscaled_pic);
    // ALIGN_POWER_OF_TWO(pixels, 3) >> 2 ??
    int mi_cols_sr     = ((ppcs->enhanced_unscaled_pic->width + 15) / 16) << 2;
    int sb_mi_width_sr = coded_to_superres_mi(mi_size_wide[scs->seq_header.sb_size], ppcs->superres_denom);
    int bsize_base     = ppcs->tpl_ctrls.synth_blk_size == 32 ? BLOCK_32X32 : BLOCK_16X16;
    int num_mi_w       = mi_size_wide[bsize_base];
    int num_mi_h       = mi_size_high[bsize_base];
    int num_cols       = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    int num_rows       = (ppcs->av1_cm->mi_rows + num_mi_h - 1) / num_mi_h;
    int num_bcols      = (sb_mi_width_sr + num_mi_w - 1) / num_mi_w;
    int num_brows      = (mi_size_high[scs->seq_header.sb_size] + num_mi_h - 1) / num_mi_h;

    int row, col;

    int32_t base_block_count = 0;
    double  log_sum          = 0.0;

    for (row = mi_row / num_mi_w; row < num_rows && row < mi_row / num_mi_w + num_brows; ++row) {
        for (col = mi_col_sr / num_mi_h; col < num_cols && col < mi_col_sr / num_mi_h + num_bcols; ++col) {
            int index = row * num_cols + col;
            log_sum += log(ppcs->pa_me_data->tpl_rdmult_scaling_factors[index]);
            ++base_block_count;
        }
    }
    assert(base_block_count > 0);

    EbBitDepth bit_depth   = pcs->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT;
    double     orig_rdmult = svt_aom_compute_rd_mult(
        pcs, ppcs->frm_hdr.quantization_params.base_q_idx, ppcs->frm_hdr.quantization_params.base_q_idx, bit_depth);
    double new_rdmult = svt_aom_compute_rd_mult(
        pcs, sb_ptr->qindex, svt_aom_get_me_qindex(pcs, sb_ptr, scs->seq_header.sb_size == BLOCK_128X128), bit_depth);
    double scaling_factor = new_rdmult / orig_rdmult;
    //double scale_adj = exp(log(scaling_factor) - log_sum / base_block_count);
    double scale_adj = scaling_factor / exp(log_sum / base_block_count);

    for (row = mi_row / num_mi_w; row < num_rows && row < mi_row / num_mi_w + num_brows; ++row) {
        for (col = mi_col_sr / num_mi_h; col < num_cols && col < mi_col_sr / num_mi_h + num_bcols; ++col) {
            int index                                              = row * num_cols + col;
            ppcs->pa_me_data->tpl_sb_rdmult_scaling_factors[index] = scale_adj *
                ppcs->pa_me_data->tpl_rdmult_scaling_factors[index];
        }
    }
    ppcs->blk_lambda_tuning = true;
}

/******************************************************
 * svt_aom_sb_qp_derivation_tpl_la
 * Calculates the QP per SB based on the tpl statistics
 * used in one pass and second pass of two pass encoding
 ******************************************************/
void svt_aom_sb_qp_derivation_tpl_la(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs = pcs->ppcs;
    SequenceControlSet*      scs  = ppcs->scs;
    if (ppcs->r0_delta_qp_quant) {
        ppcs->frm_hdr.delta_q_params.delta_q_present = 1;
    }

    // super res pictures scaled with different sb count, should use sb_total_count for each picture
    uint16_t sb_cnt = scs->sb_total_count;
    if (ppcs->frame_superres_enabled || ppcs->frame_resize_enabled) {
        sb_cnt = pcs->sb_total_count;
    }
    if (ppcs->r0_delta_qp_md && ppcs->tpl_is_valid == 1) {
#if DEBUG_VAR_BOOST_STATS
        SVT_DEBUG("TPL qindex boost, frame %llu, temp. level %i\n", pcs->picture_number, pcs->temporal_layer_index);
#endif
        for (uint32_t sb_addr = 0; sb_addr < sb_cnt; ++sb_addr) {
            SuperBlock* sb_ptr = pcs->sb_ptr_array[sb_addr];
            double      beta   = ppcs->pa_me_data->tpl_beta[sb_addr];
            int         offset = svt_av1_get_deltaq_offset(
                scs->static_config.encoder_bit_depth, sb_ptr->qindex, beta, ppcs->slice_type == I_SLICE);
            offset = AOMMIN(offset, 9 * 4 - 1);
            offset = AOMMAX(offset, -9 * 4 + 1);

#if DEBUG_VAR_BOOST_STATS
            SVT_DEBUG("%4d ", -offset);
            if (pcs->frame_width <= (sb_ptr->org_x + 64)) {
                SVT_DEBUG("\n");
            }
#endif
            // read back SB qindex value, and add TPL boost on top
            // q_index 0 is lossless, and is currently not supported in SVT-AV1
            sb_ptr->qindex = CLIP3(1, MAXQ, (int16_t)sb_ptr->qindex + (int16_t)offset);

            sb_setup_lambda(pcs, sb_ptr);
        }
    }
}

/******************************************************
 * svt_av1_normalize_sb_delta_q
 * Adjusts superblock delta q to the most optimal res
 ******************************************************/
void svt_av1_normalize_sb_delta_q(PictureControlSet* pcs) {
    PictureParentControlSet* ppcs        = pcs->ppcs;
    SequenceControlSet*      scs         = ppcs->scs;
    uint8_t                  delta_q_res = ppcs->frm_hdr.delta_q_params.delta_q_res;

    assert(delta_q_res == 2 || delta_q_res == 4 || delta_q_res == 8);

    uint8_t mask              = ~(delta_q_res - 1);
    uint8_t delta_q_remainder = (ppcs->frm_hdr.quantization_params.base_q_idx) & ~mask;
    // Adjustment to push sb qindex toward the nearest multiple of delta_q_res, relative to base_q_idx
    int8_t delta_q_adjustment = (delta_q_res - delta_q_remainder) - (delta_q_res / 2);

    // super res pictures scaled with different sb count, should use sb_total_count for each picture
    uint16_t sb_cnt = scs->sb_total_count;
    if (ppcs->frame_superres_enabled || ppcs->frame_resize_enabled) {
        sb_cnt = ppcs->b64_total_count;
    }
#if DEBUG_VAR_BOOST_STATS
    SVT_LOG("Normalized delta q boost, frame %llu, temp. level %i, new delta_q_res %i\n",
            pcs->picture_number,
            pcs->temporal_layer_index,
            delta_q_res);
#endif
    for (uint32_t sb_addr = 0; sb_addr < sb_cnt; ++sb_addr) {
        SuperBlock* sb_ptr = pcs->sb_ptr_array[sb_addr];
        // Adjust sb_qindex to minimize the difference between its pre- and post-normalization value
        uint8_t adjusted_q_index   = CLIP3(1, MAXQ, sb_ptr->qindex + delta_q_adjustment);
        uint8_t normalized_q_index = (adjusted_q_index & mask) + delta_q_remainder;

        // q_index 0 is lossless, so do not use it when encoding in lossy mode
        sb_ptr->qindex = normalized_q_index == 0 ? delta_q_res : normalized_q_index;
#if DEBUG_VAR_BOOST_STATS
        SVT_LOG("%4d ", sb_ptr->qindex);
        if (pcs->frame_width <= (sb_ptr->org_x + 64)) {
            SVT_LOG("\n");
        }
#endif
    }
}

// Initialize SB qindex values and apply per-SB adjustments (variance boost, TPL, cyclic refresh).
void svt_av1_rc_init_sb_qindex(PictureControlSet* pcs, SequenceControlSet* scs) {
    PictureParentControlSet* ppcs    = pcs->ppcs;
    FrameHeader*             frm_hdr = &ppcs->frm_hdr;

    // set initial SB base_q_idx values
    frm_hdr->delta_q_params.delta_q_present = 0;
    for (int sb_addr = 0; sb_addr < pcs->sb_total_count; ++sb_addr) {
        pcs->sb_ptr_array[sb_addr]->qindex = frm_hdr->quantization_params.base_q_idx;
    }

    // adjust SB qindex based on variance
    // note: do not enable Variance Boost for CBR rate control mode
    if (scs->static_config.enable_variance_boost && scs->enc_ctx->rc_cfg.mode != AOM_CBR) {
        svt_av1_variance_adjust_qp(pcs);
    }
    // QPM with tpl_la
    if (scs->static_config.aq_mode == 2 && ppcs->tpl_ctrls.enable && ppcs->r0 != 0) {
        svt_aom_sb_qp_derivation_tpl_la(pcs);
    }
    if (ppcs->cyclic_refresh.apply_cyclic_refresh) {
        cyclic_sb_qp_derivation(pcs);
    }

    if (frm_hdr->delta_q_params.delta_q_present && frm_hdr->delta_q_params.delta_q_res != 1) {
        // adjust delta q res and normalize superblock delta q values to reduce signaling overhead
        svt_av1_normalize_sb_delta_q(pcs);
    }

    // Derive a QP per 64x64 using ME distortions (to be used for lambda modulation only; not at Q/Q-1)
    if (scs->stats_based_sb_lambda_modulation) {
        generate_b64_me_qindex_map(pcs);
    }
}

/******************************************************
 *  svt_aom_cyclic_refresh_init
 * Initial cyclic refresh parameters
 ******************************************************/
void svt_aom_cyclic_refresh_init(PictureParentControlSet* ppcs) {
    SequenceControlSet* scs = ppcs->scs;
    CyclicRefresh*      cr  = &ppcs->cyclic_refresh;

    EncodeContext* enc_ctx = scs->enc_ctx;
    RATE_CONTROL*  rc      = &enc_ctx->rc;

    // Cases to reset the cyclic refresh adjustment parameters.
    if (ppcs->slice_type == I_SLICE) {
        // Reset adaptive elements for intra only frames and scene changes.
        rc->percent_refresh_adjustment   = 5;
        rc->rate_ratio_qdelta_adjustment = 0.25;
    }

    cr->percent_refresh = 20 + rc->percent_refresh_adjustment;

    if (ppcs->sc_class1) {
        cr->percent_refresh += 5;
    }

    cr->apply_cyclic_refresh = (ppcs->slice_type != I_SLICE &&
                                (ppcs->scs->use_flat_ipp || ppcs->temporal_layer_index == 0));

    int qp_thresh     = AOMMAX(16, rc->best_quality + 4);
    int qp_max_thresh = 118 * MAXQ >> 7;

    if (rc->avg_frame_qindex[INTER_FRAME] > qp_max_thresh) {
        cr->apply_cyclic_refresh = 0;
    }

    if (rc->avg_frame_qindex[INTER_FRAME] < qp_thresh) {
        cr->apply_cyclic_refresh = 0;
    }

    if (rc->avg_frame_low_motion && rc->avg_frame_low_motion < 50) {
        cr->apply_cyclic_refresh = 0;
    }

    if (!cr->apply_cyclic_refresh) {
        return;
    }

    uint16_t sb_cnt = scs->sb_total_count;

    cr->sb_start            = scs->enc_ctx->cr_sb_end;
    cr->sb_end              = cr->sb_start + sb_cnt * cr->percent_refresh / 100;
    scs->enc_ctx->cr_sb_end = cr->sb_end >= sb_cnt ? 0 : cr->sb_end;

    // Use larger delta - qp(increase rate_ratio_qdelta) for first few(~4)
    // periods of the refresh cycle, after a key frame.
    cr->max_qdelta_perc = 60;

    // Use larger delta-qp (increase rate_ratio_qdelta) for first few
    // refresh cycles after a key frame (svc) or scene change (non svc).
    // For non svc screen content, after a scene change gradually reduce
    // this boost and suppress it further if either of the previous two
    // frames overshot.
    if (cr->percent_refresh > 0) {
        if (!ppcs->sc_class1) {
            cr->rate_ratio_qdelta = ((uint64_t)rc->frames_since_key <
                                     (uint64_t)(4 * (1 << scs->static_config.hierarchical_levels) * 100 /
                                                cr->percent_refresh))
                ? 1.50
                : 1.15;
            cr->rate_ratio_qdelta += rc->rate_ratio_qdelta_adjustment;
            cr->rate_boost_fac = 15;
        } else {
            double distance_from_sc_factor = AOMMIN(0.75, (rc->frames_since_key / 10) * 0.1);
            cr->rate_ratio_qdelta          = 2.25 + rc->rate_ratio_qdelta_adjustment - distance_from_sc_factor;
            if (rc->rc_1_frame < 0 || rc->rc_2_frame < 0) {
                cr->rate_ratio_qdelta -= 0.25;
            }
            cr->rate_boost_fac = 10;
        }
    } else {
        cr->rate_ratio_qdelta = 1.50 + rc->rate_ratio_qdelta_adjustment;
    }
}
