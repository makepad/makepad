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

#include <stdlib.h>

#include "utility.h"
#include "md_process.h"
#include "lambda_rate_tables.h"
#include "rc_process.h"
#include "enc_mode_config.h"

const uint8_t quantizer_to_qindex[64] = {
    0,   4,   8,   12,  16,  20,  24,  28,  32,  36,  40,  44,  48,  52,  56,  60,  64,  68,  72,  76,  80,  84,
    88,  92,  96,  100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 148, 152, 156, 160, 164, 168, 172,
    176, 180, 184, 188, 192, 196, 200, 204, 208, 212, 216, 220, 224, 228, 232, 236, 240, 244, 249, 255};

const int percents[2][FIXED_QP_OFFSET_COUNT] = {
    {75, 70, 60, 20, 15, 0}, {76, 60, 30, 15, 8, 4} // libaom offsets
};

const uint8_t uni_psy_bias[64] = {
    85, 85, 85, 85, 85,  85,  85,  85,  85,  85,  85,  85,  85,  85,  85,  85,  95,  95,  95,  95,  95, 95,
    95, 95, 95, 95, 95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95,  95, 95,
    95, 95, 95, 95, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100,
};

static void mode_decision_context_dctor(EbPtr p) {
    ModeDecisionContext* obj = (ModeDecisionContext*)p;

    uint32_t block_max_count_sb = obj->init_max_block_cnt;

    // MD palette search
    if (obj->palette_buffer) {
        EB_FREE(obj->palette_buffer);
    }
    if (obj->palette_cand_array) {
        // Free fields in palette_cand_array before freeing palette_cand_array
        for (int cd = 0; cd < MAX_PAL_CAND; cd++) {
            if (obj->palette_cand_array[cd].color_idx_map) {
                EB_FREE_ARRAY(obj->palette_cand_array[cd].color_idx_map);
            }
        }

        EB_FREE_ARRAY(obj->palette_cand_array);
    }
    if (obj->palette_size_array_0) {
        EB_FREE_ARRAY(obj->palette_size_array_0);
    }
    for (CandClass cand_class_it = CAND_CLASS_0; cand_class_it < CAND_CLASS_TOTAL; cand_class_it++) {
        EB_FREE_ARRAY(obj->cand_buff_indices[cand_class_it]);
    }
    EB_FREE_ARRAY(obj->best_candidate_index_array);

    EB_FREE_ARRAY(obj->above_txfm_context);
    EB_FREE_ARRAY(obj->left_txfm_context);
    for (uint32_t coded_leaf_index = 0; coded_leaf_index < block_max_count_sb; ++coded_leaf_index) {
        if (obj->md_blk_arr_nsq[coded_leaf_index].coeff_tmp) {
            EB_DELETE(obj->md_blk_arr_nsq[coded_leaf_index].coeff_tmp);
        }
        if (obj->md_blk_arr_nsq[coded_leaf_index].recon_tmp) {
            EB_DELETE(obj->md_blk_arr_nsq[coded_leaf_index].recon_tmp);
        }
    }
    EB_DELETE_PTR_ARRAY(obj->cand_bf_ptr_array, obj->max_nics_uv);
    EB_FREE_ARRAY(obj->cand_bf_tx_depth_1->cand);
    EB_DELETE(obj->cand_bf_tx_depth_1);
    EB_FREE_ARRAY(obj->cand_bf_tx_depth_2->cand);
    EB_DELETE(obj->cand_bf_tx_depth_2);
    EB_FREE_ALIGNED_ARRAY(obj->cfl_temp_luma_recon16bit);
    EB_FREE_ALIGNED_ARRAY(obj->cfl_temp_luma_recon);
    EB_FREE_ALIGNED_ARRAY(obj->pred_buf_q3);
    EB_FREE_ARRAY(obj->fast_cand_array);
    EB_FREE_2D(obj->injected_mvs);
    EB_FREE_ARRAY(obj->injected_ref_types);
    EB_FREE_ARRAY(obj->fast_cost_array);
    EB_FREE_ARRAY(obj->full_cost_array);
    if (obj->md_blk_arr_nsq) {
        for (int i = 0; i < 3; i++) {
            EB_FREE_ARRAY(obj->md_blk_arr_nsq[0].neigh_left_recon_16bit[i]);
            EB_FREE_ARRAY(obj->md_blk_arr_nsq[0].neigh_top_recon_16bit[i]);
            EB_FREE_ARRAY(obj->md_blk_arr_nsq[0].neigh_left_recon[i]);
            EB_FREE_ARRAY(obj->md_blk_arr_nsq[0].neigh_top_recon[i]);
        }
    }
    if (obj->md_blk_arr_nsq) {
        EB_FREE_ARRAY(obj->md_blk_arr_nsq[0].av1xd);
    }
    EB_FREE_ARRAY(obj->mds);
    EB_FREE_ARRAY(obj->pc_tree);
    EB_FREE_ARRAY(obj->tested_blk);
    obj->blocks_to_alloc = 0;
    EB_FREE_ARRAY(obj->md_blk_arr_nsq);
    if (obj->rate_est_table) {
        EB_FREE_ARRAY(obj->rate_est_table);
    }

    for (int i = 0; i < NEAREST_NEAR_MV_CNT; i++) {
        if (obj->cmp_store.pred0_buf[i]) {
            EB_FREE(obj->cmp_store.pred0_buf[i]);
        }
        if (obj->cmp_store.pred1_buf[i]) {
            EB_FREE(obj->cmp_store.pred1_buf[i]);
        }
    }
    if (obj->residual1) {
        EB_FREE(obj->residual1);
    }
    if (obj->diff10) {
        EB_FREE(obj->diff10);
    }

    if (obj->intrapred_buf) {
        EB_FREE_2D(obj->intrapred_buf);
    }

    if (obj->obmc_buff_0) {
        EB_FREE(obj->obmc_buff_0);
    }
    if (obj->obmc_buff_1) {
        EB_FREE(obj->obmc_buff_1);
    }
    if (obj->wsrc_buf) {
        EB_FREE(obj->wsrc_buf);
    }
    if (obj->mask_buf) {
        EB_FREE(obj->mask_buf);
    }
    for (uint32_t txt_itr = 0; txt_itr < TX_TYPES; ++txt_itr) {
        EB_DELETE(obj->recon_coeff_ptr[txt_itr]);
        EB_DELETE(obj->recon_ptr[txt_itr]);
        EB_DELETE(obj->quant_coeff_ptr[txt_itr]);
    }
    EB_DELETE(obj->tx_coeffs);
    EB_DELETE(obj->scratch_prediction_ptr);
    EB_DELETE(obj->temp_residual);
    EB_DELETE(obj->temp_recon_ptr);
    EB_FREE_ARRAY(obj->full_cost_ssim_array);
}

