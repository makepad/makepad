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

#include "pcs.h"
#include "sequence_control_set.h"

#include "src_ops_process.h"
#include "initial_rc_results.h"
#include "pic_demux_results.h"
#ifdef ARCH_X86_64
#include <emmintrin.h>
#endif
#include "enc_handle.h"
#include "utility.h"
#include "pic_manager_process.h"
#include "reference_object.h"
#include "transforms.h"
#include "aom_dsp_rtcd.h"
#include "svt_log.h"
#include "intra_prediction.h"
#include "motion_estimation.h"
#include "enc_dec_results.h"
#include "rd_cost.h"
#include "rc_process.h"
#include "mcomp.h"
#include "av1me.h"
#include "enc_inter_prediction.h"
#include "resize.h"

/**************************************
 * Context
 **************************************/
typedef struct SourceBasedOperationsContext {
    EbDctor  dctor;
    EbFifo*  initial_rate_control_results_input_fifo_ptr;
    EbFifo*  picture_demux_results_output_fifo_ptr;
    EbFifo*  sbo_output_fifo_ptr;
    uint8_t* y_mean_ptr;
    uint8_t* cr_mean_ptr;
    uint8_t* cb_mean_ptr;
} SourceBasedOperationsContext;

typedef struct TplDispenserContext {
    EbDctor  dctor;
    EbFifo*  tpl_disp_input_fifo_ptr;
    EbFifo*  tpl_disp_fb_fifo_ptr;
    uint32_t sb_index;
    uint32_t coded_sb_count;
} TplDispenserContext;

static void source_based_operations_context_dctor(EbPtr p) {
    EbThreadContext*              thread_ctx = (EbThreadContext*)p;
    SourceBasedOperationsContext* obj        = (SourceBasedOperationsContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

/************************************************
 * Source Based Operation Context Constructor
 ************************************************/
EbErrorType svt_aom_source_based_operations_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                         int tpl_index, int index) {
    SourceBasedOperationsContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = source_based_operations_context_dctor;

    context_ptr->initial_rate_control_results_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->initial_rate_control_results_resource_ptr, index);
    context_ptr->sbo_output_fifo_ptr = svt_system_resource_get_producer_fifo(enc_handle_ptr->tpl_disp_res_srm,
                                                                             tpl_index);

    context_ptr->picture_demux_results_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->picture_demux_results_resource_ptr, index);

    return EB_ErrorNone;
}

/*
     TPL dispenser context dctor
*/
static void tpl_disp_context_dctor(EbPtr p) {
    EbThreadContext*     thread_ctx = (EbThreadContext*)p;
    TplDispenserContext* obj        = (TplDispenserContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

/*
     TPL dispenser context cctor
*/
EbErrorType svt_aom_tpl_disp_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index,
                                          int tasks_index) {
    TplDispenserContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);

    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = tpl_disp_context_dctor;

    context_ptr->tpl_disp_input_fifo_ptr = svt_system_resource_get_consumer_fifo(enc_handle_ptr->tpl_disp_res_srm,
                                                                                 index);

    context_ptr->tpl_disp_fb_fifo_ptr = svt_system_resource_get_producer_fifo(enc_handle_ptr->tpl_disp_res_srm,
                                                                              tasks_index);

    return EB_ErrorNone;
}

/* this function sets up ME refs for a regular pic*/
static void tpl_regular_setup_me_refs(PictureParentControlSet* base_pcs, PictureParentControlSet* cur_pcs) {
    for (uint8_t list_index = REF_LIST_0; list_index < TOTAL_NUM_OF_REF_LISTS; list_index++) {
        uint8_t ref_list_count = (list_index == REF_LIST_0) ? cur_pcs->ref_list0_count_try
                                                            : cur_pcs->ref_list1_count_try;

        if (list_index == REF_LIST_0) {
            cur_pcs->tpl_data.tpl_ref0_count = ref_list_count;
        } else {
            cur_pcs->tpl_data.tpl_ref1_count = ref_list_count;
        }

        for (uint8_t ref_idx = 0; ref_idx < ref_list_count; ref_idx++) {
            uint64_t ref_poc = cur_pcs->ref_pic_poc_array[list_index][ref_idx];

            cur_pcs->tpl_data.ref_tpl_group_idx[list_index][ref_idx] = -1;
            for (uint32_t j = 0; j < base_pcs->tpl_group_size; j++) {
                if (ref_poc == base_pcs->tpl_group[j]->picture_number) {
                    cur_pcs->tpl_data.ref_in_slide_window[list_index][ref_idx] = true;
                    cur_pcs->tpl_data.ref_tpl_group_idx[list_index][ref_idx]   = j;
                    break;
                }
            }

            EbPaReferenceObject* ref_obj =
                (EbPaReferenceObject*)cur_pcs->ref_pa_pic_ptr_array[list_index][ref_idx]->object_ptr;

            cur_pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_idx].picture_number = ref_obj->picture_number;
            cur_pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_idx].picture_ptr    = ref_obj->input_padded_pic;
            //not needed for TPL but could be linked.
            cur_pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_idx].sixteenth_picture_ptr = NULL;
            cur_pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_idx].quarter_picture_ptr   = NULL;
        }
    }
}

/*
  prepare TPL data fields
*/
static void tpl_prep_info(PictureParentControlSet* pcs) {
    for (uint32_t pic_i = 0; pic_i < pcs->tpl_group_size; ++pic_i) {
        PictureParentControlSet* pcs_tpl = pcs->tpl_group[pic_i];

        pcs_tpl->tpl_data.tpl_ref0_count = 0;
        pcs_tpl->tpl_data.tpl_ref1_count = 0;
        EB_MEMSET(
            pcs_tpl->tpl_data.ref_in_slide_window, 0, MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH * sizeof(bool));
        EB_MEMSET(pcs_tpl->tpl_data.tpl_ref_ds_ptr_array[REF_LIST_0],
                  0,
                  REF_LIST_MAX_DEPTH * sizeof(EbDownScaledBufDescPtrArray));
        EB_MEMSET(pcs_tpl->tpl_data.tpl_ref_ds_ptr_array[REF_LIST_1],
                  0,
                  REF_LIST_MAX_DEPTH * sizeof(EbDownScaledBufDescPtrArray));

        pcs_tpl->tpl_data.tpl_slice_type           = pcs_tpl->slice_type;
        pcs_tpl->tpl_data.tpl_temporal_layer_index = pcs_tpl->temporal_layer_index;
        pcs_tpl->tpl_data.is_ref                   = pcs_tpl->is_ref;
        pcs_tpl->tpl_data.tpl_decode_order         = pcs_tpl->decode_order;

        pcs_tpl->tpl_data.base_pcs = pcs;

        if (pcs_tpl->tpl_data.tpl_slice_type != I_SLICE) {
            tpl_regular_setup_me_refs(pcs, pcs_tpl);
        }
    }
}

// Generate lambda factor to tune lambda based on TPL stats
void generate_lambda_scaling_factor(PictureParentControlSet* pcs, int64_t mc_dep_cost_base) {
    Av1Common*   cm                    = pcs->av1_cm;
    uint8_t      tpl_synth_size_offset = pcs->tpl_ctrls.synth_blk_size == 8 ? 1
             : pcs->tpl_ctrls.synth_blk_size == 16                          ? 2
                                                                            : 3;
    const int    step                  = 1 << (tpl_synth_size_offset);
    const int    mi_cols_sr            = ((pcs->enhanced_unscaled_pic->width + 15) / 16) << 2;
    const int    block_size            = pcs->tpl_ctrls.synth_blk_size == 32 ? BLOCK_32X32 : BLOCK_16X16;
    const int    num_mi_w              = mi_size_wide[block_size];
    const int    num_mi_h              = mi_size_high[block_size];
    const int    num_cols              = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    const int    num_rows              = (cm->mi_rows + num_mi_h - 1) / num_mi_h;
    const int    stride                = mi_cols_sr >> tpl_synth_size_offset;
    const double c                     = 1.2;

    for (int row = 0; row < num_rows; row++) {
        for (int col = 0; col < num_cols; col++) {
            int64_t   recrf_dist_sum   = 0;
            int64_t   mc_dep_delta_sum = 0;
            const int index            = row * num_cols + col;
            for (int mi_row = row * num_mi_h; mi_row < (row + 1) * num_mi_h; mi_row += step) {
                for (int mi_col = col * num_mi_w; mi_col < (col + 1) * num_mi_w; mi_col += step) {
                    if (mi_row >= cm->mi_rows || mi_col >= mi_cols_sr) {
                        continue;
                    }

                    const int index1 = (mi_row >> (tpl_synth_size_offset)) * stride +
                        (mi_col >> (tpl_synth_size_offset));
                    TplStats* tpl_stats_ptr = pcs->pa_me_data->tpl_stats[index1];
                    int64_t   mc_dep_delta  = RDCOST(
                        pcs->pa_me_data->base_rdmult, tpl_stats_ptr->mc_dep_rate, tpl_stats_ptr->mc_dep_dist);
                    recrf_dist_sum += tpl_stats_ptr->recrf_dist;
                    mc_dep_delta_sum += mc_dep_delta;
                }
            }
            double scaling_factors = c;
            if (mc_dep_cost_base && (recrf_dist_sum > 0)) {
                double rk = ((double)(recrf_dist_sum << (RDDIV_BITS))) /
                    ((recrf_dist_sum << RDDIV_BITS) + mc_dep_delta_sum);
                scaling_factors += rk / pcs->r0;
            }
            pcs->pa_me_data->tpl_rdmult_scaling_factors[index] = scaling_factors;
        }
    }

    return;
}

static AOM_INLINE void get_quantize_error(MacroblockPlane* p, const TranLow* coeff, TranLow* qcoeff, TranLow* dqcoeff,
                                          TxSize tx_size, uint16_t* eob, int64_t* recon_error, int64_t* sse) {
    const ScanOrder* const scan_order = get_scan_order(tx_size, DCT_DCT);
    int                    pix_num    = 1 << eb_num_pels_log2_lookup[txsize_to_bsize[tx_size]];
    const int              shift      = tx_size == TX_32X32 ? 0 : 2;

    svt_av1_quantize_fp(coeff,
                        pix_num,
                        p->zbin_qtx,
                        p->round_fp_qtx,
                        p->quant_fp_qtx,
                        p->quant_shift_qtx,
                        qcoeff,
                        dqcoeff,
                        p->dequant_qtx,
                        eob,
                        scan_order->scan,
                        scan_order->iscan);

    *recon_error = svt_av1_block_error(coeff, dqcoeff, pix_num, sse) >> shift;
    *recon_error = AOMMAX(*recon_error, 1);

    *sse = (*sse) >> shift;
    *sse = AOMMAX(*sse, 1);
}

static int rate_estimator(TranLow* qcoeff, int eob, TxSize tx_size) {
    const ScanOrder* const scan_order = get_scan_order(tx_size, DCT_DCT);

    assert((1 << eb_num_pels_log2_lookup[txsize_to_bsize[tx_size]]) >= eob);

    int rate_cost = eob + 1;

    for (int idx = 0; idx < eob; ++idx) {
        int abs_level = abs(qcoeff[scan_order->scan[idx]]);
        rate_cost += svt_log2f(abs_level + 1) + (abs_level > 0);
    }

    return (rate_cost << AV1_PROB_COST_SHIFT);
}

static void result_model_store(PictureParentControlSet* pcs, TplStats* tpl_stats_ptr, uint32_t mb_origin_x,
                               uint32_t mb_origin_y, uint32_t size) {
    tpl_stats_ptr->srcrf_dist = AOMMAX(1, tpl_stats_ptr->srcrf_dist);
    tpl_stats_ptr->recrf_dist = AOMMAX(1, tpl_stats_ptr->recrf_dist);
    tpl_stats_ptr->srcrf_rate = AOMMAX(1, tpl_stats_ptr->srcrf_rate);
    tpl_stats_ptr->recrf_rate = AOMMAX(1, tpl_stats_ptr->recrf_rate);
    if (pcs->tpl_ctrls.synth_blk_size == 32) {
        const int stride  = (((pcs->aligned_width + 31) / 32));
        TplStats* dst_ptr = pcs->pa_me_data->tpl_stats[(mb_origin_y >> 5) * stride + (mb_origin_x >> 5)];

        //write to a 32x32 grid
        *dst_ptr = *tpl_stats_ptr;
    } else if (pcs->tpl_ctrls.synth_blk_size == 16) {
        const int stride  = ((pcs->aligned_width + 15) / 16);
        TplStats* dst_ptr = pcs->pa_me_data->tpl_stats[(mb_origin_y >> 4) * stride + (mb_origin_x >> 4)];

        //write to a 16x16 grid
        if (size == 32) {
            //normalize based on the block size 16x16
            tpl_stats_ptr->srcrf_dist = AOMMAX(1, tpl_stats_ptr->srcrf_dist / 4);
            tpl_stats_ptr->recrf_dist = AOMMAX(1, tpl_stats_ptr->recrf_dist / 4);
            tpl_stats_ptr->srcrf_rate = AOMMAX(1, tpl_stats_ptr->srcrf_rate / 4);
            tpl_stats_ptr->recrf_rate = AOMMAX(1, tpl_stats_ptr->recrf_rate / 4);
            dst_ptr[0]                = *tpl_stats_ptr;
            dst_ptr[1]                = *tpl_stats_ptr;
            dst_ptr[stride]           = *tpl_stats_ptr;
            dst_ptr[stride + 1]       = *tpl_stats_ptr;
        } else if (size == 16) {
            *dst_ptr = *tpl_stats_ptr;
        }
    } else {
        //for small resolution, the 16x16 data is duplicated at an 8x8 grid
        const int stride  = ((pcs->aligned_width + 15) / 16) << 1;
        TplStats* dst_ptr = pcs->pa_me_data->tpl_stats[(mb_origin_y >> 3) * stride + (mb_origin_x >> 3)];

        //write to a 8x8 grid
        if (size == 32) {
            //normalize based on the block size 8x8
            tpl_stats_ptr->srcrf_dist = AOMMAX(1, tpl_stats_ptr->srcrf_dist / 16);
            tpl_stats_ptr->recrf_dist = AOMMAX(1, tpl_stats_ptr->recrf_dist / 16);
            tpl_stats_ptr->srcrf_rate = AOMMAX(1, tpl_stats_ptr->srcrf_rate / 16);
            tpl_stats_ptr->recrf_rate = AOMMAX(1, tpl_stats_ptr->recrf_rate / 16);
            dst_ptr[0]                = *tpl_stats_ptr;
            dst_ptr[1]                = *tpl_stats_ptr;
            dst_ptr[2]                = *tpl_stats_ptr;
            dst_ptr[3]                = *tpl_stats_ptr;

            dst_ptr[stride]     = *tpl_stats_ptr;
            dst_ptr[stride + 1] = *tpl_stats_ptr;
            dst_ptr[stride + 2] = *tpl_stats_ptr;
            dst_ptr[stride + 3] = *tpl_stats_ptr;

            dst_ptr[(stride * 2)]     = *tpl_stats_ptr;
            dst_ptr[(stride * 2) + 1] = *tpl_stats_ptr;
            dst_ptr[(stride * 2) + 2] = *tpl_stats_ptr;
            dst_ptr[(stride * 2) + 3] = *tpl_stats_ptr;

            dst_ptr[(stride * 3)]     = *tpl_stats_ptr;
            dst_ptr[(stride * 3) + 1] = *tpl_stats_ptr;
            dst_ptr[(stride * 3) + 2] = *tpl_stats_ptr;
            dst_ptr[(stride * 3) + 3] = *tpl_stats_ptr;

        } else if (size == 16) {
            //normalize based on the block size 8x8
            tpl_stats_ptr->srcrf_dist = AOMMAX(1, tpl_stats_ptr->srcrf_dist / 4);
            tpl_stats_ptr->recrf_dist = AOMMAX(1, tpl_stats_ptr->recrf_dist / 4);
            tpl_stats_ptr->srcrf_rate = AOMMAX(1, tpl_stats_ptr->srcrf_rate / 4);
            tpl_stats_ptr->recrf_rate = AOMMAX(1, tpl_stats_ptr->recrf_rate / 4);
            dst_ptr[0]                = *tpl_stats_ptr;
            dst_ptr[1]                = *tpl_stats_ptr;
            dst_ptr[stride]           = *tpl_stats_ptr;
            dst_ptr[stride + 1]       = *tpl_stats_ptr;
        }
    }
}