void svt_aom_set_nics(SequenceControlSet* scs, NicScalingCtrls* scaling_ctrls, uint32_t mds1_count[CAND_CLASS_TOTAL],
                      uint32_t mds2_count[CAND_CLASS_TOTAL], uint32_t mds3_count[CAND_CLASS_TOTAL], uint8_t pic_type,
                      uint32_t qp);

static void setup_mds(SequenceControlSet* scs, MdScan* mds, uint32_t* mds_idx, int index, BlockSize bsize,
                      const int min_sq_size) {
    mds->mds_idx = *mds_idx;
    mds->bsize   = bsize;
    mds->index   = index;

    // If applicable, add split depths
    const BlockGeom* blk_geom = get_blk_geom_mds(scs->blk_geom_mds, *mds_idx);
    const int        sq_size  = block_size_wide[bsize];
    if (sq_size > min_sq_size) {
        const BlockSize subsize             = get_partition_subsize(bsize, PARTITION_SPLIT);
        const int       sq_subsize          = block_size_wide[subsize];
        int             blocks_per_subdepth = (sq_subsize / min_sq_size) * (sq_subsize / min_sq_size);
        int             blocks_to_skip      = 0;

        for (int i = min_sq_size; i <= sq_subsize; i <<= 1, blocks_per_subdepth >>= 2) {
            blocks_to_skip += blocks_per_subdepth;
        }

        *mds_idx += blk_geom->d1_depth_offset;
        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            mds->split[i] = mds + i * blocks_to_skip + 1;
            setup_mds(scs, mds->split[i], mds_idx, i, subsize, min_sq_size);
        }
    } else {
        *mds_idx += blk_geom->ns_depth_offset;
    }
}

static void setup_pc_tree(PC_TREE* pc_tree, bool (*test_blk_array)[PART_S][4], int index, BlockSize bsize,
                          const int min_sq_size) {
    pc_tree->bsize      = bsize;
    pc_tree->index      = index;
    pc_tree->tested_blk = test_blk_array[0];

    // If applicable, add split depths
    const int sq_size = block_size_wide[bsize];
    if (sq_size > min_sq_size) {
        const BlockSize subsize             = get_partition_subsize(bsize, PARTITION_SPLIT);
        const int       sq_subsize          = block_size_wide[subsize];
        int             blocks_per_subdepth = (sq_subsize / min_sq_size) * (sq_subsize / min_sq_size);
        int             blocks_to_skip      = 0;

        for (int i = min_sq_size; i <= sq_subsize; i <<= 1, blocks_per_subdepth >>= 2) {
            blocks_to_skip += blocks_per_subdepth;
        }

        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            pc_tree->split[i]         = pc_tree + i * blocks_to_skip + 1;
            pc_tree->split[i]->parent = pc_tree;
            setup_pc_tree(pc_tree->split[i], test_blk_array + i * blocks_to_skip + 1, i, subsize, min_sq_size);
        }
    }
}

/******************************************************
 * Mode Decision Context Constructor
 ******************************************************/
EbErrorType svt_aom_mode_decision_context_ctor(ModeDecisionContext* ctx, SequenceControlSet* scs,
                                               EbColorFormat color_format, uint8_t sb_size, EncMode enc_mode,
                                               uint16_t max_block_cnt, uint32_t encoder_bit_depth,
                                               EbFifo* mode_decision_configuration_input_fifo_ptr,
                                               EbFifo* mode_decision_output_fifo_ptr, uint8_t enable_hbd_mode_decision,
                                               uint8_t seq_qp_mod) {
#if !TUNE_STILL_IMAGE
    const EbInputResolution input_resolution = scs->input_resolution;
#endif
    const bool allintra = scs->allintra;
#if TUNE_STILL_IMAGE
    const bool rtc_tune = scs->static_config.rtc;
#endif
    uint32_t buffer_index;
    uint32_t cand_index;

    ctx->init_max_block_cnt     = max_block_cnt;
    uint32_t block_max_count_sb = max_block_cnt;

    ctx->sb_size = sb_size;
    (void)color_format;

    ctx->dctor  = mode_decision_context_dctor;
    ctx->hbd_md = enable_hbd_mode_decision;

    // Input/Output System Resource Manager FIFOs
    ctx->mode_decision_configuration_input_fifo_ptr = mode_decision_configuration_input_fifo_ptr;
    ctx->mode_decision_output_fifo_ptr              = mode_decision_output_fifo_ptr;

    // Maximum number of candidates MD can support
    // determine MAX_NICS for a given preset
    // get the min scaling level (the smallest scaling level is the most conservative)
    uint8_t min_nic_scaling_level = NICS_SCALING_LEVELS - 1;
#if TUNE_STILL_IMAGE
    uint8_t stage1_scaling_num;
    if (allintra) {
        uint8_t nic_level  = svt_aom_get_nic_level_allintra(enc_mode);
        stage1_scaling_num = MD_STAGE_NICS_SCAL_NUM[svt_aom_set_nic_controls(NULL, nic_level)][MD_STAGE_1];
    } else if (rtc_tune) {
        uint8_t nic_level  = svt_aom_get_nic_level_rtc(enc_mode, scs->use_flat_ipp);
        stage1_scaling_num = MD_STAGE_NICS_SCAL_NUM[svt_aom_set_nic_controls(NULL, nic_level)][MD_STAGE_1];
    } else {
        for (uint8_t sc_class1 = 0; sc_class1 < 2; sc_class1++) {
            for (uint8_t is_base = 0; is_base < 2; is_base++) {
                uint8_t nic_level         = svt_aom_get_nic_level_default(enc_mode, is_base, sc_class1);
                uint8_t nic_scaling_level = svt_aom_set_nic_controls(NULL, nic_level);
                min_nic_scaling_level     = MIN(min_nic_scaling_level, nic_scaling_level);
            }
        }

        stage1_scaling_num = MD_STAGE_NICS_SCAL_NUM[min_nic_scaling_level][MD_STAGE_1];
    }
#else
    for (uint8_t sc_class1 = 0; sc_class1 < 2; sc_class1++) {
        for (uint8_t rtc_itr = 0; rtc_itr < 2; rtc_itr++) {
            bool rtc_tune = (bool)rtc_itr;
            for (uint8_t is_base = 0; is_base < 2; is_base++) {
                uint8_t nic_level         = svt_aom_get_nic_level(scs, enc_mode, is_base, rtc_tune, sc_class1);
                uint8_t nic_scaling_level = svt_aom_set_nic_controls(NULL, nic_level);
                min_nic_scaling_level     = MIN(min_nic_scaling_level, nic_scaling_level);
            }
        }
    }

    uint8_t stage1_scaling_num = MD_STAGE_NICS_SCAL_NUM[min_nic_scaling_level][MD_STAGE_1];

#endif
    // scale max_nics
    uint32_t max_nics = 0;
    {
        NicScalingCtrls scaling_ctrls;
        scaling_ctrls.stage1_scaling_num = stage1_scaling_num;
        scaling_ctrls.stage2_scaling_num = stage1_scaling_num;
        scaling_ctrls.stage3_scaling_num = stage1_scaling_num;
        uint32_t mds1_count[CAND_CLASS_TOTAL];
        uint32_t mds2_count[CAND_CLASS_TOTAL];
        uint32_t mds3_count[CAND_CLASS_TOTAL];
        for (uint8_t pic_type = 0; pic_type < NICS_PIC_TYPE; pic_type++) {
            for (uint8_t qp = MIN_QP_VALUE; qp <= MAX_QP_VALUE; qp++) {
                svt_aom_set_nics(scs, &scaling_ctrls, mds1_count, mds2_count, mds3_count, pic_type, qp);

                uint32_t nics = 0;
                for (CandClass cidx = CAND_CLASS_0; cidx < CAND_CLASS_TOTAL; cidx++) {
                    nics += mds1_count[cidx];
                }
                max_nics = MAX(max_nics, nics);
            }
        }
    }

    // If independent chroma search is used, need to allocate additional 84 candidate buffers
#if TUNE_STILL_IMAGE
    bool is_chroma_mode_0;
    if (allintra) {
        is_chroma_mode_0 = svt_aom_set_chroma_controls(NULL, svt_aom_get_chroma_level_allintra(enc_mode)) ==
            CHROMA_MODE_0;
    } else if (scs->static_config.rtc) {
        for (uint8_t is_i_slice = 0; is_i_slice < 2; is_i_slice++) {
            is_chroma_mode_0 = svt_aom_set_chroma_controls(NULL, svt_aom_get_chroma_level_rtc(enc_mode, is_i_slice)) ==
                CHROMA_MODE_0;
            if (is_chroma_mode_0) {
                break;
            }
        }
    } else {
        for (uint8_t is_i_slice = 0; is_i_slice < 2; is_i_slice++) {
            is_chroma_mode_0 = svt_aom_set_chroma_controls(
                                   NULL, svt_aom_get_chroma_level_default(enc_mode, is_i_slice)) == CHROMA_MODE_0;
            if (is_chroma_mode_0) {
                break;
            }
        }
    }
#else
    bool is_chroma_mode_0 = false;
    for (uint8_t is_i_slice = 0; is_i_slice < 2; is_i_slice++) {
        is_chroma_mode_0 = svt_aom_set_chroma_controls(
                               NULL, svt_aom_get_chroma_level(enc_mode, is_i_slice, allintra)) == CHROMA_MODE_0;
        if (is_chroma_mode_0) {
            break;
        }
    }
#endif
    const uint8_t ind_uv_cands = is_chroma_mode_0 ? 84 : 0;
    max_nics += CAND_CLASS_TOTAL; //need one extra temp buffer for each fast loop call
    ctx->max_nics    = max_nics;
    ctx->max_nics_uv = max_nics + ind_uv_cands;
    // Cfl scratch memory
    if (ctx->hbd_md > EB_8_BIT_MD) {
        EB_MALLOC_ALIGNED(ctx->cfl_temp_luma_recon16bit, sizeof(uint16_t) * sb_size * sb_size);
    }
    if (ctx->hbd_md != EB_10_BIT_MD) {
        EB_MALLOC_ALIGNED(ctx->cfl_temp_luma_recon, sizeof(uint8_t) * sb_size * sb_size);
    }
    EB_MALLOC_ALIGNED(ctx->pred_buf_q3, CFL_BUF_SQUARE);
    uint8_t use_update_cdf = 0;
#if TUNE_STILL_IMAGE
    if (allintra) {
        use_update_cdf = svt_aom_get_update_cdf_level_allintra(enc_mode);
    } else if (rtc_tune) {
        for (uint8_t sc_class1 = 0; sc_class1 < 2; sc_class1++) {
            for (uint8_t is_islice = 0; is_islice < 2; is_islice++) {
                for (uint8_t is_base = 0; is_base < 2; is_base++) {
                    if (use_update_cdf) {
                        break;
                    }
                    use_update_cdf |= svt_aom_get_update_cdf_level_rtc(enc_mode, is_islice, is_base, sc_class1);
                }
            }
        }
    } else {
        for (uint8_t sc_class1 = 0; sc_class1 < 2; sc_class1++) {
            for (uint8_t is_islice = 0; is_islice < 2; is_islice++) {
                for (uint8_t is_base = 0; is_base < 2; is_base++) {
                    if (use_update_cdf) {
                        break;
                    }
                    use_update_cdf |= svt_aom_get_update_cdf_level_default(enc_mode, is_islice, is_base, sc_class1);
                }
            }
        }
    }
#else
    for (uint8_t sc_class1 = 0; sc_class1 < 2; sc_class1++) {
        for (uint8_t is_islice = 0; is_islice < 2; is_islice++) {
            for (uint8_t is_base = 0; is_base < 2; is_base++) {
                if (use_update_cdf) {
                    break;
                }
                use_update_cdf |= svt_aom_get_update_cdf_level(
                    enc_mode, is_islice, is_base, sc_class1, input_resolution, allintra);
            }
        }
    }
#endif
    if (use_update_cdf) {
        EB_CALLOC_ARRAY(ctx->rate_est_table, 1);
    } else {
        ctx->rate_est_table = NULL;
    }
    // Allocate buffer for inter-inter compound prediction
    if (get_inter_compound_level(enc_mode)) {
        const uint8_t bits = ctx->hbd_md > EB_8_BIT_MD ? 2 : 1;
        for (int i = 0; i < NEAREST_NEAR_MV_CNT; i++) {
            EB_MALLOC(ctx->cmp_store.pred0_buf[i], sb_size * sb_size * bits * sizeof(uint8_t));
            EB_MALLOC(ctx->cmp_store.pred1_buf[i], sb_size * sb_size * bits * sizeof(uint8_t));
        }
        EB_MALLOC(ctx->residual1, sb_size * sb_size * sizeof(ctx->residual1[0]));
        EB_MALLOC(ctx->diff10, sb_size * sb_size * sizeof(ctx->diff10[0]));
    }

    // Allocate buffer for inter-intra prediction
    uint8_t ii_allowed = 0;
    for (uint8_t transition_present = 0; transition_present < 2; transition_present++) {
        if (ii_allowed) {
            break;
        }
        ii_allowed |= svt_aom_get_inter_intra_level(enc_mode, transition_present);
    }
    if (ii_allowed) {
        const uint8_t bits = ctx->hbd_md > EB_8_BIT_MD ? 2 : 1;
        // MAX block size for inter intra is 32x32
        EB_MALLOC_2D(ctx->intrapred_buf, INTERINTRA_MODES, 32 * 32 * bits * sizeof(ctx->intrapred_buf[0][0]));
    }

    // Allocate buffers for obmc prediction
    uint8_t obmc_allowed = 0;
    for (uint8_t is_base = 0; is_base < 2; is_base++) {
        for (uint8_t qp = MIN_QP_VALUE; qp <= MAX_QP_VALUE; qp++) {
            if (obmc_allowed) {
                break;
            }
            obmc_allowed |= svt_aom_get_obmc_level(enc_mode, qp, seq_qp_mod);
        }
    }
    if (obmc_allowed) {
        const uint8_t bits = ctx->hbd_md > EB_8_BIT_MD ? 2 : 1;
        EB_MALLOC(ctx->obmc_buff_0, sb_size * sb_size * bits * MAX_MB_PLANE * sizeof(ctx->obmc_buff_0[0]));
        EB_MALLOC(ctx->obmc_buff_1, sb_size * sb_size * bits * MAX_MB_PLANE * sizeof(ctx->obmc_buff_1[0]));
        EB_MALLOC(ctx->wsrc_buf, sb_size * sb_size * sizeof(ctx->wsrc_buf[0]));
        EB_MALLOC(ctx->mask_buf, sb_size * sb_size * sizeof(ctx->mask_buf[0]));
    }
    EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq, block_max_count_sb);
    // Fast Candidate Array
    uint16_t max_can_count = svt_aom_get_max_can_count(enc_mode) + ind_uv_cands;
    EB_MALLOC_ARRAY(ctx->fast_cand_array, max_can_count);

    for (cand_index = 0; cand_index < max_can_count; ++cand_index) {
        ctx->fast_cand_array[cand_index].palette_info = NULL;
    }
    svt_aom_assert_err(max_can_count > ind_uv_cands, "Max. candidates is too low");
    EB_MALLOC_2D(ctx->injected_mvs, (uint16_t)(max_can_count - ind_uv_cands), 2);
    EB_MALLOC_ARRAY(ctx->injected_ref_types, (max_can_count - ind_uv_cands));

    // Set buffers for MD palette search to NULL; will be init'd at runtime if needed
    ctx->palette_buffer       = NULL;
    ctx->palette_cand_array   = NULL;
    ctx->palette_size_array_0 = NULL;

    // Cost Arrays
    EB_MALLOC_ARRAY(ctx->fast_cost_array, ctx->max_nics_uv);
    EB_MALLOC_ARRAY(ctx->full_cost_array, ctx->max_nics_uv);
    EB_MALLOC_ARRAY(ctx->full_cost_ssim_array, ctx->max_nics_uv);
    // Candidate Buffers
    EB_NEW(ctx->cand_bf_tx_depth_1,
           svt_aom_mode_decision_scratch_cand_bf_ctor,
           sb_size,
           ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT);

    EB_ALLOC_PTR_ARRAY(ctx->cand_bf_tx_depth_1->cand, 1);
    EB_NEW(ctx->cand_bf_tx_depth_2,
           svt_aom_mode_decision_scratch_cand_bf_ctor,
           sb_size,
           ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT);

    EB_ALLOC_PTR_ARRAY(ctx->cand_bf_tx_depth_2->cand, 1);
    for (int i = 0; i < 3; i++) {
        ctx->md_blk_arr_nsq[0].neigh_left_recon[i]       = NULL;
        ctx->md_blk_arr_nsq[0].neigh_top_recon[i]        = NULL;
        ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[i] = NULL;
        ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[i]  = NULL;
    }
    uint32_t coded_leaf_index;
    uint16_t sz = sizeof(uint16_t);
    if (ctx->hbd_md > EB_8_BIT_MD) {
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[0], block_max_count_sb * sb_size * sz);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[0], block_max_count_sb * sb_size * sz);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[1], block_max_count_sb * sb_size * sz >> 1);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[1], block_max_count_sb * sb_size * sz >> 1);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[2], block_max_count_sb * sb_size * sz >> 1);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[2], block_max_count_sb * sb_size * sz >> 1);

        for (coded_leaf_index = 0; coded_leaf_index < block_max_count_sb; ++coded_leaf_index) {
            size_t offset = coded_leaf_index * sb_size * sz;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_left_recon_16bit[0] =
                ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[0] + offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_top_recon_16bit[0] =
                ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[0] + offset;
            offset >>= 1;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_left_recon_16bit[1] =
                ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[1] + offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_top_recon_16bit[1] =
                ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[1] + offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_left_recon_16bit[2] =
                ctx->md_blk_arr_nsq[0].neigh_left_recon_16bit[2] + offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_top_recon_16bit[2] =
                ctx->md_blk_arr_nsq[0].neigh_top_recon_16bit[2] + offset;
        }
    }
    if (ctx->hbd_md != EB_10_BIT_MD) {
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_left_recon[0], block_max_count_sb * sb_size);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_top_recon[0], block_max_count_sb * sb_size);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_left_recon[1], block_max_count_sb * sb_size >> 1);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_top_recon[1], block_max_count_sb * sb_size >> 1);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_left_recon[2], block_max_count_sb * sb_size >> 1);
        EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].neigh_top_recon[2], block_max_count_sb * sb_size >> 1);

        for (coded_leaf_index = 0; coded_leaf_index < block_max_count_sb; ++coded_leaf_index) {
            size_t offset                                             = coded_leaf_index * sb_size;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_left_recon[0] = ctx->md_blk_arr_nsq[0].neigh_left_recon[0] +
                offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_top_recon[0] = ctx->md_blk_arr_nsq[0].neigh_top_recon[0] +
                offset;
            offset >>= 1;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_left_recon[1] = ctx->md_blk_arr_nsq[0].neigh_left_recon[1] +
                offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_top_recon[1] = ctx->md_blk_arr_nsq[0].neigh_top_recon[1] +
                offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_left_recon[2] = ctx->md_blk_arr_nsq[0].neigh_left_recon[2] +
                offset;
            ctx->md_blk_arr_nsq[coded_leaf_index].neigh_top_recon[2] = ctx->md_blk_arr_nsq[0].neigh_top_recon[2] +
                offset;
        }
    }
    ctx->md_blk_arr_nsq[0].av1xd = NULL;
    EB_MALLOC_ARRAY(ctx->md_blk_arr_nsq[0].av1xd, block_max_count_sb);

    // Alloc mds and pc_tree, which are used to track tested blocks in MD