/*
    TPL Dispenser SB based (sz 64x64)
*/
static uint8_t tpl_blk_idx_tab[2][21] = {
    /*CU index*/ {0, 1, 22, 43, 64, 2, 7, 12, 17, 23, 28, 33, 38, 44, 49, 54, 59, 65, 70, 75, 80},
    /*ME index*/ {0, 1, 2, 3, 4, 5, 6, 9, 10, 7, 8, 11, 12, 13, 14, 17, 18, 15, 16, 19, 20}};

/*
neigh array for DC prediction and avail references
*/
static inline void get_neighbor_samples_dc(uint8_t* src, uint32_t src_stride, uint8_t* above0_row, uint8_t* left0_col,
                                           uint8_t bsize) {
    //top left
    above0_row[-1] = left0_col[-1] = src[-(int)src_stride - 1];
    //top
    memcpy(above0_row, src - src_stride, bsize);
    //left
    uint8_t* read_ptr = src - 1;
    for (uint32_t idx = 0; idx < bsize; ++idx) {
        left0_col[idx] = *read_ptr;
        read_ptr += src_stride;
    }
}

#define MAX_TPL_MODE 3
#define MAX_TPL_SIZE 32
#define MAX_TPL_SAMPLES_PER_BLOCK MAX_TPL_SIZE* MAX_TPL_SIZE
#define TPL_RDMULT_SCALING_FACTOR 6 // rdmult used in TPL will be divided by this factor
static uint32_t size_array[MAX_TPL_MODE]         = {16, 32, 64};
static uint32_t blk_start_array[MAX_TPL_MODE]    = {5, 1, 0};
static uint32_t blk_end_array[MAX_TPL_MODE]      = {20, 4, 0};
static TxSize   tx_size_array[MAX_TPL_MODE]      = {TX_16X16, TX_32X32, TX_64X64};
static TxSize   sub2_tx_size_array[MAX_TPL_MODE] = {TX_16X8, TX_32X16, TX_64X32};
static TxSize   sub4_tx_size_array[MAX_TPL_MODE] = {TX_16X4, TX_32X8, TX_64X16};

static void svt_tpl_init_mv_cost_params(svt_mv_cost_param* mv_cost_params, const Mv* ref_mv, uint8_t base_q_idx,
                                        uint32_t rdmult, uint8_t hbd_md) {
    mv_cost_params->ref_mv        = ref_mv;
    mv_cost_params->full_ref_mv   = get_fullmv_from_mv(ref_mv);
    mv_cost_params->early_exit_th = 0;
    mv_cost_params->error_per_bit = AOMMAX(rdmult >> RD_EPB_SHIFT, 1);
    mv_cost_params->sad_per_bit   = svt_aom_get_sad_per_bit(base_q_idx, hbd_md);

    /*
    TPL has no rate estimation update for mv costs, so MV_COST_NONE is used.
    If MV_COST_ENTROPY is to be used, must assign TPL its own mv cost arrays to avoid overwriting MD data and causing a r2r.
    MV_COST_ENTROPY could only help if mvjcost and mvcost are not all zeroes (otherwise it's the same as MV_COST_NONE).
    */
    mv_cost_params->mv_cost_type = MV_COST_NONE;
    mv_cost_params->mvjcost      = NULL;
    mv_cost_params->mvcost[0]    = NULL;
    mv_cost_params->mvcost[1]    = NULL;
}

// Initialize xd fields required by TPL dispenser
static void init_xd_tpl(MacroBlockD* xd, const Av1Common* const cm, const BlockSize block_size,
                        const uint32_t mb_origin_x, const uint32_t mb_origin_y) {
    const int32_t bw      = mi_size_wide[block_size];
    const int32_t bh      = mi_size_high[block_size];
    const int     mi_row  = mb_origin_y >> MI_SIZE_LOG2;
    const int     mi_col  = mb_origin_x >> MI_SIZE_LOG2;
    xd->mb_to_top_edge    = -((mi_row * MI_SIZE) * 8);
    xd->mb_to_bottom_edge = ((cm->mi_rows - bh - mi_row) * MI_SIZE) * 8;
    xd->mb_to_left_edge   = -((mi_col * MI_SIZE) * 8);
    xd->mb_to_right_edge  = ((cm->mi_cols - bw - mi_col) * MI_SIZE) * 8;
    xd->mi_row            = -xd->mb_to_top_edge / (8 * MI_SIZE);
    xd->mi_col            = -xd->mb_to_left_edge / (8 * MI_SIZE);
}

// best_mv inputs the starting full-pel MV, around which subpel search is to be performed and outputs the new best subpel MV
static void tpl_subpel_search(SequenceControlSet* scs, PictureParentControlSet* pcs, EbPictureBufferDesc* ref_pic,
                              EbPictureBufferDesc* input_pic, MacroBlockD* xd, const uint32_t mb_origin_x,
                              const uint32_t mb_origin_y, const uint8_t bsize, Mv* best_mv) {
    const Av1Common* const cm         = pcs->av1_cm;
    const BlockSize        block_size = bsize == 8 ? BLOCK_8X8
               : bsize == 16                       ? BLOCK_16X16
               : bsize == 32                       ? BLOCK_32X32
                                                   : BLOCK_64X64;

    // ref_mv is used to calculate the cost of the motion vector
    Mv ref_mv;
    ref_mv.as_int = 0;
    // High level params
    SUBPEL_MOTION_SEARCH_PARAMS  ms_params_struct;
    SUBPEL_MOTION_SEARCH_PARAMS* ms_params = &ms_params_struct;

    ms_params->allow_hp       = 0; // Allow MAX QUARTER_PEL because pcs->frm_hdr.allow_high_precision_mv is not set yet
    ms_params->forced_stop    = pcs->tpl_ctrls.subpel_depth;
    ms_params->iters_per_step = 2; // Maximum number of steps in logarithmic subpel search before giving up. [1, 2]

    // Set up limit values for MV components.
    // Mv beyond the range do not produce new/different prediction block.
    MvLimits mv_limits;
    int      mi_width  = mi_size_wide[block_size];
    int      mi_height = mi_size_high[block_size];
    mv_limits.row_min  = -(((xd->mi_row + mi_height) * MI_SIZE) + AOM_INTERP_EXTEND);
    mv_limits.col_min  = -(((xd->mi_col + mi_width) * MI_SIZE) + AOM_INTERP_EXTEND);
    mv_limits.row_max  = (cm->mi_rows - xd->mi_row) * MI_SIZE + AOM_INTERP_EXTEND;
    mv_limits.col_max  = (cm->mi_cols - xd->mi_col) * MI_SIZE + AOM_INTERP_EXTEND;
    svt_av1_set_mv_search_range(&mv_limits, &ref_mv);
    svt_av1_set_subpel_mv_search_range(&ms_params->mv_limits, (FullMvLimits*)&mv_limits, &ref_mv);

    // Mvcost params
    int32_t qIndex = quantizer_to_qindex[(uint8_t)scs->static_config.qp] +
        scs->static_config.extended_crf_qindex_offset;
    qIndex          = AOMMIN(MAXQ, qIndex);
    uint32_t rdmult = svt_aom_compute_rd_mult_based_on_qindex(EB_EIGHT_BIT, pcs->update_type, qIndex) /
        TPL_RDMULT_SCALING_FACTOR;
    svt_tpl_init_mv_cost_params(&ms_params->mv_cost_params, &ref_mv, qIndex, rdmult,
                                0); // 10BIT not supported

    // Subpel variance params
    ms_params->var_params.vfp                = &svt_aom_mefn_ptr[block_size];
    ms_params->var_params.subpel_search_type = USE_4_TAPS;
    ms_params->var_params.w                  = block_size_wide[block_size];
    ms_params->var_params.h                  = block_size_high[block_size];

    // Ref and src buffers
    MSBuffers* ms_buffers       = &ms_params->var_params.ms_buffers;
    int32_t    ref_origin_index = ref_pic->org_x + mb_origin_x + (mb_origin_y + ref_pic->org_y) * ref_pic->stride_y;

    // Ref buffer
    struct svt_buf_2d ref_struct;
    ref_struct.buf    = ref_pic->buffer_y + ref_origin_index;
    ref_struct.width  = ref_pic->width;
    ref_struct.height = ref_pic->height;
    ref_struct.stride = ref_pic->stride_y;
    ms_buffers->ref   = &ref_struct;

    // Src buffer
    uint32_t input_origin_index = (mb_origin_x + input_pic->org_x) +
        (mb_origin_y + input_pic->org_y) * input_pic->stride_y;
    struct svt_buf_2d src_struct;
    src_struct.buf    = input_pic->buffer_y + input_origin_index;
    src_struct.width  = input_pic->width;
    src_struct.height = input_pic->height;
    src_struct.stride = input_pic->stride_y;
    ms_buffers->src   = &src_struct;

    Mv best_sp_mv;
    // TODO: should use get_fullmv_from_mv instead of shifting
    best_sp_mv.x       = best_mv->x >> 3;
    best_sp_mv.y       = best_mv->y >> 3;
    Mv subpel_start_mv = get_mv_from_fullmv(&best_sp_mv);

    int          not_used = 0;
    unsigned int pred_sse = 0; // not used

    // Assign which subpel search method to use - always use pruned because tested regular and did not give any gain
    fractional_mv_step_fp* subpel_search_method = svt_av1_find_best_sub_pixel_tree_pruned;
    ms_params->pred_variance_th                 = 0;
    ms_params->abs_th_mult                      = 0;
    ms_params->round_dev_th                     = MAX_SIGNED_VALUE;
    ms_params->skip_diag_refinement             = pcs->tpl_ctrls.subpel_diag_refinement;
    ms_params->var_params.bias_fp               = 0;
    uint8_t early_exit                          = 0;
    subpel_search_method(NULL,
                         xd,
                         (const struct AV1Common* const)cm,
                         ms_params,
                         subpel_start_mv,
                         &best_sp_mv,
                         &not_used,
                         &pred_sse,
                         qIndex,
                         block_size,
                         early_exit);

    // Update the MV to the new best
    best_mv->as_int = best_sp_mv.as_int;
}