#if TUNE_STILL_IMAGE
    bool disallow_4x4 = allintra ? svt_aom_get_disallow_4x4_allintra(enc_mode)
        : rtc_tune               ? svt_aom_get_disallow_4x4_rtc(enc_mode)
                                 : svt_aom_get_disallow_4x4_default(enc_mode);
    bool disallow_8x8 = allintra ? svt_aom_get_disallow_8x8_allintra()
        : rtc_tune ? svt_aom_get_disallow_8x8_rtc(enc_mode, scs->max_input_luma_width, scs->max_input_luma_height)
                   : svt_aom_get_disallow_8x8_default();
#else
    bool disallow_4x4 = svt_aom_get_disallow_4x4(enc_mode);
    bool disallow_8x8 = svt_aom_get_disallow_8x8(
        enc_mode, allintra, scs->static_config.rtc, scs->max_input_luma_width, scs->max_input_luma_height);
#endif
    uint8_t min_bsize        = disallow_8x8 ? 16 : disallow_4x4 ? 8 : 4;
    int     blocks_per_depth = (sb_size / min_bsize) * (sb_size / min_bsize);
    int     blocks_to_alloc  = 0;

    for (int i = min_bsize; i <= sb_size; i <<= 1, blocks_per_depth >>= 2) {
        blocks_to_alloc += blocks_per_depth;
    }
    EB_CALLOC_ARRAY(ctx->mds, blocks_to_alloc);
    uint32_t mds_idx = 0;
    setup_mds(scs, ctx->mds, &mds_idx, 0, scs->seq_header.sb_size, min_bsize);
    EB_CALLOC_ARRAY(ctx->pc_tree, blocks_to_alloc);
    EB_MALLOC_ARRAY(ctx->tested_blk, blocks_to_alloc);
    setup_pc_tree(ctx->pc_tree, ctx->tested_blk, 0, scs->seq_header.sb_size, min_bsize);
    ctx->blocks_to_alloc = blocks_to_alloc;