static void tpl_mc_flow_dispenser_sb_generic(EncodeContext* enc_ctx, SequenceControlSet* scs,
                                             PictureParentControlSet* pcs, int32_t frame_idx, uint32_t sb_index,
                                             int32_t qIndex, uint8_t dispenser_search_level) {
    uint32_t size      = size_array[dispenser_search_level];
    uint32_t blk_start = blk_start_array[dispenser_search_level];
    uint32_t blk_end   = blk_end_array[dispenser_search_level];

    int16_t      x_curr_mv    = 0;
    int16_t      y_curr_mv    = 0;
    uint32_t     me_mb_offset = 0;
    TplControls* tpl_ctrls    = &pcs->tpl_ctrls;

    TxSize tx_size = (tpl_ctrls->subsample_tx == 2) ? sub4_tx_size_array[dispenser_search_level]
        : (tpl_ctrls->subsample_tx == 1)            ? sub2_tx_size_array[dispenser_search_level]
                                                    : tx_size_array[dispenser_search_level];

    EbPictureBufferDesc* ref_pic_ptr;
    EbPictureBufferDesc* input_pic = pcs->enhanced_pic;
    EbPictureBufferDesc* recon_pic = enc_ctx->mc_flow_rec_picture_buffer[frame_idx];
    TplStats             tpl_stats;

    DECLARE_ALIGNED(16, uint8_t, predictor8[(const uint32_t)MAX_TPL_SAMPLES_PER_BLOCK * 2]);
    memset(predictor8, 0, sizeof(predictor8));
    DECLARE_ALIGNED(16, int16_t, src_diff[MAX_TPL_SAMPLES_PER_BLOCK]);
    DECLARE_ALIGNED(32, TranLow, coeff[MAX_TPL_SAMPLES_PER_BLOCK]);
    DECLARE_ALIGNED(32, TranLow, qcoeff[MAX_TPL_SAMPLES_PER_BLOCK]);
    DECLARE_ALIGNED(32, TranLow, dqcoeff[MAX_TPL_SAMPLES_PER_BLOCK]);
    DECLARE_ALIGNED(32, TranLow, best_coeff[MAX_TPL_SAMPLES_PER_BLOCK]);
    DECLARE_ALIGNED(16, uint8_t, compensated_blk[(const uint32_t)MAX_TPL_SAMPLES_PER_BLOCK]);
    uint8_t* predictor = predictor8;

    MacroblockPlane mb_plane;
    mb_plane.quant_qtx       = scs->enc_ctx->quants_8bit.y_quant[qIndex];
    mb_plane.quant_fp_qtx    = scs->enc_ctx->quants_8bit.y_quant_fp[qIndex];
    mb_plane.round_fp_qtx    = scs->enc_ctx->quants_8bit.y_round_fp[qIndex];
    mb_plane.quant_shift_qtx = scs->enc_ctx->quants_8bit.y_quant_shift[qIndex];
    mb_plane.zbin_qtx        = scs->enc_ctx->quants_8bit.y_zbin[qIndex];
    mb_plane.round_qtx       = scs->enc_ctx->quants_8bit.y_round[qIndex];
    mb_plane.dequant_qtx     = scs->enc_ctx->deq_8bit.y_dequant_qtx[qIndex];

    const uint32_t src_stride      = pcs->enhanced_pic->stride_y;
    B64Geom*       b64_geom        = &scs->b64_geom[sb_index];
    const int      aligned16_width = (pcs->aligned_width + 15) >> 4;

    const uint8_t disable_intra_pred = (pcs->tpl_ctrls.disable_intra_pred_nref &&
                                        (pcs->temporal_layer_index == pcs->hierarchical_levels));
    const uint8_t intra_dc_sad_path  = pcs->tpl_ctrls.use_sad_in_src_search && pcs->tpl_ctrls.intra_mode_end == DC_PRED;

    for (uint32_t blk_index = blk_start; blk_index <= blk_end; blk_index++) {
        uint32_t               z_blk_index   = tpl_blk_idx_tab[0][blk_index];
        const CodedBlockStats* blk_stats_ptr = svt_aom_get_coded_blk_stats(z_blk_index);
        const uint8_t          bsize         = blk_stats_ptr->size;
        const BlockSize        block_size    = bsize == 8 ? BLOCK_8X8
                      : bsize == 16                       ? BLOCK_16X16
                      : bsize == 32                       ? BLOCK_32X32
                                                          : BLOCK_64X64;
        const uint32_t         mb_origin_x   = b64_geom->org_x + blk_stats_ptr->org_x;
        const uint32_t         mb_origin_y   = b64_geom->org_y + blk_stats_ptr->org_y;

        // at least half of the block inside
        if (mb_origin_x + (size >> 1) > pcs->enhanced_pic->width ||
            mb_origin_y + (size >> 1) > pcs->enhanced_pic->height) {
            continue;
        }

        MacroBlockD xd;
        init_xd_tpl(&xd, pcs->av1_cm, block_size, mb_origin_x, mb_origin_y);

        const int dst_buffer_stride = recon_pic->stride_y;
        const int dst_mb_offset     = mb_origin_y * dst_buffer_stride + mb_origin_x;
        const int dst_basic_offset  = recon_pic->org_y * recon_pic->stride_y + recon_pic->org_x;
        uint8_t*  dst_buffer        = recon_pic->buffer_y + dst_basic_offset + dst_mb_offset;
        uint8_t*  src_mb            = input_pic->buffer_y + input_pic->org_x + mb_origin_x +
            (input_pic->org_y + mb_origin_y) * src_stride;

        int64_t  recon_error = 1, sse = 1;
        uint64_t best_ref_poc = 0;
        int32_t  best_rf_idx  = -1;

        Mv final_best_mv = {{0, 0}};

        uint8_t best_mode = DC_PRED;
        memset(&tpl_stats, 0, sizeof(tpl_stats));

        PredictionMode best_intra_mode = DC_PRED;

        TplSrcStats* tpl_src_stats_buffer =
            &pcs->pa_me_data->tpl_src_stats_buffer[(mb_origin_y >> 4) * aligned16_width + (mb_origin_x >> 4)];

        //perform src based path if not yet done in previous TPL groups
        if (pcs->tpl_src_data_ready == 0) {
            int64_t inter_cost;
            int64_t best_inter_cost = INT64_MAX;
            int64_t best_intra_cost = INT64_MAX;
            if (!disable_intra_pred) {
                // ois always process as block16x16 even bsize or tx_size is 8x8
                // fast (DC only + sad ) path
                if (intra_dc_sad_path) {
                    DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, above0_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);
                    DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, left0_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);
                    uint8_t* above0_row = above0_data + MAX_TPL_SIZE;
                    uint8_t* left0_col  = left0_data + MAX_TPL_SIZE;

                    const uint8_t mb_inside = (mb_origin_x + size <= pcs->enhanced_pic->width) &&
                        (mb_origin_y + size <= pcs->enhanced_pic->height);
                    if (mb_origin_x > 0 && mb_origin_y > 0 && mb_inside) {
                        get_neighbor_samples_dc(src_mb, src_stride, above0_row, left0_col, bsize);
                    } else {
                        svt_aom_update_neighbor_samples_array_open_loop_mb(1,
                                                                           1,
                                                                           above0_row - 1,
                                                                           left0_col - 1,
                                                                           input_pic,
                                                                           src_stride,
                                                                           mb_origin_x,
                                                                           mb_origin_y,
                                                                           bsize,
                                                                           bsize);
                    }

                    //TODO: combine dc prediction+sad into one kernel
                    svt_aom_intra_prediction_open_loop_mb(
                        0,
                        DC_PRED,
                        mb_origin_x,
                        mb_origin_y,
                        tx_size_array[dispenser_search_level], // use full block for prediction
                        above0_row,
                        left0_col,
                        predictor,
                        size);

                    best_intra_cost = svt_nxm_sad_kernel(src_mb, src_stride, predictor, size, size, size);

                } else {
                    DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, left0_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);
                    DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, above0_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);
                    DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, left_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);
                    DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, above_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);

                    uint8_t* above_row;
                    uint8_t* left_col;
                    uint8_t* above0_row;
                    uint8_t* left0_col;
                    above0_row = above0_data + MAX_TPL_SIZE;
                    left0_col  = left0_data + MAX_TPL_SIZE;
                    above_row  = above_data + MAX_TPL_SIZE;
                    left_col   = left_data + MAX_TPL_SIZE;

                    // Fill Neighbor Arrays
                    svt_aom_update_neighbor_samples_array_open_loop_mb(1,
                                                                       1,
                                                                       above0_row - 1,
                                                                       left0_col - 1,
                                                                       input_pic,
                                                                       input_pic->stride_y,
                                                                       mb_origin_x,
                                                                       mb_origin_y,
                                                                       bsize,
                                                                       bsize);
                    uint8_t intra_mode_end = pcs->tpl_ctrls.intra_mode_end;

                    for (uint8_t ois_intra_mode = DC_PRED; ois_intra_mode <= intra_mode_end; ++ois_intra_mode) {
                        int32_t p_angle = av1_is_directional_mode((PredictionMode)ois_intra_mode)
                            ? mode_to_angle_map[(PredictionMode)ois_intra_mode]
                            : 0;

                        // Edge filter
                        if (av1_is_directional_mode((PredictionMode)ois_intra_mode)) {
                            svt_memcpy(left_data, left0_data, sizeof(uint8_t) * (MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2));
                            svt_memcpy(above_data, above0_data, sizeof(uint8_t) * (MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2));
                            above_row = above_data + MAX_TPL_SIZE;
                            left_col  = left_data + MAX_TPL_SIZE;
                            svt_aom_filter_intra_edge(ois_intra_mode,
                                                      scs->max_input_luma_width,
                                                      scs->max_input_luma_height,
                                                      p_angle,
                                                      (int32_t)mb_origin_x,
                                                      (int32_t)mb_origin_y,
                                                      above_row,
                                                      left_col);
                        } else {
                            above_row = above0_row;
                            left_col  = left0_col;
                        }
                        // PRED
                        svt_aom_intra_prediction_open_loop_mb(
                            p_angle,
                            ois_intra_mode,
                            mb_origin_x,
                            mb_origin_y,
                            tx_size_array[dispenser_search_level], // use full block for prediction
                            above_row,
                            left_col,
                            predictor,
                            size);

                        // Distortion
                        int64_t intra_cost;
                        if (pcs->tpl_ctrls.use_sad_in_src_search) {
                            intra_cost = svt_nxm_sad_kernel(src_mb, input_pic->stride_y, predictor, size, size, size);
                        } else {
                            svt_aom_subtract_block(size >> tpl_ctrls->subsample_tx,
                                                   size,
                                                   src_diff,
                                                   size << tpl_ctrls->subsample_tx,
                                                   src_mb,
                                                   input_pic->stride_y << tpl_ctrls->subsample_tx,
                                                   predictor,
                                                   size << tpl_ctrls->subsample_tx);

                            EB_TRANS_COEFF_SHAPE pf_shape = pcs->tpl_ctrls.pf_shape;
                            svt_av1_wht_fwd_txfm(
                                src_diff, size << tpl_ctrls->subsample_tx, coeff, tx_size, pf_shape, 8, 0);

                            intra_cost = svt_aom_satd(coeff, (size * size) >> tpl_ctrls->subsample_tx)
                                << tpl_ctrls->subsample_tx;
                        }

                        if (intra_cost < best_intra_cost) {
                            best_mode       = ois_intra_mode;
                            best_intra_cost = intra_cost;
                            best_intra_mode = ois_intra_mode;
                        }
                    }
                }
            }

            //Inter Src path
            me_mb_offset = tpl_blk_idx_tab[1][blk_index];
            if (!pcs->enable_me_16x16) {
                me_mb_offset = (me_mb_offset - 1) / 4;
            }
            const MeSbResults* me_results = pcs->pa_me_data->me_results[sb_index];
            const MeCandidate* me_block_results =
                &me_results->me_candidate_array[me_mb_offset * pcs->pa_me_data->max_cand];
            const uint8_t total_me_cnt = pcs->slice_type == I_SLICE
                ? 0
                : me_results->total_me_candidate_index[me_mb_offset];

            for (uint32_t me_cand_i = 0; me_cand_i < total_me_cnt; ++me_cand_i) {
                const MeCandidate* me_cand = &me_block_results[me_cand_i];
                //consider only single refs
                if (me_cand->direction > 1) {
                    continue;
                }

                const uint32_t list_index    = me_cand->direction;
                const uint32_t ref_pic_index = list_index == 0 ? me_cand->ref_idx_l0 : me_cand->ref_idx_l1;
                //exclude this cand if the reference is within the sliding window and does not have valid TPL recon data
                const int32_t ref_grp_idx = pcs->tpl_data.ref_tpl_group_idx[list_index][ref_pic_index];
                if (ref_grp_idx > 0 && pcs->tpl_data.base_pcs->tpl_valid_pic[ref_grp_idx] == 0) {
                    continue;
                }
                const uint32_t rf_idx    = svt_get_ref_frame_type(list_index, ref_pic_index) - 1;
                const uint32_t me_offset = me_mb_offset * pcs->pa_me_data->max_refs +
                    (list_index ? pcs->pa_me_data->max_l0 : 0) + ref_pic_index;
                x_curr_mv = (me_results->me_mv_array[me_offset].x) << 3;
                y_curr_mv = (me_results->me_mv_array[me_offset].y) << 3;

                ref_pic_ptr =
                    (EbPictureBufferDesc*)pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_pic_index].picture_ptr;

                if (((int)mb_origin_x + (x_curr_mv >> 3)) < -TPL_PADX) {
                    x_curr_mv = (-TPL_PADX - mb_origin_x) << 3;
                }

                if (((int)mb_origin_x + (int)bsize + (x_curr_mv >> 3)) > (TPL_PADX + (int)ref_pic_ptr->max_width - 1)) {
                    x_curr_mv = ((TPL_PADX + ref_pic_ptr->max_width - 1) - (mb_origin_x + bsize)) << 3;
                }

                if (((int)mb_origin_y + (y_curr_mv >> 3)) < -TPL_PADY) {
                    y_curr_mv = (-TPL_PADY - mb_origin_y) << 3;
                }

                if (((int)mb_origin_y + (int)bsize + (y_curr_mv >> 3)) >
                    (TPL_PADY + (int)ref_pic_ptr->max_height - 1)) {
                    y_curr_mv = ((TPL_PADY + ref_pic_ptr->max_height - 1) - (mb_origin_y + bsize)) << 3;
                }

                Mv best_mv = {{x_curr_mv, y_curr_mv}};

                if (pcs->tpl_ctrls.subpel_depth != FULL_PEL) {
                    tpl_subpel_search(scs, pcs, ref_pic_ptr, input_pic, &xd, mb_origin_x, mb_origin_y, bsize, &best_mv);
                }
                int32_t ref_origin_index = (int32_t)ref_pic_ptr->org_x + ((int32_t)mb_origin_x + (best_mv.x / 8)) +
                    ((int32_t)mb_origin_y + (best_mv.y / 8) + (int32_t)ref_pic_ptr->org_y) *
                        (int32_t)ref_pic_ptr->stride_y;

                // Need to do compensation for subpel, otherwise, can get pixels directly from REF picture
                uint8_t subpel_mv = (best_mv.x & 0x7 || best_mv.y & 0x7);
                if (subpel_mv) {
                    DECLARE_ALIGNED(32, uint16_t, tmp_dst_y[MAX_TPL_SAMPLES_PER_BLOCK]);
                    DECLARE_ALIGNED(16, uint8_t, seg_mask[2 * MAX_TPL_SAMPLES_PER_BLOCK]);
                    ConvolveParams conv_params_y = get_conv_params_no_round(
                        0, 0, 0, tmp_dst_y, MAX_TPL_SIZE, 0 /*is_compound*/, 8 /*bit_depth*/);

                    svt_aom_enc_make_inter_predictor(
                        scs,
                        ref_pic_ptr->buffer_y + ref_pic_ptr->org_x + (ref_pic_ptr->org_y * ref_pic_ptr->stride_y),
                        NULL, // src_ptr_2b,
                        compensated_blk,
                        (int16_t)mb_origin_y,
                        (int16_t)mb_origin_x,
                        best_mv,
                        &scs->sf_identity,
                        &conv_params_y,
                        0, // interp_filters
                        0, // interinter_comp
                        seg_mask,
                        ref_pic_ptr->width,
                        ref_pic_ptr->height,
                        bsize, // bwidth
                        bsize, // bheight
                        block_size,
                        &xd,
                        ref_pic_ptr->stride_y,
                        size,
                        0,
                        0, // ss_y,
                        0, // ss_x,
                        8, // Always use 8bit for now
                        0, // use_intrabc,
                        0,
                        false, // is16bit
                        false, // is_wm
                        NULL); // wm_params
                }

                if (pcs->tpl_ctrls.use_sad_in_src_search) {
                    inter_cost = svt_nxm_sad_kernel(
                        src_mb,
                        input_pic->stride_y,
                        subpel_mv ? compensated_blk : ref_pic_ptr->buffer_y + ref_origin_index,
                        subpel_mv ? size : ref_pic_ptr->stride_y,
                        size,
                        size);
                } else {
                    svt_aom_subtract_block(size >> tpl_ctrls->subsample_tx,
                                           size,
                                           src_diff,
                                           size << tpl_ctrls->subsample_tx,
                                           src_mb,
                                           input_pic->stride_y << tpl_ctrls->subsample_tx,
                                           subpel_mv ? compensated_blk : ref_pic_ptr->buffer_y + ref_origin_index,
                                           (subpel_mv ? size : ref_pic_ptr->stride_y) << tpl_ctrls->subsample_tx);
                    EB_TRANS_COEFF_SHAPE pf_shape = pcs->tpl_ctrls.pf_shape;
                    svt_av1_wht_fwd_txfm(src_diff, size << tpl_ctrls->subsample_tx, coeff, tx_size, pf_shape, 8, 0);

                    inter_cost = svt_aom_satd(coeff, (size * size) >> tpl_ctrls->subsample_tx)
                        << tpl_ctrls->subsample_tx;
                }

                if (inter_cost < best_inter_cost) {
                    if (!pcs->tpl_ctrls.use_sad_in_src_search) {
                        svt_memcpy(best_coeff, coeff, sizeof(best_coeff));
                    }

                    best_ref_poc    = pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_pic_index].picture_number;
                    best_rf_idx     = rf_idx;
                    best_inter_cost = inter_cost;
                    final_best_mv   = best_mv;
                }
            } // rf_idx

            if (best_inter_cost < best_intra_cost) {
                best_mode = NEWMV;
            }

            if (best_mode == NEWMV) {
                uint16_t eob = 0;

                if (pcs->tpl_ctrls.use_sad_in_src_search) {
                    uint32_t list_index    = best_rf_idx < 4 ? 0 : 1;
                    uint32_t ref_pic_index = best_rf_idx >= 4 ? (best_rf_idx - 4) : best_rf_idx;
                    ref_pic_ptr            = pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_pic_index].picture_ptr;
                    int32_t ref_origin_index = (int32_t)ref_pic_ptr->org_x +
                        ((int32_t)mb_origin_x + (final_best_mv.x >> 3)) +
                        ((int32_t)mb_origin_y + (final_best_mv.y >> 3) + (int32_t)ref_pic_ptr->org_y) *
                            (int32_t)ref_pic_ptr->stride_y;
                    // Need to do compensation for subpel, otherwise, can get pixels directly from REF picture
                    uint8_t subpel_mv = (final_best_mv.x & 0x7 || final_best_mv.y & 0x7);
                    if (subpel_mv) {
                        DECLARE_ALIGNED(32, uint16_t, tmp_dst_y[MAX_TPL_SAMPLES_PER_BLOCK]);
                        DECLARE_ALIGNED(16, uint8_t, seg_mask[2 * MAX_TPL_SAMPLES_PER_BLOCK]);
                        ConvolveParams conv_params_y = get_conv_params_no_round(
                            0, 0, 0, tmp_dst_y, MAX_TPL_SIZE, 0 /*is_compound*/, 8 /*bit_depth*/);

                        svt_aom_enc_make_inter_predictor(
                            scs,
                            ref_pic_ptr->buffer_y + ref_pic_ptr->org_x + (ref_pic_ptr->org_y * ref_pic_ptr->stride_y),
                            NULL, // src_ptr_2b,
                            compensated_blk,
                            (int16_t)mb_origin_y,
                            (int16_t)mb_origin_x,
                            final_best_mv, //best_mv,
                            &scs->sf_identity,
                            &conv_params_y,
                            0, // interp_filters
                            0, // interinter_comp
                            seg_mask,
                            ref_pic_ptr->width,
                            ref_pic_ptr->height,
                            bsize, // bwidth
                            bsize, // bheight
                            block_size,
                            &xd,
                            ref_pic_ptr->stride_y,
                            size,
                            0,
                            0, // ss_y,
                            0, // ss_x,
                            8, // Always use 8bit for now
                            0, // use_intrabc,
                            0,
                            false, // is16bit
                            false, // is_wm
                            NULL); // wm_params
                    }

                    svt_aom_subtract_block(size >> tpl_ctrls->subsample_tx,
                                           size,
                                           src_diff,
                                           size << tpl_ctrls->subsample_tx,
                                           src_mb,
                                           input_pic->stride_y << tpl_ctrls->subsample_tx,
                                           subpel_mv ? compensated_blk : ref_pic_ptr->buffer_y + ref_origin_index,
                                           (subpel_mv ? size : ref_pic_ptr->stride_y) << tpl_ctrls->subsample_tx);
                    EB_TRANS_COEFF_SHAPE pf_shape = pcs->tpl_ctrls.pf_shape;

                    svt_av1_wht_fwd_txfm(
                        src_diff, size << tpl_ctrls->subsample_tx, best_coeff, tx_size, pf_shape, 8, 0);
                }

                get_quantize_error(&mb_plane, best_coeff, qcoeff, dqcoeff, tx_size, &eob, &recon_error, &sse);

                int rate_cost        = pcs->tpl_ctrls.compute_rate ? rate_estimator(qcoeff, eob, tx_size) : 0;
                tpl_stats.srcrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << tpl_ctrls->subsample_tx;
                tpl_stats.srcrf_dist = (recon_error << (TPL_DEP_COST_SCALE_LOG2)) << tpl_ctrls->subsample_tx;
            }
            if (scs->tpl_lad_mg > 0) {
                //store src based stats
                tpl_src_stats_buffer->srcrf_dist      = tpl_stats.srcrf_dist;
                tpl_src_stats_buffer->srcrf_rate      = tpl_stats.srcrf_rate;
                tpl_src_stats_buffer->mv              = final_best_mv;
                tpl_src_stats_buffer->best_rf_idx     = best_rf_idx;
                tpl_src_stats_buffer->ref_frame_poc   = best_ref_poc;
                tpl_src_stats_buffer->best_mode       = best_mode;
                tpl_src_stats_buffer->best_intra_mode = best_intra_mode;
            }
        } else {
            // get src based stats from previously computed data
            tpl_stats.srcrf_dist = tpl_src_stats_buffer->srcrf_dist;
            tpl_stats.srcrf_rate = tpl_src_stats_buffer->srcrf_rate;
            final_best_mv        = tpl_src_stats_buffer->mv;
            best_rf_idx          = tpl_src_stats_buffer->best_rf_idx;
            best_ref_poc         = tpl_src_stats_buffer->ref_frame_poc;
            best_mode            = tpl_src_stats_buffer->best_mode;
            best_intra_mode      = tpl_src_stats_buffer->best_intra_mode;
        }

        //Recon path
        if (best_mode == NEWMV) {
            // inter recon with rec_picture as reference pic
            uint64_t ref_poc       = best_ref_poc;
            uint32_t list_index    = best_rf_idx < 4 ? 0 : 1;
            uint32_t ref_pic_index = best_rf_idx >= 4 ? (best_rf_idx - 4) : best_rf_idx;

            if (pcs->tpl_data.ref_in_slide_window[list_index][ref_pic_index]) {
                uint32_t ref_frame_idx = 0;
                while (ref_frame_idx < MAX_TPL_LA_SW && enc_ctx->poc_map_idx[ref_frame_idx] != ref_poc) {
                    ref_frame_idx++;
                }
                assert(ref_frame_idx != MAX_TPL_LA_SW);
                ref_pic_ptr = enc_ctx->mc_flow_rec_picture_buffer[ref_frame_idx];
            } else {
                ref_pic_ptr =
                    (EbPictureBufferDesc*)pcs->tpl_data.tpl_ref_ds_ptr_array[list_index][ref_pic_index].picture_ptr;
            }

            int32_t ref_origin_index = (int32_t)ref_pic_ptr->org_x + ((int32_t)mb_origin_x + (final_best_mv.x >> 3)) +
                ((int32_t)mb_origin_y + (final_best_mv.y >> 3) + (int32_t)ref_pic_ptr->org_y) *
                    (int32_t)ref_pic_ptr->stride_y;
            // REDO COMPENSATION WITH REF PIC (INSTEAD OF REF BEING THE SRC PIC)
            // Need to do compensation for subpel, otherwise, can get pixels directly from RECON picture
            uint8_t subpel_mv = (final_best_mv.x & 0x7 || final_best_mv.y & 0x7);
            if (subpel_mv) {
                DECLARE_ALIGNED(32, uint16_t, tmp_dst_y[MAX_TPL_SAMPLES_PER_BLOCK]);
                DECLARE_ALIGNED(16, uint8_t, seg_mask[2 * MAX_TPL_SAMPLES_PER_BLOCK]);
                ConvolveParams conv_params_y = get_conv_params_no_round(
                    0, 0, 0, tmp_dst_y, MAX_TPL_SIZE, 0 /*is_compound*/, 8 /*bit_depth*/);

                svt_aom_enc_make_inter_predictor(
                    scs,
                    ref_pic_ptr->buffer_y + ref_pic_ptr->org_x + (ref_pic_ptr->org_y * ref_pic_ptr->stride_y),
                    NULL, // src_ptr_2b,
                    dst_buffer,
                    (int16_t)mb_origin_y,
                    (int16_t)mb_origin_x,
                    final_best_mv,
                    &scs->sf_identity,
                    &conv_params_y,
                    0, // interp_filters
                    0, // interinter_comp
                    seg_mask,
                    ref_pic_ptr->width,
                    ref_pic_ptr->height,
                    bsize, // bwidth
                    bsize, // bheight
                    block_size,
                    &xd,
                    ref_pic_ptr->stride_y,
                    dst_buffer_stride,
                    0,
                    0, // ss_y,
                    0, // ss_x,
                    8, // Always 8bit,
                    0, // use_intrabc,
                    0,
                    false, // is16bit
                    false, // is_wm
                    NULL); // wm_params
            } else {
                for (int i = 0; i < (int)size; ++i) {
                    svt_memcpy(dst_buffer + i * dst_buffer_stride,
                               ref_pic_ptr->buffer_y + ref_origin_index + i * ref_pic_ptr->stride_y,
                               sizeof(uint8_t) * (size));
                }
            }
        } else {
            // intra recon

            uint8_t* above_row;
            uint8_t* left_col;
            DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, left_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);
            DECLARE_ALIGNED(MAX_TPL_SIZE, uint8_t, above_data[MAX_TX_SIZE * 2 + MAX_TPL_SIZE * 2]);

            above_row = above_data + MAX_TPL_SIZE;
            left_col  = left_data + MAX_TPL_SIZE;

            uint8_t* recon_buffer = recon_pic->buffer_y + dst_basic_offset;

            if (intra_dc_sad_path) {
                const uint8_t mb_inside = (mb_origin_x + size <= pcs->enhanced_pic->width) &&
                    (mb_origin_y + size <= pcs->enhanced_pic->height);
                if (mb_origin_x > 0 && mb_origin_y > 0 && mb_inside) {
                    get_neighbor_samples_dc(recon_buffer + mb_origin_x + mb_origin_y * dst_buffer_stride,
                                            dst_buffer_stride,
                                            above_row,
                                            left_col,
                                            bsize);
                } else {
                    svt_aom_update_neighbor_samples_array_open_loop_mb_recon(1, // use_top_righ_bottom_left
                                                                             1, // update_top_neighbor
                                                                             above_row - 1,
                                                                             left_col - 1,
                                                                             recon_buffer,
                                                                             dst_buffer_stride,
                                                                             mb_origin_x,
                                                                             mb_origin_y,
                                                                             size,
                                                                             size,
                                                                             input_pic->width,
                                                                             input_pic->height);
                }

                svt_aom_intra_prediction_open_loop_mb(
                    0,
                    DC_PRED,
                    mb_origin_x,
                    mb_origin_y,
                    tx_size_array[dispenser_search_level], // use full block for prediction
                    above_row,
                    left_col,
                    dst_buffer,
                    dst_buffer_stride);

            } else {
                svt_aom_update_neighbor_samples_array_open_loop_mb_recon(1, // use_top_righ_bottom_left
                                                                         1, // update_top_neighbor
                                                                         above_row - 1,
                                                                         left_col - 1,
                                                                         recon_buffer,
                                                                         dst_buffer_stride,
                                                                         mb_origin_x,
                                                                         mb_origin_y,
                                                                         size,
                                                                         size,
                                                                         input_pic->width,
                                                                         input_pic->height);
                uint8_t ois_intra_mode = best_intra_mode;
                int32_t p_angle        = av1_is_directional_mode((PredictionMode)ois_intra_mode)
                           ? mode_to_angle_map[(PredictionMode)ois_intra_mode]
                           : 0;
                // Edge filter
                if (av1_is_directional_mode((PredictionMode)ois_intra_mode)) {
                    svt_aom_filter_intra_edge(ois_intra_mode,
                                              scs->max_input_luma_width,
                                              scs->max_input_luma_height,
                                              p_angle,
                                              mb_origin_x,
                                              mb_origin_y,
                                              above_row,
                                              left_col);
                }
                // PRED
                svt_aom_intra_prediction_open_loop_mb(
                    p_angle,
                    ois_intra_mode,
                    mb_origin_x,
                    mb_origin_y,
                    tx_size_array[dispenser_search_level], // use full block for prediction
                    above_row,
                    left_col,
                    dst_buffer,
                    dst_buffer_stride);
            }
        }

        svt_aom_subtract_block(size >> tpl_ctrls->subsample_tx,
                               size,
                               src_diff,
                               size << tpl_ctrls->subsample_tx,
                               src_mb,
                               input_pic->stride_y << tpl_ctrls->subsample_tx,
                               dst_buffer,
                               dst_buffer_stride << tpl_ctrls->subsample_tx);
        EB_TRANS_COEFF_SHAPE pf_shape = pcs->tpl_ctrls.pf_shape;
        svt_av1_wht_fwd_txfm(src_diff, size << tpl_ctrls->subsample_tx, coeff, tx_size, pf_shape, 8, 0);

        uint16_t eob = 0;

        get_quantize_error(&mb_plane, coeff, qcoeff, dqcoeff, tx_size, &eob, &recon_error, &sse);
        int rate_cost = pcs->tpl_ctrls.compute_rate ? rate_estimator(qcoeff, eob, tx_size) : 0;

        if (!disable_intra_pred || (pcs->tpl_data.is_ref)) {
            if (eob) {
                svt_aom_inv_transform_recon8bit((int32_t*)dqcoeff,
                                                dst_buffer,
                                                dst_buffer_stride << tpl_ctrls->subsample_tx,
                                                dst_buffer,
                                                dst_buffer_stride << tpl_ctrls->subsample_tx,
                                                tx_size,
                                                DCT_DCT,
                                                PLANE_TYPE_Y,
                                                eob,
                                                0);

                // If subsampling is used for the TX, need to populate the missing rows in recon with a copy of the neighbouring rows
                if (tpl_ctrls->subsample_tx == 2) {
                    for (int i = 0; i < (int)size; i += 4) {
                        svt_memcpy(dst_buffer + (i + 1) * dst_buffer_stride,
                                   dst_buffer + i * dst_buffer_stride,
                                   sizeof(uint8_t) * (size));
                        svt_memcpy(dst_buffer + (i + 2) * dst_buffer_stride,
                                   dst_buffer + i * dst_buffer_stride,
                                   sizeof(uint8_t) * (size));
                        svt_memcpy(dst_buffer + (i + 3) * dst_buffer_stride,
                                   dst_buffer + i * dst_buffer_stride,
                                   sizeof(uint8_t) * (size));
                    }
                } else if (tpl_ctrls->subsample_tx == 1) {
                    for (int i = 0; i < (int)size; i += 2) {
                        svt_memcpy(dst_buffer + (i + 1) * dst_buffer_stride,
                                   dst_buffer + i * dst_buffer_stride,
                                   sizeof(uint8_t) * (size));
                    }
                }
            }
        }

        tpl_stats.recrf_dist = (recon_error << (TPL_DEP_COST_SCALE_LOG2)) << tpl_ctrls->subsample_tx;
        tpl_stats.recrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << tpl_ctrls->subsample_tx;
        if (best_mode != NEWMV) {
            tpl_stats.srcrf_dist = (recon_error << (TPL_DEP_COST_SCALE_LOG2)) << tpl_ctrls->subsample_tx;
            tpl_stats.srcrf_rate = (rate_cost << TPL_DEP_COST_SCALE_LOG2) << tpl_ctrls->subsample_tx;
        }

        tpl_stats.recrf_dist = AOMMAX(tpl_stats.srcrf_dist, tpl_stats.recrf_dist);
        tpl_stats.recrf_rate = AOMMAX(tpl_stats.srcrf_rate, tpl_stats.recrf_rate);
        if (pcs->tpl_data.tpl_slice_type != I_SLICE && best_rf_idx != -1) {
            tpl_stats.mv            = final_best_mv;
            tpl_stats.ref_frame_poc = best_ref_poc;
        }

        // Motion flow dependency dispenser.
        result_model_store(pcs, &tpl_stats, mb_origin_x, mb_origin_y, size);
    }
}

#define TPL_TASKS_MDC_INPUT 0
#define TPL_TASKS_ENCDEC_INPUT 1
#define TPL_TASKS_CONTINUE 2

/*
   Assign TPL dispenser segments
*/
static bool assign_tpl_segments(EncDecSegments* segmentPtr, uint16_t* segmentInOutIndex, TplDispResults* taskPtr,
                                int32_t frame_idx, EbFifo* srmFifoPtr) {
    bool     continue_processing_flag = false;
    uint32_t row_segment_index        = 0;
    uint32_t segment_index;
    uint32_t right_segment_index;
    uint32_t bottom_left_segment_index;

    int16_t feedback_row_index = -1;

    uint32_t self_assigned = false;

    //static FILE *trace = 0;
    //
    //if(trace == 0) {
    //    trace = fopen("seg-trace.txt","w");
    //}

    switch (taskPtr->input_type) {
    case TPL_TASKS_MDC_INPUT:

        // The entire picture is provided by the MDC process, so
        //   no logic is necessary to clear input dependencies.
        for (uint32_t row_index = 0; row_index < segmentPtr->segment_row_count; ++row_index) {
            segmentPtr->row_array[row_index].current_seg_index = segmentPtr->row_array[row_index].starting_seg_index;
        }

        // Start on Segment 0 immediately
        *segmentInOutIndex  = segmentPtr->row_array[0].current_seg_index;
        taskPtr->input_type = TPL_TASKS_CONTINUE;
        ++segmentPtr->row_array[0].current_seg_index;
        continue_processing_flag = true;
        break;

    case TPL_TASKS_ENCDEC_INPUT:
        // Start on the assigned row immediately
        *segmentInOutIndex  = segmentPtr->row_array[taskPtr->enc_dec_segment_row].current_seg_index;
        taskPtr->input_type = TPL_TASKS_CONTINUE;
        ++segmentPtr->row_array[taskPtr->enc_dec_segment_row].current_seg_index;
        continue_processing_flag = true;
        break;

    case TPL_TASKS_CONTINUE:

        // Update the Dependency List for Right and Bottom Neighbors
        segment_index     = *segmentInOutIndex;
        row_segment_index = segment_index / segmentPtr->segment_band_count;

        right_segment_index       = segment_index + 1;
        bottom_left_segment_index = segment_index + segmentPtr->segment_band_count;

        // Right Neighbor
        if (segment_index < segmentPtr->row_array[row_segment_index].ending_seg_index) {
            svt_block_on_mutex(segmentPtr->row_array[row_segment_index].assignment_mutex);

            --segmentPtr->dep_map.dependency_map[right_segment_index];

            if (segmentPtr->dep_map.dependency_map[right_segment_index] == 0) {
                *segmentInOutIndex = segmentPtr->row_array[row_segment_index].current_seg_index;
                ++segmentPtr->row_array[row_segment_index].current_seg_index;
                self_assigned            = true;
                continue_processing_flag = true;
            }

            svt_release_mutex(segmentPtr->row_array[row_segment_index].assignment_mutex);
        }

        // Bottom-left Neighbor
        if (row_segment_index < segmentPtr->segment_row_count - 1 &&
            bottom_left_segment_index >= segmentPtr->row_array[row_segment_index + 1].starting_seg_index) {
            svt_block_on_mutex(segmentPtr->row_array[row_segment_index + 1].assignment_mutex);

            --segmentPtr->dep_map.dependency_map[bottom_left_segment_index];

            if (segmentPtr->dep_map.dependency_map[bottom_left_segment_index] == 0) {
                if (self_assigned == true) {
                    feedback_row_index = (int16_t)row_segment_index + 1;
                } else {
                    *segmentInOutIndex = segmentPtr->row_array[row_segment_index + 1].current_seg_index;
                    ++segmentPtr->row_array[row_segment_index + 1].current_seg_index;
                    continue_processing_flag = true;
                }
            }
            svt_release_mutex(segmentPtr->row_array[row_segment_index + 1].assignment_mutex);
        }

        if (feedback_row_index > 0) {
            EbObjectWrapper* out_results_wrapper;

            svt_get_empty_object(srmFifoPtr, &out_results_wrapper);

            TplDispResults* out_results = (TplDispResults*)out_results_wrapper->object_ptr;
            out_results->input_type     = TPL_TASKS_ENCDEC_INPUT;

            out_results->enc_dec_segment_row = feedback_row_index;
            out_results->tile_group_index    = taskPtr->tile_group_index;
            out_results->qIndex              = taskPtr->qIndex;

            out_results->pcs_wrapper = taskPtr->pcs_wrapper;
            out_results->pcs         = taskPtr->pcs;
            out_results->frame_index = frame_idx;
            svt_post_full_object(out_results_wrapper);
        }

        break;

    default:
        break;
    }

    return continue_processing_flag;
}