#if TUNE_STILL_IMAGE
    bool bypass_encdec = allintra ? svt_aom_get_bypass_encdec_allintra(enc_mode)
        : rtc_tune                ? svt_aom_get_bypass_encdec_rtc(enc_mode, encoder_bit_depth)
                                  : svt_aom_get_bypass_encdec_default(enc_mode, encoder_bit_depth);
#endif
    for (coded_leaf_index = 0; coded_leaf_index < block_max_count_sb; ++coded_leaf_index) {
        ctx->md_blk_arr_nsq[coded_leaf_index].av1xd      = ctx->md_blk_arr_nsq[0].av1xd + coded_leaf_index;
        ctx->md_blk_arr_nsq[coded_leaf_index].segment_id = 0;
        const BlockGeom* blk_geom                        = get_blk_geom_mds(scs->blk_geom_mds, coded_leaf_index);
#if TUNE_STILL_IMAGE
        if (bypass_encdec) {
#else
        if (svt_aom_get_bypass_encdec(enc_mode, encoder_bit_depth)) {
#endif
            EbPictureBufferDescInitData init_data;

            init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
            init_data.max_width          = blk_geom->bwidth;
            init_data.max_height         = blk_geom->bheight;
            init_data.bit_depth          = EB_THIRTYTWO_BIT;
            init_data.color_format       = (blk_geom->bwidth > 4 && blk_geom->bheight > 4)
                      ? EB_YUV420
                      : EB_YUV444; // PW - must have at least 4x4 for chroma coeffs
            init_data.left_padding       = 0;
            init_data.right_padding      = 0;
            init_data.top_padding        = 0;
            init_data.bot_padding        = 0;
            init_data.split_mode         = false;

            EB_NEW(ctx->md_blk_arr_nsq[coded_leaf_index].coeff_tmp, svt_picture_buffer_desc_ctor, (EbPtr)&init_data);

            init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
            init_data.max_width          = blk_geom->bwidth;
            init_data.max_height         = blk_geom->bheight;
            init_data.bit_depth          = ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT;
            ;
            init_data.color_format  = (blk_geom->bwidth > 4 && blk_geom->bheight > 4) ? EB_YUV420 : EB_YUV444;
            init_data.left_padding  = 0;
            init_data.right_padding = 0;
            init_data.top_padding   = 0;
            init_data.bot_padding   = 0;
            init_data.split_mode    = false;

            EB_NEW(ctx->md_blk_arr_nsq[coded_leaf_index].recon_tmp, svt_picture_buffer_desc_ctor, (EbPtr)&init_data);
        } else {
            ctx->md_blk_arr_nsq[coded_leaf_index].coeff_tmp = NULL;
            ctx->md_blk_arr_nsq[coded_leaf_index].recon_tmp = NULL;
        }
    }
    for (CandClass cand_class_it = CAND_CLASS_0; cand_class_it < CAND_CLASS_TOTAL; cand_class_it++) {
        EB_MALLOC_ARRAY(ctx->cand_buff_indices[cand_class_it], ctx->max_nics_uv);
    }

    EB_MALLOC_ARRAY(ctx->best_candidate_index_array, ctx->max_nics_uv);
    EB_MALLOC_ARRAY(ctx->above_txfm_context, (sb_size >> MI_SIZE_LOG2));
    EB_MALLOC_ARRAY(ctx->left_txfm_context, (sb_size >> MI_SIZE_LOG2));
    EbPictureBufferDescInitData thirty_two_width_picture_buffer_desc_init_data;
    EbPictureBufferDescInitData picture_buffer_desc_init_data;

    picture_buffer_desc_init_data.max_width          = sb_size;
    picture_buffer_desc_init_data.max_height         = sb_size;
    picture_buffer_desc_init_data.bit_depth          = ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT;
    picture_buffer_desc_init_data.color_format       = EB_YUV420;
    picture_buffer_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    picture_buffer_desc_init_data.left_padding       = 0;
    picture_buffer_desc_init_data.right_padding      = 0;
    picture_buffer_desc_init_data.top_padding        = 0;
    picture_buffer_desc_init_data.bot_padding        = 0;
    picture_buffer_desc_init_data.split_mode         = false;
    picture_buffer_desc_init_data.is_16bit_pipeline  = false;

    thirty_two_width_picture_buffer_desc_init_data.max_width          = sb_size;
    thirty_two_width_picture_buffer_desc_init_data.max_height         = sb_size;
    thirty_two_width_picture_buffer_desc_init_data.bit_depth          = EB_THIRTYTWO_BIT;
    thirty_two_width_picture_buffer_desc_init_data.color_format       = EB_YUV420;
    thirty_two_width_picture_buffer_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    thirty_two_width_picture_buffer_desc_init_data.left_padding       = 0;
    thirty_two_width_picture_buffer_desc_init_data.right_padding      = 0;
    thirty_two_width_picture_buffer_desc_init_data.top_padding        = 0;
    thirty_two_width_picture_buffer_desc_init_data.bot_padding        = 0;
    thirty_two_width_picture_buffer_desc_init_data.split_mode         = false;
    thirty_two_width_picture_buffer_desc_init_data.is_16bit_pipeline  = false;
    for (uint32_t txt_itr = 0; txt_itr < TX_TYPES; ++txt_itr) {
        EB_NEW(ctx->recon_coeff_ptr[txt_itr],
               svt_picture_buffer_desc_ctor,
               (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);
        EB_NEW(ctx->recon_ptr[txt_itr], svt_picture_buffer_desc_ctor, (EbPtr)&picture_buffer_desc_init_data);
        EB_NEW(ctx->quant_coeff_ptr[txt_itr],
               svt_picture_buffer_desc_ctor,
               (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);
    }
    EB_NEW(ctx->tx_coeffs, svt_picture_buffer_desc_ctor, (EbPtr)&thirty_two_width_picture_buffer_desc_init_data);
    EB_NEW(ctx->scratch_prediction_ptr, svt_picture_buffer_desc_ctor, (EbPtr)&picture_buffer_desc_init_data);
    EbPictureBufferDescInitData double_width_picture_buffer_desc_init_data;
    double_width_picture_buffer_desc_init_data.max_width          = sb_size;
    double_width_picture_buffer_desc_init_data.max_height         = sb_size;
    double_width_picture_buffer_desc_init_data.bit_depth          = EB_SIXTEEN_BIT;
    double_width_picture_buffer_desc_init_data.color_format       = EB_YUV420;
    double_width_picture_buffer_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;
    double_width_picture_buffer_desc_init_data.left_padding       = 0;
    double_width_picture_buffer_desc_init_data.right_padding      = 0;
    double_width_picture_buffer_desc_init_data.top_padding        = 0;
    double_width_picture_buffer_desc_init_data.bot_padding        = 0;
    double_width_picture_buffer_desc_init_data.split_mode         = false;
    double_width_picture_buffer_desc_init_data.is_16bit_pipeline  = false;

    // The temp_recon_ptr and temp_residual will be shared by all candidates
    // If you want to do something with residual or recon, you need to create one
    EB_NEW(ctx->temp_recon_ptr, svt_picture_buffer_desc_ctor, (EbPtr)&picture_buffer_desc_init_data);
    EB_NEW(ctx->temp_residual, svt_picture_buffer_desc_ctor, (EbPtr)&double_width_picture_buffer_desc_init_data);

    // Candidate Buffers
    EB_ALLOC_PTR_ARRAY(ctx->cand_bf_ptr_array, ctx->max_nics_uv);

    for (buffer_index = 0; buffer_index < ctx->max_nics; ++buffer_index) {
        EB_NEW(ctx->cand_bf_ptr_array[buffer_index],
               svt_aom_mode_decision_cand_bf_ctor,
               ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
               sb_size,
               PICTURE_BUFFER_DESC_FULL_MASK,
               ctx->temp_residual,
               ctx->temp_recon_ptr,
               &(ctx->fast_cost_array[buffer_index]),
               &(ctx->full_cost_array[buffer_index]),
               &(ctx->full_cost_ssim_array[buffer_index]));
    }

    for (buffer_index = max_nics; buffer_index < ctx->max_nics_uv; ++buffer_index) {
        EB_NEW(ctx->cand_bf_ptr_array[buffer_index],
               svt_aom_mode_decision_cand_bf_ctor,
               ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT,
               sb_size,
               PICTURE_BUFFER_DESC_CHROMA_MASK,
               ctx->temp_residual,
               ctx->temp_recon_ptr,
               &(ctx->fast_cost_array[buffer_index]),
               &(ctx->full_cost_array[buffer_index]),
               &(ctx->full_cost_ssim_array[buffer_index]));
    }

    return EB_ErrorNone;
}

/**************************************************
 * Reset Mode Decision Neighbor Arrays
 *************************************************/
void svt_aom_reset_mode_decision_neighbor_arrays(PictureControlSet* pcs, uint16_t tile_idx) {
    uint8_t depth;
    for (depth = 0; depth < NA_TOT_CNT; depth++) {
        svt_aom_neighbor_array_unit_reset(pcs->mdleaf_partition_na[depth][tile_idx]);
        if (pcs->hbd_md != EB_10_BIT_MD) {
            svt_aom_neighbor_array_unit_reset(pcs->md_luma_recon_na[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_tx_depth_1_luma_recon_na[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_tx_depth_2_luma_recon_na[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_cb_recon_na[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_cr_recon_na[depth][tile_idx]);
        }
        if (pcs->hbd_md > EB_8_BIT_MD || (pcs->scs->encoder_bit_depth > EB_EIGHT_BIT && pcs->pic_bypass_encdec)) {
            svt_aom_neighbor_array_unit_reset(pcs->md_luma_recon_na_16bit[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_tx_depth_1_luma_recon_na_16bit[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_tx_depth_2_luma_recon_na_16bit[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_cb_recon_na_16bit[depth][tile_idx]);
            svt_aom_neighbor_array_unit_reset(pcs->md_cr_recon_na_16bit[depth][tile_idx]);
        }

        svt_aom_neighbor_array_unit_reset(pcs->md_y_dcs_na[depth][tile_idx]);
        svt_aom_neighbor_array_unit_reset(pcs->md_tx_depth_1_luma_dc_sign_level_coeff_na[depth][tile_idx]);
        svt_aom_neighbor_array_unit_reset(pcs->md_cb_dc_sign_level_coeff_na[depth][tile_idx]);
        svt_aom_neighbor_array_unit_reset(pcs->md_cr_dc_sign_level_coeff_na[depth][tile_idx]);
        svt_aom_neighbor_array_unit_reset(pcs->md_txfm_context_array[depth][tile_idx]);
    }

    return;
}

// If the ref intra percentage is below the TH, applying modulation to the MD lambda
#define LAMBDA_MOD_INTRA_TH 50
#define LAMBDA_MOD_INTRA_SCALING_FACTOR 138

// Set the lambda for each sb.
// When lambda tuning is on (blk_lambda_tuning), lambda of each block is set separately (full_lambda_md/fast_lambda_md)
// later in svt_aom_set_tuned_blk_lambda
// Testing showed that updating SAD lambda based on frame info was not helpful; therefore, the SAD lambda generation is not changed.
static void av1_lambda_assign_md(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    ctx->full_lambda_md[0] = svt_aom_compute_rd_mult(pcs, ctx->qp_index, ctx->me_q_index, EB_EIGHT_BIT);
    ctx->fast_lambda_md[0] = svt_aom_compute_fast_lambda(pcs, ctx->qp_index, ctx->me_q_index, EB_EIGHT_BIT);
    ctx->full_lambda_md[1] = svt_aom_compute_rd_mult(pcs, ctx->qp_index, ctx->me_q_index, EB_TEN_BIT);
    ctx->fast_lambda_md[1] = svt_aom_compute_fast_lambda(pcs, ctx->qp_index, ctx->me_q_index, EB_TEN_BIT);

    if (!pcs->scs->static_config.rtc && pcs->scs->stats_based_sb_lambda_modulation) {
        if (pcs->temporal_layer_index > 0) {
            if (pcs->ref_intra_percentage < LAMBDA_MOD_INTRA_TH) {
                ctx->full_lambda_md[0] = (ctx->full_lambda_md[0] * LAMBDA_MOD_INTRA_SCALING_FACTOR) >> 7;
                ctx->fast_lambda_md[0] = (ctx->fast_lambda_md[0] * LAMBDA_MOD_INTRA_SCALING_FACTOR) >> 7;
                ctx->full_lambda_md[1] = (ctx->full_lambda_md[1] * LAMBDA_MOD_INTRA_SCALING_FACTOR) >> 7;
                ctx->fast_lambda_md[1] = (ctx->fast_lambda_md[1] * LAMBDA_MOD_INTRA_SCALING_FACTOR) >> 7;
            }
        }
    }

    if (pcs->lambda_weight) {
        ctx->full_lambda_md[0] = (uint32_t)((ctx->full_lambda_md[0] * (uint64_t)pcs->lambda_weight) >> 7);
        ctx->fast_lambda_md[0] = (uint32_t)((ctx->fast_lambda_md[0] * (uint64_t)pcs->lambda_weight) >> 7);
        ctx->full_lambda_md[1] = (uint32_t)((ctx->full_lambda_md[1] * (uint64_t)pcs->lambda_weight) >> 7);
        ctx->fast_lambda_md[1] = (uint32_t)((ctx->fast_lambda_md[1] * (uint64_t)pcs->lambda_weight) >> 7);
    }
    ctx->full_lambda_md[1] *= 16;
    ctx->fast_lambda_md[1] *= 4;

    SequenceControlSet* scs          = pcs->scs;
    uint64_t            scale_factor = scs->static_config.lambda_scale_factors[pcs->ppcs->update_type];
    ctx->full_lambda_md[0]           = (uint32_t)((ctx->full_lambda_md[0] * scale_factor) >> 7);
    ctx->full_lambda_md[1]           = (uint32_t)((ctx->full_lambda_md[1] * scale_factor) >> 7);
    ctx->fast_lambda_md[0]           = (uint32_t)((ctx->fast_lambda_md[0] * scale_factor) >> 7);
    ctx->fast_lambda_md[1]           = (uint32_t)((ctx->fast_lambda_md[1] * scale_factor) >> 7);

    ctx->full_sb_lambda_md[0] = ctx->full_lambda_md[0];
    ctx->full_sb_lambda_md[1] = ctx->full_lambda_md[1];
}

void svt_aom_reset_mode_decision(SequenceControlSet* scs, ModeDecisionContext* ctx, PictureControlSet* pcs,
                                 uint16_t tile_group_idx, uint32_t segment_index) {
    const bool rtc_tune = scs->static_config.rtc;
    ctx->hbd_md         = pcs->hbd_md;
    // Reset MD rate Estimation table to initial values by copying from md_rate_est_ctx
    ctx->md_rate_est_ctx = pcs->md_rate_est_ctx;
    // Reset CABAC Contexts

    // Reset Neighbor Arrays at start of new Segment / Picture
    if (segment_index == 0) {
        for (uint16_t r = pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_start_y;
             r < pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_end_y;
             r++) {
            for (uint16_t c = pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_start_x;
                 c < pcs->ppcs->tile_group_info[tile_group_idx].tile_group_tile_end_x;
                 c++) {
                uint16_t tile_idx = c + r * pcs->ppcs->av1_cm->tiles_info.tile_cols;
                svt_aom_reset_mode_decision_neighbor_arrays(pcs, tile_idx);
            }
        }
        (void)scs;
    }
    //each segment enherits the bypass encdec from the picture level
    ctx->bypass_encdec = pcs->pic_bypass_encdec;

    if (!rtc_tune && (pcs->enc_mode <= ENC_M11 || pcs->temporal_layer_index != 0)) {
        ctx->rtc_use_N4_dct_dct_shortcut = 1;
    } else {
        ctx->rtc_use_N4_dct_dct_shortcut = 0;
    }
    return;
}

/******************************************************
 * Mode Decision Configure SB
 ******************************************************/
void svt_aom_mode_decision_configure_sb(ModeDecisionContext* ctx, PictureControlSet* pcs, uint8_t sb_qp,
                                        uint8_t me_sb_qp) {
    /* Note(CHKN) : when Qp modulation varies QP on a sub-SB(CU) basis,  Lamda has to change based on Cu->QP , and then this code has to move inside the CU loop in MD */

    // Lambda Assignement
    ctx->qp_index = pcs->ppcs->frm_hdr.delta_q_params.delta_q_present || pcs->ppcs->r0_delta_qp_md
        ? sb_qp
        : (uint8_t)pcs->ppcs->frm_hdr.quantization_params.base_q_idx;

    ctx->me_q_index = me_sb_qp;

    av1_lambda_assign_md(pcs, ctx);

    ctx->hbd_pack_done = 0;

    return;
}