/************************************************
 * Genrate TPL MC Flow Dispenser  Based on Lookahead
 ** LAD Window: sliding window size
 ************************************************/

static void tpl_mc_flow_dispenser(EncodeContext* enc_ctx, SequenceControlSet* scs, int32_t* base_rdmult,
                                  PictureParentControlSet* pcs, int32_t frame_idx,
                                  SourceBasedOperationsContext* context_ptr) {
    EbPictureBufferDesc* recon_pic = enc_ctx->mc_flow_rec_picture_buffer[frame_idx];
    int32_t              qIndex    = quantizer_to_qindex[(uint8_t)scs->static_config.qp] +
        scs->static_config.extended_crf_qindex_offset;
    qIndex = AOMMIN(MAXQ, qIndex);

    if (pcs->tpl_ctrls.enable_tpl_qps) {
        static const double delta_rate_new[7][6] = {
            {1.0, 1.0, 1.0, 1.0, 1.0, 1.0}, // 1L
            {0.6, 1.0, 1.0, 1.0, 1.0, 1.0}, // 2L
            {0.6, 0.8, 1.0, 1.0, 1.0, 1.0}, // 3L
            {0.6, 0.8, 0.9, 1.0, 1.0, 1.0}, // 4L
            {0.35, 0.6, 0.8, 0.9, 1.0, 1.0}, //5L
            {0.35, 0.6, 0.8, 0.9, 0.95, 1.0} //6L
        };
        double q_val;
        q_val = svt_av1_convert_qindex_to_q(qIndex, 8);
        int32_t delta_qindex;
        if (pcs->tpl_data.tpl_slice_type == I_SLICE) {
            delta_qindex = svt_av1_compute_qdelta(q_val, q_val * 0.25, 8);
        } else {
            delta_qindex = svt_av1_compute_qdelta(
                q_val, q_val * delta_rate_new[pcs->hierarchical_levels][pcs->tpl_data.tpl_temporal_layer_index], 8);
        }
        qIndex = (qIndex + delta_qindex);
    }
    *base_rdmult = svt_aom_compute_rd_mult_based_on_qindex(EB_EIGHT_BIT, pcs->update_type, qIndex) /
        TPL_RDMULT_SCALING_FACTOR;

    {
        {
            // reset number of TPLed sbs per pic
            pcs->tpl_disp_coded_sb_count = 0;

            EbObjectWrapper* out_results_wrapper;

            // TPL dispenser kernel
            svt_get_empty_object(context_ptr->sbo_output_fifo_ptr, &out_results_wrapper);

            TplDispResults* out_results = (TplDispResults*)out_results_wrapper->object_ptr;
            // out_results->pcs_wrapper = pcs->p_pcs_wrapper_ptr;
            out_results->pcs              = pcs;
            out_results->input_type       = TPL_TASKS_MDC_INPUT;
            out_results->tile_group_index = /*tile_group_idx*/ 0;

            out_results->frame_index = frame_idx;
            out_results->qIndex      = qIndex;

            svt_post_full_object(out_results_wrapper);

            svt_block_on_semaphore(pcs->tpl_disp_done_semaphore); // we can do all in // ?
        }
    }

    // padding current recon picture
    svt_aom_generate_padding(recon_pic->buffer_y,
                             recon_pic->stride_y,
                             recon_pic->width,
                             recon_pic->height,
                             recon_pic->org_x,
                             recon_pic->org_y);

    return;
}

static int get_overlap_area(int grid_pos_row, int grid_pos_col, int ref_pos_row, int ref_pos_col, int block,
                            int /*BLOCK_SIZE*/ bsize) {
    int width = 0, height = 0;
    int bw = 4 << mi_size_wide_log2[bsize];
    int bh = 4 << mi_size_high_log2[bsize];

    switch (block) {
    case 0:
        width  = grid_pos_col + bw - ref_pos_col;
        height = grid_pos_row + bh - ref_pos_row;
        break;
    case 1:
        width  = ref_pos_col + bw - grid_pos_col;
        height = grid_pos_row + bh - ref_pos_row;
        break;
    case 2:
        width  = grid_pos_col + bw - ref_pos_col;
        height = ref_pos_row + bh - grid_pos_row;
        break;
    case 3:
        width  = ref_pos_col + bw - grid_pos_col;
        height = ref_pos_row + bh - grid_pos_row;
        break;
    default:
        assert(0);
    }

    return width * height;
}

static int round_floor(int ref_pos, int bsize_pix) {
    int round;
    if (ref_pos < 0) {
        round = -(1 + (-ref_pos - 1) / bsize_pix);
    } else {
        round = ref_pos / bsize_pix;
    }

    return round;
}

static int64_t delta_rate_cost(int64_t delta_rate, int64_t recrf_dist, int64_t srcrf_dist, int pix_num) {
    double  beta      = (double)srcrf_dist / recrf_dist;
    int64_t rate_cost = delta_rate;

    if (srcrf_dist <= 128) {
        return rate_cost;
    }

    double dr = (double)(delta_rate >> (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT)) / pix_num;

    double log_den = log(beta) / log(2.0) + 2.0 * dr;

    if (log_den > log(10.0) / log(2.0)) {
        rate_cost = (int64_t)((log(1.0 / beta) * pix_num) / log(2.0) / 2.0);
        rate_cost <<= (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT);
        return rate_cost;
    }

    double num = pow(2.0, log_den);
    double den = num * beta + (1 - beta) * beta;

    rate_cost = (int64_t)((pix_num * log(num / den)) / log(2.0) / 2.0);

    rate_cost <<= (TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT);

    return rate_cost;
}

/************************************************
* Genrate TPL MC Flow Synthesizer
************************************************/

static AOM_INLINE void tpl_model_update_b(PictureParentControlSet* ref_pcs_ptr, PictureParentControlSet* pcs,
                                          TplStats* tpl_stats_ptr, int mi_row, int mi_col,
                                          const int /*BLOCK_SIZE*/ bsize) {
    Av1Common* ref_cm = ref_pcs_ptr->av1_cm;
    TplStats*  ref_tpl_stats_ptr;

    const Mv  full_mv     = get_fullmv_from_mv(&tpl_stats_ptr->mv);
    const int ref_pos_row = mi_row * MI_SIZE + full_mv.y;
    const int ref_pos_col = mi_col * MI_SIZE + full_mv.x;

    const int bw         = 4 << mi_size_wide_log2[bsize];
    const int bh         = 4 << mi_size_high_log2[bsize];
    const int mi_height  = mi_size_high[bsize];
    const int mi_width   = mi_size_wide[bsize];
    const int pix_num    = bw * bh;
    const int shift      = pcs->tpl_ctrls.synth_blk_size == 8 ? 1 : pcs->tpl_ctrls.synth_blk_size == 16 ? 2 : 3;
    const int mi_cols_sr = ((ref_pcs_ptr->aligned_width + 15) / 16) << 2;

    // top-left on grid block location in pixel
    int grid_pos_row_base = round_floor(ref_pos_row, bh) * bh;
    int grid_pos_col_base = round_floor(ref_pos_col, bw) * bw;
    int block;

    int64_t cur_dep_dist = tpl_stats_ptr->recrf_dist - tpl_stats_ptr->srcrf_dist;
    int64_t mc_dep_dist  = tpl_stats_ptr->mc_dep_dist * (tpl_stats_ptr->recrf_dist - tpl_stats_ptr->srcrf_dist) /
        tpl_stats_ptr->recrf_dist;
    int64_t delta_rate  = tpl_stats_ptr->recrf_rate - tpl_stats_ptr->srcrf_rate;
    int64_t mc_dep_rate = pcs->tpl_ctrls.compute_rate
        ? delta_rate_cost(tpl_stats_ptr->mc_dep_rate, tpl_stats_ptr->recrf_dist, tpl_stats_ptr->srcrf_dist, pix_num)
        : 0;

    for (block = 0; block < 4; ++block) {
        int grid_pos_row = grid_pos_row_base + bh * (block >> 1);
        int grid_pos_col = grid_pos_col_base + bw * (block & 0x01);

        if (grid_pos_row >= 0 && grid_pos_row < ref_cm->mi_rows * MI_SIZE && grid_pos_col >= 0 &&
            grid_pos_col < ref_cm->mi_cols * MI_SIZE) {
            int overlap_area = get_overlap_area(grid_pos_row, grid_pos_col, ref_pos_row, ref_pos_col, block, bsize);
            int ref_mi_row   = round_floor(grid_pos_row, bh) * mi_height;
            int ref_mi_col   = round_floor(grid_pos_col, bw) * mi_width;
            const int step   = 1 << (shift);

            for (int idy = 0; idy < mi_height; idy += step) {
                for (int idx = 0; idx < mi_width; idx += step) {
                    ref_tpl_stats_ptr =
                        ref_pcs_ptr->pa_me_data->tpl_stats[((ref_mi_row + idy) >> shift) * (mi_cols_sr >> shift) +
                                                           ((ref_mi_col + idx) >> shift)];
                    ref_tpl_stats_ptr->mc_dep_dist += ((cur_dep_dist + mc_dep_dist) * overlap_area) / pix_num;
                    ref_tpl_stats_ptr->mc_dep_rate += ((delta_rate + mc_dep_rate) * overlap_area) / pix_num;
                    assert(overlap_area >= 0);
                }
            }
        }
    }
}

/************************************************
* Genrate TPL MC Flow Synthesizer
************************************************/

static AOM_INLINE void tpl_model_update(PictureParentControlSet* pcs_array[MAX_TPL_LA_SW], int32_t frame_idx,
                                        int mi_row, int mi_col, const int /*BLOCK_SIZE*/ bsize, uint8_t frames_in_sw) {
    const int                mi_height = mi_size_high[bsize];
    const int                mi_width  = mi_size_wide[bsize];
    PictureParentControlSet* pcs       = pcs_array[frame_idx];

    const int /*BLOCK_SIZE*/ block_size = pcs->tpl_ctrls.synth_blk_size == 8 ? BLOCK_8X8
        : pcs->tpl_ctrls.synth_blk_size == 16                                ? BLOCK_16X16
                                                                             : BLOCK_32X32;
    const int shift      = pcs->tpl_ctrls.synth_blk_size == 8 ? 1 : pcs->tpl_ctrls.synth_blk_size == 16 ? 2 : 3;
    const int step       = 1 << (shift);
    const int mi_cols_sr = ((pcs->aligned_width + 15) / 16) << 2;
    int       i          = 0;

    for (int idy = 0; idy < mi_height; idy += step) {
        for (int idx = 0; idx < mi_width; idx += step) {
            TplStats* tpl_stats_ptr = pcs->pa_me_data->tpl_stats[(((mi_row + idy) >> shift) * (mi_cols_sr >> shift)) +
                                                                 ((mi_col + idx) >> shift)];

            while (i < frames_in_sw && pcs_array[i]->picture_number != tpl_stats_ptr->ref_frame_poc) {
                i++;
            }
            if (i < frames_in_sw) {
                tpl_model_update_b(pcs_array[i], pcs, tpl_stats_ptr, mi_row + idy, mi_col + idx, block_size);
            }
        }
    }
}

/************************************************
* Genrate TPL MC Flow Synthesizer Based on Lookahead
** LAD Window: sliding window size
************************************************/

void tpl_mc_flow_synthesizer(PictureParentControlSet* pcs_array[MAX_TPL_LA_SW], int32_t frame_idx,
                             uint8_t frames_in_sw) {
    Av1Common*               cm    = pcs_array[frame_idx]->av1_cm;
    const int /*BLOCK_SIZE*/ bsize = pcs_array[frame_idx]->tpl_ctrls.synth_blk_size == 32 ? BLOCK_32X32 : BLOCK_16X16;
    const int                mi_height = mi_size_high[bsize];
    const int                mi_width  = mi_size_wide[bsize];

    for (int mi_row = 0; mi_row < cm->mi_rows; mi_row += mi_height) {
        for (int mi_col = 0; mi_col < cm->mi_cols; mi_col += mi_width) {
            tpl_model_update(pcs_array, frame_idx, mi_row, mi_col, bsize, frames_in_sw);
        }
    }
    return;
}

void svt_aom_generate_r0beta(PictureParentControlSet* pcs) {
    Av1Common*          cm                    = pcs->av1_cm;
    SequenceControlSet* scs                   = pcs->scs;
    int64_t             recrf_dist_base_sum   = 0;
    int64_t             mc_dep_delta_base_sum = 0;
    int64_t             mc_dep_cost_base      = 0;
    const int32_t       shift = pcs->tpl_ctrls.synth_blk_size == 8 ? 1 : pcs->tpl_ctrls.synth_blk_size == 16 ? 2 : 3;
    const int32_t       step  = 1 << (shift);
    const int32_t       col_step_sr = coded_to_superres_mi(step, pcs->superres_denom);
    // Super-res upscaled size should be used here.
    const int32_t mi_cols_sr = ((pcs->enhanced_unscaled_pic->width + 15) / 16) << 2; // picture column boundary
    const int32_t mi_rows    = ((pcs->enhanced_unscaled_pic->height + 15) / 16) << 2; // picture row boundary
    int64_t       count      = 0;
    int64_t       max_dist   = 0;

    for (int row = 0; row < cm->mi_rows; row += step) {
        for (int col = 0; col < mi_cols_sr; col += col_step_sr) {
            TplStats* tpl_stats_ptr =
                pcs->pa_me_data->tpl_stats[(row >> shift) * (mi_cols_sr >> shift) + (col >> shift)];
            int64_t mc_dep_delta = RDCOST(
                pcs->pa_me_data->base_rdmult, tpl_stats_ptr->mc_dep_rate, tpl_stats_ptr->mc_dep_dist);
            recrf_dist_base_sum += tpl_stats_ptr->recrf_dist;
            mc_dep_delta_base_sum += mc_dep_delta;
            count++;
            if (mc_dep_delta > max_dist) {
                max_dist = mc_dep_delta;
            }
        }
    }

    mc_dep_cost_base = (recrf_dist_base_sum << RDDIV_BITS) + mc_dep_delta_base_sum;
    if (mc_dep_cost_base != 0) {
        pcs->r0 = ((double)(recrf_dist_base_sum << (RDDIV_BITS))) / mc_dep_cost_base;
        // If there are outlier blocks responsible for most of the propagation, set r0 to 1.0 to indicate
        // no error propagation, as the result may not be reliable.
        if (max_dist > (mc_dep_delta_base_sum / count) * 100 && max_dist > (mc_dep_delta_base_sum * 9 / 10)) {
            pcs->r0 = 1.0;
        }
        pcs->tpl_is_valid = 1;
    } else {
        pcs->tpl_is_valid = 0;
    }
#if DEBUG_TPL
    SVT_LOG("svt_aom_generate_r0beta ------> poc %ld\t%.0f\t%.5f\tbase_rdmult=%d\n",
            pcs->picture_number,
            (double)mc_dep_cost_base,
            pcs->r0,
            pcs->pa_me_data->base_rdmult);
#endif
    generate_lambda_scaling_factor(pcs, mc_dep_cost_base);

    // If superres scale down is on, should use scaled width instead of full size
    const int32_t  sb_mi_sz          = (int32_t)(scs->sb_size >> 2);
    const uint32_t picture_sb_width  = (uint32_t)((pcs->aligned_width + scs->sb_size - 1) / scs->sb_size);
    const uint32_t picture_sb_height = (uint32_t)((pcs->aligned_height + scs->sb_size - 1) / scs->sb_size);
    const int32_t  mi_high           = sb_mi_sz; // sb size in 4x4 units
    const int32_t  mi_wide           = sb_mi_sz;
    for (uint32_t sb_y = 0; sb_y < picture_sb_height; ++sb_y) {
        for (uint32_t sb_x = 0; sb_x < picture_sb_width; ++sb_x) {
            uint16_t  mi_row           = pcs->sb_geom[sb_y * picture_sb_width + sb_x].org_y >> 2;
            uint16_t  mi_col           = pcs->sb_geom[sb_y * picture_sb_width + sb_x].org_x >> 2;
            int64_t   recrf_dist_sum   = 0;
            int64_t   mc_dep_delta_sum = 0;
            const int mi_col_sr        = coded_to_superres_mi(mi_col, pcs->superres_denom);
            const int mi_col_end_sr    = coded_to_superres_mi(mi_col + mi_wide, pcs->superres_denom);
            const int row_step         = step;

            // loop all mb in the sb
            for (int row = mi_row; row < mi_row + mi_high; row += row_step) {
                for (int col = mi_col_sr; col < mi_col_end_sr; col += col_step_sr) {
                    if (row >= mi_rows || col >= mi_cols_sr) {
                        continue;
                    }

                    int       index         = (row >> shift) * (mi_cols_sr >> shift) + (col >> shift);
                    TplStats* tpl_stats_ptr = pcs->pa_me_data->tpl_stats[index];
                    int64_t   mc_dep_delta  = RDCOST(
                        pcs->pa_me_data->base_rdmult, tpl_stats_ptr->mc_dep_rate, tpl_stats_ptr->mc_dep_dist);
                    recrf_dist_sum += tpl_stats_ptr->recrf_dist;
                    mc_dep_delta_sum += mc_dep_delta;
                }
            }
            double beta = 1.0;
            if (recrf_dist_sum > 0) {
                double rk = ((double)(recrf_dist_sum << (RDDIV_BITS))) /
                    ((recrf_dist_sum << RDDIV_BITS) + mc_dep_delta_sum);
                beta = (pcs->r0 / rk);
                assert(beta > 0.0);
            }
            pcs->pa_me_data->tpl_beta[sb_y * picture_sb_width + sb_x] = beta;
        }
    }
    return;
}

/************************************************
 * Allocate and initialize buffers needed for tpl
 ************************************************/
static EbErrorType init_tpl_buffers(EncodeContext* enc_ctx) {
    for (int frame_idx = 0; frame_idx < MAX_TPL_LA_SW; frame_idx++) {
        enc_ctx->poc_map_idx[frame_idx]                = -1;
        enc_ctx->mc_flow_rec_picture_buffer[frame_idx] = NULL;
    }
    return EB_ErrorNone;
}

/************************************************
* init tpl tpl_disp_segment_ctrl
************************************************/
static void init_tpl_segments(SequenceControlSet* scs, PictureParentControlSet* pcs,
                              PictureParentControlSet** pcs_array, int32_t frames_in_sw) {
    for (int32_t frame_idx = 0; frame_idx < frames_in_sw; frame_idx++) {
        uint32_t enc_dec_seg_col_cnt = scs->tpl_segment_col_count_array;
        uint32_t enc_dec_seg_row_cnt = scs->tpl_segment_row_count_array;

        const int tile_cols       = pcs->av1_cm->tiles_info.tile_cols;
        const int tile_rows       = pcs->av1_cm->tiles_info.tile_rows;
        uint8_t   tile_group_cols = MIN(tile_cols, scs->tile_group_col_count_array);
        uint8_t   tile_group_rows = MIN(tile_rows, scs->tile_group_row_count_array);

        // Valid when only one tile used
        // TPL segments + tiles (not working)
        // TPL segments are 64x64 SB based
        uint16_t pic_width_in_sb;
        uint16_t pic_height_in_sb;
        pic_width_in_sb  = (pcs->aligned_width + scs->b64_size - 1) / scs->b64_size;
        pic_height_in_sb = (pcs->aligned_height + scs->b64_size - 1) / scs->b64_size;

        if (tile_group_cols * tile_group_rows > 1) {
            enc_dec_seg_col_cnt = MIN(enc_dec_seg_col_cnt, (uint8_t)(pic_width_in_sb / tile_group_cols));
            enc_dec_seg_row_cnt = MIN(enc_dec_seg_row_cnt, (uint8_t)(pic_height_in_sb / tile_group_rows));
        }
        // Init segments within the tile group
        int sb_size_log2 = scs->seq_header.sb_size_log2;

        uint8_t tile_group_col_start_tile_idx[1024];
        uint8_t tile_group_row_start_tile_idx[1024];

        // Get the tile start index for tile group
        for (uint8_t c = 0; c <= tile_group_cols; c++) {
            tile_group_col_start_tile_idx[c] = c * tile_cols / tile_group_cols;
        }
        for (uint8_t r = 0; r <= tile_group_rows; r++) {
            tile_group_row_start_tile_idx[r] = r * tile_rows / tile_group_rows;
        }

        for (uint8_t r = 0; r < tile_group_rows; r++) {
            for (uint8_t c = 0; c < tile_group_cols; c++) {
                uint16_t tile_group_idx            = r * tile_group_cols + c;
                uint16_t top_left_tile_col_idx     = tile_group_col_start_tile_idx[c];
                uint16_t top_left_tile_row_idx     = tile_group_row_start_tile_idx[r];
                uint16_t bottom_right_tile_col_idx = tile_group_col_start_tile_idx[c + 1];
                uint16_t bottom_right_tile_row_idx = tile_group_row_start_tile_idx[r + 1];

                TileGroupInfo* tg_info_ptr = &pcs_array[frame_idx]->tile_group_info[tile_group_idx];

                tg_info_ptr->tile_group_tile_start_x = top_left_tile_col_idx;
                tg_info_ptr->tile_group_tile_end_x   = bottom_right_tile_col_idx;

                tg_info_ptr->tile_group_tile_start_y = top_left_tile_row_idx;
                tg_info_ptr->tile_group_tile_end_y   = bottom_right_tile_row_idx;

                tg_info_ptr->tile_group_sb_start_x = pcs->av1_cm->tiles_info.tile_col_start_mi[top_left_tile_col_idx] >>
                    sb_size_log2;
                tg_info_ptr->tile_group_sb_start_y = pcs->av1_cm->tiles_info.tile_row_start_mi[top_left_tile_row_idx] >>
                    sb_size_log2;

                // Get the SB end of the bottom right tile
                tg_info_ptr->tile_group_sb_end_x = pic_width_in_sb;
                //(pcs->av1_cm->tiles_info.tile_col_start_mi[bottom_right_tile_col_idx] >>
                //    sb_size_log2);
                tg_info_ptr->tile_group_sb_end_y = pic_height_in_sb;
                //(pcs->av1_cm->tiles_info.tile_row_start_mi[bottom_right_tile_row_idx] >>
                //    sb_size_log2);

                // Get the width/height of tile group in SB
                tg_info_ptr->tile_group_height_in_sb = tg_info_ptr->tile_group_sb_end_y -
                    tg_info_ptr->tile_group_sb_start_y;
                tg_info_ptr->tile_group_width_in_sb = tg_info_ptr->tile_group_sb_end_x -
                    tg_info_ptr->tile_group_sb_start_x;

                svt_aom_enc_dec_segments_init(pcs_array[frame_idx]->tpl_disp_segment_ctrl[tile_group_idx],
                                              enc_dec_seg_col_cnt,
                                              enc_dec_seg_row_cnt,
                                              tg_info_ptr->tile_group_width_in_sb,
                                              tg_info_ptr->tile_group_height_in_sb);
            }
        }
    }
}

typedef struct TplRefList {
    EbObjectWrapper* ref;
    int32_t          frame_idx;
    uint8_t          refresh_frame_mask;
    bool             is_valid;
} TplRefList;

/************************************************
 * Genrate TPL MC Flow Based on frames in the tpl group
 ************************************************/
static EbErrorType tpl_mc_flow(EncodeContext* enc_ctx, SequenceControlSet* scs, PictureParentControlSet* pcs,
                               SourceBasedOperationsContext* context_ptr) {
    int32_t  frames_in_sw         = MIN(MAX_TPL_LA_SW, pcs->tpl_group_size);
    uint32_t picture_width_in_mb  = (pcs->enhanced_pic->width + 16 - 1) / 16;
    uint32_t picture_height_in_mb = (pcs->enhanced_pic->height + 16 - 1) / 16;

    if (pcs->tpl_ctrls.synth_blk_size == 8) {
        picture_width_in_mb  = picture_width_in_mb << 1;
        picture_height_in_mb = picture_height_in_mb << 1;
    } else if (pcs->tpl_ctrls.synth_blk_size == 32) {
        picture_width_in_mb  = (pcs->enhanced_pic->width + 31) / 32;
        picture_height_in_mb = (pcs->enhanced_pic->height + 31) / 32;
    }
    // wait for PA ME to be done.
    for (uint32_t i = 1; i < pcs->tpl_group_size; i++) {
        svt_wait_cond_var(&pcs->tpl_group[i]->me_ready, 0);
    }
    pcs->tpl_is_valid = 0;
    init_tpl_buffers(enc_ctx);

    TplRefList tpl_ref_list[REF_FRAMES + 1]; // Buffer for each ref pic and current pic
    memset(tpl_ref_list, 0, sizeof(tpl_ref_list[0]) * (REF_FRAMES + 1));

    if (pcs->tpl_group[0]->tpl_data.tpl_temporal_layer_index == 0) {
        // no Tiles path
        if (scs->static_config.tile_rows == 0 && scs->static_config.tile_columns == 0) {
            init_tpl_segments(scs, pcs, pcs->tpl_group, frames_in_sw);
        }

        uint8_t tpl_on;
        enc_ctx->poc_map_idx[0] = pcs->tpl_group[0]->picture_number;
        //TPL main frame loop
        for (int32_t frame_idx = 0; frame_idx < frames_in_sw; frame_idx++) {
            enc_ctx->poc_map_idx[frame_idx] = pcs->tpl_group[frame_idx]->picture_number;
            // NREF need recon buffer for intra pred
            EbObjectWrapper* ref_pic_wrapper;
            // Get Empty Reference Picture Object
            svt_get_empty_object(scs->enc_ctx->tpl_reference_picture_pool_fifo_ptr, &ref_pic_wrapper);
            // if resolution has changed, and the tpl_reference_picture settings do not match scs settings, update tpl reference params
            if (((EbTplReferenceObject*)ref_pic_wrapper->object_ptr)->ref_picture_ptr->max_width !=
                    scs->max_input_luma_width ||
                ((EbTplReferenceObject*)ref_pic_wrapper->object_ptr)->ref_picture_ptr->max_height !=
                    scs->max_input_luma_height) {
                svt_tpl_reference_param_update((EbTplReferenceObject*)ref_pic_wrapper->object_ptr, scs);
            }
            // Give the new Reference a nominal live_count of 1
            svt_object_inc_live_count(ref_pic_wrapper, 1);

            for (int i = 0; i < (REF_FRAMES + 1); i++) {
                // Get empty list entry
                if (!tpl_ref_list[i].is_valid) {
                    tpl_ref_list[i].ref                = ref_pic_wrapper;
                    tpl_ref_list[i].refresh_frame_mask = pcs->tpl_group[frame_idx]->is_ref
                        ? pcs->tpl_group[frame_idx]->av1_ref_signal.refresh_frame_mask
                        : 0;
                    tpl_ref_list[i].frame_idx          = frame_idx;
                    tpl_ref_list[i].is_valid           = true;
                    enc_ctx->mc_flow_rec_picture_buffer[frame_idx] =
                        ((EbTplReferenceObject*)ref_pic_wrapper->object_ptr)->ref_picture_ptr;
                    break;
                }
            }
            for (uint32_t blky = 0; blky < (picture_height_in_mb); blky++) {
                memset(pcs->tpl_group[frame_idx]->pa_me_data->tpl_stats[blky * (picture_width_in_mb)],
                       0,
                       (picture_width_in_mb) * sizeof(TplStats));
            }
            tpl_on = pcs->tpl_valid_pic[frame_idx];
            if (tpl_on) {
                tpl_mc_flow_dispenser(enc_ctx,
                                      scs,
                                      &pcs->tpl_group[frame_idx]->pa_me_data->base_rdmult,
                                      pcs->tpl_group[frame_idx],
                                      frame_idx,
                                      context_ptr);
            }

            if (scs->tpl_lad_mg > 0) {
                if (tpl_on) {
                    pcs->tpl_group[frame_idx]->tpl_src_data_ready = 1;
                }
            }

            // Release references
            for (int i = 0; i < (REF_FRAMES + 1); i++) {
                // Get empty list entry
                if (tpl_ref_list[i].is_valid &&
                    (frame_idx != tpl_ref_list[i].frame_idx || tpl_ref_list[i].refresh_frame_mask == 0)) {
                    tpl_ref_list[i].refresh_frame_mask &= ~(
                        pcs->tpl_group[frame_idx]->av1_ref_signal.refresh_frame_mask);
                    if (tpl_ref_list[i].refresh_frame_mask == 0) {
                        svt_release_object(tpl_ref_list[i].ref);
                        tpl_ref_list[i].ref                                            = NULL;
                        enc_ctx->mc_flow_rec_picture_buffer[tpl_ref_list[i].frame_idx] = NULL;
                        tpl_ref_list[i].frame_idx                                      = -1;
                        tpl_ref_list[i].refresh_frame_mask                             = 0;
                        tpl_ref_list[i].is_valid                                       = false;
                    }
                }
            }
        }

        // synthesizer
        for (int32_t frame_idx = frames_in_sw - 1; frame_idx >= 0; frame_idx--) {
            tpl_on = pcs->tpl_valid_pic[frame_idx];
            if (tpl_on) {
                tpl_mc_flow_synthesizer(pcs->tpl_group, frame_idx, frames_in_sw);
            }
        }
#if DEBUG_TPL

        for (int32_t frame_idx = 0; frame_idx < frames_in_sw; frame_idx++) {
            PictureParentControlSet* pcs_ptr_tmp      = pcs->tpl_group[frame_idx];
            Av1Common*               cm               = pcs->av1_cm;
            int64_t                  intra_cost_base  = 0;
            int64_t                  mc_dep_cost_base = 0;
#if FIX_R2R_TPL_IXX
            const int shift      = pcs->tpl_ctrls.synth_blk_size == 8 ? 1 : pcs->tpl_ctrls.synth_blk_size == 16 ? 2 : 3;
            const int step       = 1 << (shift);
            const int mi_cols_sr = ((pcs->aligned_width + 15) / 16) << 2;
#else
            const int shift      = pcs->tpl_ctrls.synth_blk_size == 8 ? 1 : pcs->tpl_ctrls.synth_blk_size == 16 ? 2 : 3;
            const int step       = 1 << (shift);
            const int mi_cols_sr = ((pcs_ptr_tmp->aligned_width + 15) / 16) << 2;
#endif
            for (int row = 0; row < cm->mi_rows; row += step) {
                for (int col = 0; col < mi_cols_sr; col += step) {
                    TplStats* tpl_stats_ptr =
                        pcs_ptr_tmp->pa_me_data->tpl_stats[(row >> shift) * (mi_cols_sr >> shift) + (col >> shift)];
                    int64_t mc_dep_delta = RDCOST(
                        pcs->pa_me_data->base_rdmult, tpl_stats_ptr->mc_dep_rate, tpl_stats_ptr->mc_dep_dist);
                    intra_cost_base += (tpl_stats_ptr->recrf_dist << RDDIV_BITS);
                    mc_dep_cost_base += (tpl_stats_ptr->recrf_dist << RDDIV_BITS) + mc_dep_delta;
                }
            }

            SVT_LOG(
                "After "
                "mc_flow_synthesizer:\tframe_indx:%d\tdisplayorder:%ld\tIntra:%lld\tmc_dep:%lld "
                "rdmult:%i\n",
                frame_idx,
                pcs_ptr_tmp->picture_number,
                intra_cost_base,
                mc_dep_cost_base,
                pcs->pa_me_data->base_rdmult);
        }
    }
#else
    }
#endif
    // Release un-released tpl references
    for (int i = 0; i < (REF_FRAMES + 1); i++) {
        // Get empty list entry
        if (tpl_ref_list[i].is_valid) {
            svt_release_object(tpl_ref_list[i].ref);
            tpl_ref_list[i].ref                                            = NULL;
            enc_ctx->mc_flow_rec_picture_buffer[tpl_ref_list[i].frame_idx] = NULL;
            tpl_ref_list[i].frame_idx                                      = -1;
            tpl_ref_list[i].refresh_frame_mask                             = 0;
            tpl_ref_list[i].is_valid                                       = false;
        }
    }

    // When super-res recode is actived, don't release pa_ref_objs until final loop is finished
    // Although tpl-la won't be enabled in super-res FIXED or RANDOM mode, here we use the condition to align with that in initial rate control process
    bool release_pa_ref = (scs->static_config.superres_mode <= SUPERRES_RANDOM) ? true : false;
    for (uint32_t i = 0; i < pcs->tpl_group_size; i++) {
        if (release_pa_ref) {
            if (svt_aom_is_incomp_mg_frame(pcs->tpl_group[i])) {
                if (pcs->tpl_group[i]->ext_mg_id == pcs->ext_mg_id + 1) {
                    svt_aom_release_pa_reference_objects(scs, pcs->tpl_group[i]);
                }
            } else {
                if (pcs->tpl_group[i]->ext_mg_id == pcs->ext_mg_id) {
                    svt_aom_release_pa_reference_objects(scs, pcs->tpl_group[i]);
                }
            }
        }
        if (pcs->tpl_group[i]->non_tf_input) {
            EB_DELETE(pcs->tpl_group[i]->non_tf_input);
        }
    }

    return EB_ErrorNone;
}

/*
   TPL dispenser kernel
   process one picture of TPL group
*/

void* svt_aom_tpl_disp_kernel(void* input_ptr) {
    EbThreadContext*     thread_ctx  = (EbThreadContext*)input_ptr;
    TplDispenserContext* context_ptr = (TplDispenserContext*)thread_ctx->priv;
    EbObjectWrapper*     in_results_wrapper_ptr;
    TplDispResults*      in_results_ptr;
    for (;;) {
        // Get Input Full Object
        EB_GET_FULL_OBJECT(context_ptr->tpl_disp_input_fifo_ptr, &in_results_wrapper_ptr);

        in_results_ptr = (TplDispResults*)in_results_wrapper_ptr->object_ptr;

        PictureParentControlSet* pcs = in_results_ptr->pcs;

        SequenceControlSet* scs = (SequenceControlSet*)pcs->scs;

        int32_t frame_idx           = in_results_ptr->frame_index;
        context_ptr->coded_sb_count = 0;

        uint16_t tile_group_width_in_sb = pcs->tile_group_info[0 /*context_ptr->tile_group_index*/] //  1 tile
                                              .tile_group_width_in_sb;
        EncDecSegments* segments_ptr;

        segments_ptr = pcs->tpl_disp_segment_ctrl[0 /*context_ptr->tile_group_index*/]; //  1 tile
        // Segments
        uint16_t segment_index;

        uint8_t  b64_size        = (uint8_t)scs->b64_size;
        uint8_t  sb_size_log2    = (uint8_t)svt_log2f(b64_size);
        uint32_t pic_width_in_sb = (pcs->aligned_width + b64_size - 1) >> sb_size_log2;

        segment_index = 0;
        // no Tiles path
        if (scs->static_config.tile_rows == 0 && scs->static_config.tile_columns == 0) {
            // segments loop
            while (assign_tpl_segments(
                       segments_ptr, &segment_index, in_results_ptr, frame_idx, context_ptr->tpl_disp_fb_fifo_ptr) ==
                   true) {
                uint32_t x_sb_start_index;
                uint32_t y_sb_start_index;
                uint32_t sb_start_index;
                uint32_t sb_segment_count;
                uint32_t sb_segment_index;
                uint32_t segment_row_index;
                uint32_t segment_band_index;
                uint32_t segment_band_size;
                // SB Loop variables
                uint32_t x_sb_index;
                uint32_t y_sb_index;

                x_sb_start_index = segments_ptr->x_start_array[segment_index];
                y_sb_start_index = segments_ptr->y_start_array[segment_index];
                sb_start_index   = y_sb_start_index * tile_group_width_in_sb + x_sb_start_index;
                sb_segment_count = segments_ptr->valid_sb_count_array[segment_index];

                segment_row_index  = segment_index / segments_ptr->segment_band_count;
                segment_band_index = segment_index - segment_row_index * segments_ptr->segment_band_count;
                segment_band_size  = (segments_ptr->sb_band_count * (segment_band_index + 1) +
                                     segments_ptr->segment_band_count - 1) /
                    segments_ptr->segment_band_count;

                for (y_sb_index = y_sb_start_index, sb_segment_index = sb_start_index;
                     sb_segment_index < sb_start_index + sb_segment_count;
                     ++y_sb_index) {
                    for (x_sb_index = x_sb_start_index;
                         x_sb_index < tile_group_width_in_sb && (x_sb_index + y_sb_index < segment_band_size) &&
                         sb_segment_index < sb_start_index + sb_segment_count;
                         ++x_sb_index, ++sb_segment_index) {
                        uint16_t tile_group_y_sb_start =
                            pcs->tile_group_info[0 /*context_ptr->tile_group_index*/] //  1 tile
                                .tile_group_sb_start_y;
                        uint16_t tile_group_x_sb_start =
                            pcs->tile_group_info[0 /*context_ptr->tile_group_index*/] //  1 tile
                                .tile_group_sb_start_x;

                        context_ptr->sb_index = (uint16_t)((y_sb_index + tile_group_y_sb_start) * pic_width_in_sb +
                                                           x_sb_index + tile_group_x_sb_start);

                        // TPL dispenser per SB (64)
                        B64Geom* b64_geom = &scs->b64_geom[context_ptr->sb_index];
                        tpl_mc_flow_dispenser_sb_generic(pcs->scs->enc_ctx,
                                                         scs,
                                                         pcs,
                                                         frame_idx,
                                                         context_ptr->sb_index,
                                                         in_results_ptr->qIndex,
                                                         (b64_geom->width == 64 && b64_geom->height == 64)
                                                             ? pcs->tpl_ctrls.dispenser_search_level
                                                             : 0);
                        context_ptr->coded_sb_count++;
                    }

                    x_sb_start_index = (x_sb_start_index > 0) ? x_sb_start_index - 1 : 0;
                }
            }

            svt_block_on_mutex(pcs->tpl_disp_mutex);
            pcs->tpl_disp_coded_sb_count += (uint32_t)context_ptr->coded_sb_count;
            bool last_sb_flag = (pcs->b64_total_count == pcs->tpl_disp_coded_sb_count);

            svt_release_mutex(pcs->tpl_disp_mutex);
            if (last_sb_flag) {
                svt_post_semaphore(pcs->tpl_disp_done_semaphore);
            }
        } else {
            // Tiles path does not suupport segments
            for (uint32_t sb_index = 0; sb_index < pcs->b64_total_count; ++sb_index) {
                B64Geom* b64_geom = &scs->b64_geom[sb_index];
                tpl_mc_flow_dispenser_sb_generic(
                    pcs->scs->enc_ctx,
                    scs,
                    pcs,
                    frame_idx,
                    sb_index,
                    in_results_ptr->qIndex,
                    (b64_geom->width == 64 && b64_geom->height == 64) ? pcs->tpl_ctrls.dispenser_search_level : 0);
            }
            svt_post_semaphore(pcs->tpl_disp_done_semaphore);
        }
        svt_release_object(in_results_wrapper_ptr);
    }
    return NULL;
}

static void sbo_send_picture_out(SourceBasedOperationsContext* context_ptr, PictureParentControlSet* pcs,
                                 bool superres_recode) {
    EbObjectWrapper* out_results_wrapper;

    // Get Empty Results Object
    svt_get_empty_object(context_ptr->picture_demux_results_output_fifo_ptr, &out_results_wrapper);

    PictureDemuxResults* out_results = (PictureDemuxResults*)out_results_wrapper->object_ptr;
    out_results->pcs_wrapper         = pcs->p_pcs_wrapper_ptr;
    out_results->picture_type        = superres_recode ? EB_PIC_SUPERRES_INPUT : EB_PIC_INPUT;

    // Post the Full Results Object
    svt_post_full_object(out_results_wrapper);
}

// This is used as a reference when computing the source variance for the
//  purposes of activity masking.
// Eventually this should be replaced by custom no-reference routines,
//  which will be faster.
static const uint8_t AV1_VAR_OFFS[MAX_SB_SIZE] = {
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128};

unsigned int svt_aom_get_perpixel_variance(const uint8_t* buf, uint32_t stride, const int block_size) {
    unsigned int            var, sse;
    const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[block_size];
    var                            = fn_ptr->vf(buf, stride, AV1_VAR_OFFS, 0, &sse);
    return ROUND_POWER_OF_TWO(var, eb_num_pels_log2_lookup[block_size]);
}

static void aom_av1_set_mb_ssim_rdmult_scaling(PictureParentControlSet* pcs) {
    Av1Common* cm       = pcs->av1_cm;
    const int  y_stride = pcs->enhanced_pic->stride_y;
    uint8_t*   y_buffer = pcs->enhanced_pic->buffer_y + pcs->enhanced_pic->org_x + pcs->enhanced_pic->org_y * y_stride;

    const int block_size = BLOCK_16X16;

    const int num_mi_w = mi_size_wide[block_size];
    const int num_mi_h = mi_size_high[block_size];
    const int num_cols = (cm->mi_cols + num_mi_w - 1) / num_mi_w;
    const int num_rows = (cm->mi_rows + num_mi_h - 1) / num_mi_h;
    double    log_sum  = 0.0;

    const double factor_a = 67.035434;
    const double factor_b = -0.0021489;
    const double factor_c = 17.492222;
    const bool   do_print = false;
    if (do_print) {
        fprintf(stderr, "16x16 block variance");
    }

    // Loop through each 16x16 block.
    for (int row = 0; row < num_rows; ++row) {
        for (int col = 0; col < num_cols; ++col) {
            double    var = 0.0, num_of_var = 0.0;
            const int index = row * num_cols + col;

            // Loop through each 8x8 block.
            for (int mi_row = row * num_mi_h; mi_row < cm->mi_rows && mi_row < (row + 1) * num_mi_h; mi_row += 2) {
                for (int mi_col = col * num_mi_w; mi_col < cm->mi_cols && mi_col < (col + 1) * num_mi_w; mi_col += 2) {
                    const int row_offset_y = mi_row << 2;
                    const int col_offset_y = mi_col << 2;

                    const uint8_t* buf = y_buffer + row_offset_y * y_stride + col_offset_y;

                    var += svt_aom_get_perpixel_variance(buf, y_stride, BLOCK_8X8);
                    num_of_var += 1.0;
                }
            }
            var = var / num_of_var;

            // Curve fitting with an exponential model on all 16x16 blocks from the
            // midres dataset.
            double var_backup = var;
            var               = factor_a * (1 - exp(factor_b * var)) + factor_c;
            // As per the above computation, var will be in the range of
            // [17.492222, 84.527656], assuming the data type is of infinite
            // precision. The following assert conservatively checks if var is in
            // the range of [17.0, 85.0] to avoid any issues due to the precision of
            // the relevant data type.
            assert(var > 17.0 && var < 85.0);
            pcs->pa_me_data->ssim_rdmult_scaling_factors[index] = var;
            log_sum += log(var);
            if (do_print) {
                if (col == 0) {
                    fprintf(stderr, "\n");
                }
                fprintf(stderr, "%.4f\t", var_backup);
            }
        }
    }
    log_sum = exp(log_sum / (double)(num_rows * num_cols));
    if (do_print) {
        fprintf(stderr, "\nlog_sum %.4f\n", log_sum);
        fprintf(stderr, "16x16 block rdmult scaling factors");
    }
    for (int row = 0; row < num_rows; ++row) {
        for (int col = 0; col < num_cols; ++col) {
            const int index = row * num_cols + col;
            pcs->pa_me_data->ssim_rdmult_scaling_factors[index] /= log_sum;
            if (do_print) {
                if (col == 0) {
                    fprintf(stderr, "\n");
                }
                fprintf(stderr, "%.4f\t", pcs->pa_me_data->ssim_rdmult_scaling_factors[index]);
            }
        }
    }
}

/************************************************
 * Source Based Operations Kernel
 * Source-based operations process involves a number of analysis algorithms
 * to identify spatiotemporal characteristics of the input pictures.
 ************************************************/
void* svt_aom_source_based_operations_kernel(void* input_ptr) {
    EbThreadContext*              thread_ctx  = (EbThreadContext*)input_ptr;
    SourceBasedOperationsContext* context_ptr = (SourceBasedOperationsContext*)thread_ctx->priv;
    EbObjectWrapper*              in_results_wrapper_ptr;

    for (;;) {
        // Get Input Full Object
        EB_GET_FULL_OBJECT(context_ptr->initial_rate_control_results_input_fifo_ptr, &in_results_wrapper_ptr);

        InitialRateControlResults* in_results_ptr = (InitialRateControlResults*)in_results_wrapper_ptr->object_ptr;
        PictureParentControlSet*   pcs            = (PictureParentControlSet*)in_results_ptr->pcs_wrapper->object_ptr;
        SequenceControlSet*        scs            = pcs->scs;
        if (in_results_ptr->superres_recode) {
            sbo_send_picture_out(context_ptr, pcs, true);

            // Release the Input Results
            svt_release_object(in_results_wrapper_ptr);
            continue;
        }

        // Get TPL ME
        if (pcs->tpl_ctrls.enable) {
            // tpl ME can be performed on unscaled frames in super-res q-threshold and auto mode
            if (!pcs->frame_superres_enabled && pcs->temporal_layer_index == 0) {
                tpl_prep_info(pcs);
                tpl_mc_flow(scs->enc_ctx, scs, pcs, context_ptr);
            }
            bool release_pa_ref = (scs->static_config.superres_mode <= SUPERRES_RANDOM) ? true : false;
            // Release Pa Ref if lad_mg is 0 and P slice and not flat struct (not belonging to any TPL group)
            if (release_pa_ref && /*scs->lad_mg == 0 &&*/ pcs->reference_released == 0) {
                svt_aom_release_pa_reference_objects(scs, pcs);
                // printf ("\n PIC \t %d\n",pcs->picture_number);
            }
        }
        /*********************************************Picture-based operations**********************************************************/
        if (scs->static_config.tune == TUNE_SSIM || scs->static_config.tune == TUNE_IQ ||
            scs->static_config.tune == TUNE_MS_SSIM) {
            aom_av1_set_mb_ssim_rdmult_scaling(pcs);
        }
        sbo_send_picture_out(context_ptr, pcs, false);

        // Release the Input Results
        svt_release_object(in_results_wrapper_ptr);
    }
    return NULL;
}
