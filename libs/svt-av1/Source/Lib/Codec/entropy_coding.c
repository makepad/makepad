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
#include <string.h>
#include <math.h>

#include "EbSvtAv1.h"
#include "entropy_coding.h"
#include "utility.h"
#include "transforms.h"
#include "ec_process.h"
#include "common_utils.h"
#include "adaptive_mv_pred.h"
#include "rd_cost.h"
#include "svt_log.h"
#include "full_loop.h"
#include "aom_dsp_rtcd.h"
#include "inter_prediction.h"
#include "mode_decision.h"
#include "restoration.h"

const uint8_t eob_to_pos_small[33] = {
    0, 1, 2, // 0-2
    3, 3, // 3-4
    4, 4, 4, 4, // 5-8
    5, 5, 5, 5, 5, 5, 5, 5, // 9-16
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6 // 17-32
};

const int16_t eob_group_start[12]         = {0, 1, 2, 3, 5, 9, 17, 33, 65, 129, 257, 513};
const int16_t svt_aom_eob_offset_bits[12] = {0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9};

const uint8_t eob_to_pos_large[17] = {
    6, // place holder
    7, // 33-64
    8,
    8, // 65-128
    9,
    9,
    9,
    9, // 129-256
    10,
    10,
    10,
    10,
    10,
    10,
    10,
    10, // 257-512
    11 // 513-
};

static void mem_put_varsize(uint8_t* const dst, const int sz, const int val) {
    switch (sz) {
    case 1:
        dst[0] = (uint8_t)(val & 0xff);
        break;
    case 2:
        mem_put_le16(dst, val);
        break;
    case 3:
        mem_put_le24(dst, val);
        break;
    case 4:
        mem_put_le32(dst, val);
        break;
    default:
        assert(0 && "Invalid size");
        break;
    }
}

int svt_aom_get_comp_index_context_enc(PictureParentControlSet* pcs, int cur_frame_index, int bck_frame_index,
                                       int fwd_frame_index, const MacroBlockD* xd) {
    const int fwd = abs(svt_aom_get_relative_dist_enc(&pcs->scs->seq_header, fwd_frame_index, cur_frame_index));
    const int bck = abs(svt_aom_get_relative_dist_enc(&pcs->scs->seq_header, cur_frame_index, bck_frame_index));

    const MbModeInfo* const above_mi  = xd->above_mbmi;
    const MbModeInfo* const left_mi   = xd->left_mbmi;
    const int               offset    = fwd == bck;
    int                     above_ctx = 0, left_ctx = 0;

    if (above_mi) {
        if (has_second_ref(&above_mi->block_mi)) {
            above_ctx = above_mi->block_mi.compound_idx;
        } else if (above_mi->block_mi.ref_frame[0] == ALTREF_FRAME) {
            above_ctx = 1;
        }
    }

    if (left_mi) {
        if (has_second_ref(&left_mi->block_mi)) {
            left_ctx = left_mi->block_mi.compound_idx;
        } else if (left_mi->block_mi.ref_frame[0] == ALTREF_FRAME) {
            left_ctx = 1;
        }
    }
    return above_ctx + left_ctx + 3 * offset;
}

int svt_aom_get_comp_group_idx_context_enc(const MacroBlockD* xd) {
    const MbModeInfo* const above_mi  = xd->above_mbmi;
    const MbModeInfo* const left_mi   = xd->left_mbmi;
    int                     above_ctx = 0, left_ctx = 0;
    if (above_mi) {
        if (has_second_ref(&above_mi->block_mi)) {
            above_ctx = above_mi->block_mi.comp_group_idx;
        } else if (above_mi->block_mi.ref_frame[0] == ALTREF_FRAME) {
            above_ctx = 3;
        }
    }
    if (left_mi) {
        if (has_second_ref(&left_mi->block_mi)) {
            left_ctx = left_mi->block_mi.comp_group_idx;
        } else if (left_mi->block_mi.ref_frame[0] == ALTREF_FRAME) {
            left_ctx = 3;
        }
    }
    return AOMMIN(5, above_ctx + left_ctx);
}

static INLINE int32_t does_level_match(int32_t width, int32_t height, double fps, int32_t lvl_width, int32_t lvl_height,
                                       double lvl_fps, int32_t lvl_dim_mult) {
    const int64_t lvl_luma_pels           = (int64_t)lvl_width * lvl_height;
    const double  lvl_display_sample_rate = lvl_luma_pels * lvl_fps;
    const int64_t luma_pels               = (int64_t)width * height;
    const double  display_sample_rate     = luma_pels * fps;
    return luma_pels <= lvl_luma_pels && display_sample_rate <= lvl_display_sample_rate &&
        width <= lvl_width * lvl_dim_mult && height <= lvl_height * lvl_dim_mult;
}

static void set_bitstream_level_tier(SequenceControlSet* scs) {
    // This is a placeholder function that only addresses dimensions
    // and max display sample rates.
    // Need to add checks for max bit rate, max decoded luma sample rate, header
    // rate, etc. that are not covered by this function.

    BitstreamLevel bl = {9, 3};
    if (scs->static_config.level) {
        bl.major = scs->static_config.level / 10;
        bl.minor = scs->static_config.level % 10;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                512,
                                288,
                                30.0,
                                4)) {
        bl.major = 2;
        bl.minor = 0;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                704,
                                396,
                                30.0,
                                4)) {
        bl.major = 2;
        bl.minor = 1;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                1088,
                                612,
                                30.0,
                                4)) {
        bl.major = 3;
        bl.minor = 0;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                1376,
                                774,
                                30.0,
                                4)) {
        bl.major = 3;
        bl.minor = 1;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                2048,
                                1152,
                                30.0,
                                3)) {
        bl.major = 4;
        bl.minor = 0;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                2048,
                                1152,
                                60.0,
                                3)) {
        bl.major = 4;
        bl.minor = 1;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                4096,
                                2176,
                                30.0,
                                2)) {
        bl.major = 5;
        bl.minor = 0;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                4096,
                                2176,
                                60.0,
                                2)) {
        bl.major = 5;
        bl.minor = 1;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                4096,
                                2176,
                                120.0,
                                2)) {
        bl.major = 5;
        bl.minor = 2;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                8192,
                                4352,
                                30.0,
                                2)) {
        bl.major = 6;
        bl.minor = 0;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                8192,
                                4352,
                                60.0,
                                2)) {
        bl.major = 6;
        bl.minor = 1;
    } else if (does_level_match(scs->seq_header.max_frame_width,
                                scs->seq_header.max_frame_height,
                                scs->frame_rate,
                                8192,
                                4352,
                                120.0,
                                2)) {
        bl.major = 6;
        bl.minor = 2;
    }
    for (int32_t i = 0; i < MAX_NUM_OPERATING_POINTS; ++i) {
        scs->level[i]                               = bl;
        scs->seq_header.operating_point[i].seq_tier = 0; // setting main tier by default
    }
}

static void write_golomb(AomWriter* w, int32_t level) {
    const int32_t x = level + 1;
    // while (i) { i >>= 1; ++length; }
    const uint32_t length = svt_log2f(x) + 1;

    assert(length > 0);

    for (uint32_t i = 0; i < length - 1; ++i) {
        aom_write_bit(w, 0);
    }

    for (int32_t i = length - 1; i >= 0; --i) {
        aom_write_bit(w, (x >> i) & 0x01);
    }
}

/************************************************************************************************/
// blockd.h

void svt_aom_get_txb_ctx(PictureControlSet* pcs, const int32_t plane,
                         NeighborArrayUnit* dc_sign_level_coeff_neighbor_array, uint32_t blk_org_x, uint32_t blk_org_y,
                         const BlockSize plane_bsize, const TxSize tx_size, int16_t* const txb_skip_ctx,
                         int16_t* const dc_sign_ctx) {
    uint32_t dc_sign_lvl_coeff_left_neighbor_idx = get_neighbor_array_unit_left_index(
        dc_sign_level_coeff_neighbor_array, blk_org_y);
    uint32_t dc_sign_lvl_coeff_top_neighbor_idx = get_neighbor_array_unit_top_index(dc_sign_level_coeff_neighbor_array,
                                                                                    blk_org_x);

    static const int8_t signs[3] = {0, -1, 1};
    int32_t             txb_w_unit;
    int32_t             txb_h_unit;
    if (plane) {
        txb_w_unit = MIN(eb_tx_size_wide_unit[tx_size], (int32_t)(pcs->ppcs->aligned_width / 2 - blk_org_x) >> 2);
        txb_h_unit = MIN(eb_tx_size_high_unit[tx_size], (int32_t)(pcs->ppcs->aligned_height / 2 - blk_org_y) >> 2);
    } else {
        txb_w_unit = MIN(eb_tx_size_wide_unit[tx_size], (int32_t)(pcs->ppcs->aligned_width - blk_org_x) >> 2);
        txb_h_unit = MIN(eb_tx_size_high_unit[tx_size], (int32_t)(pcs->ppcs->aligned_height - blk_org_y) >> 2);
    }
    int16_t  dc_sign = 0;
    uint16_t k       = 0;

    uint8_t sign;

    if (dc_sign_level_coeff_neighbor_array->top_array[dc_sign_lvl_coeff_top_neighbor_idx] != INVALID_NEIGHBOR_DATA) {
        do {
            sign = ((uint8_t)dc_sign_level_coeff_neighbor_array->top_array[k + dc_sign_lvl_coeff_top_neighbor_idx] >>
                    COEFF_CONTEXT_BITS);
            assert(sign <= 2);
            dc_sign += signs[sign];
        } while (++k < txb_w_unit);
    }

    if (dc_sign_level_coeff_neighbor_array->left_array[dc_sign_lvl_coeff_left_neighbor_idx] != INVALID_NEIGHBOR_DATA) {
        k = 0;
        do {
            sign = ((uint8_t)dc_sign_level_coeff_neighbor_array->left_array[k + dc_sign_lvl_coeff_left_neighbor_idx] >>
                    COEFF_CONTEXT_BITS);
            assert(sign <= 2);
            dc_sign += signs[sign];
        } while (++k < txb_h_unit);
    }

    if (dc_sign > 0) {
        *dc_sign_ctx = 2;
    } else if (dc_sign < 0) {
        *dc_sign_ctx = 1;
    } else {
        *dc_sign_ctx = 0;
    }

    if (plane == 0) {
        if (plane_bsize == txsize_to_bsize[tx_size]) {
            *txb_skip_ctx = 0;
        } else {
            static const uint8_t skip_contexts[5][5] = {
                {1, 2, 2, 2, 3}, {1, 4, 4, 4, 5}, {1, 4, 4, 4, 5}, {1, 4, 4, 4, 5}, {1, 4, 4, 4, 6}};
            int32_t top  = 0;
            int32_t left = 0;

            k = 0;
            if (dc_sign_level_coeff_neighbor_array->top_array[dc_sign_lvl_coeff_top_neighbor_idx] !=
                INVALID_NEIGHBOR_DATA) {
                do {
                    top |= (int32_t)(dc_sign_level_coeff_neighbor_array
                                         ->top_array[k + dc_sign_lvl_coeff_top_neighbor_idx]);
                } while (++k < txb_w_unit);
            }
            top &= COEFF_CONTEXT_MASK;

            if (dc_sign_level_coeff_neighbor_array->left_array[dc_sign_lvl_coeff_left_neighbor_idx] !=
                INVALID_NEIGHBOR_DATA) {
                k = 0;
                do {
                    left |= (int32_t)(dc_sign_level_coeff_neighbor_array
                                          ->left_array[k + dc_sign_lvl_coeff_left_neighbor_idx]);
                } while (++k < txb_h_unit);
            }
            left &= COEFF_CONTEXT_MASK;
            //do {
            //    top |= a[k];
            //} while (++k < txb_w_unit);
            //top &= COEFF_CONTEXT_MASK;

            //k = 0;
            //do {
            //    left |= l[k];
            //} while (++k < txb_h_unit);
            //left &= COEFF_CONTEXT_MASK;
            const int32_t max = AOMMIN(top | left, 4);
            const int32_t min = AOMMIN(AOMMIN(top, left), 4);

            *txb_skip_ctx = skip_contexts[min][max];
        }
    } else {
        //const int32_t ctx_base = get_entropy_context(tx_size, a, l);
        int16_t ctx_base_left = 0;
        int16_t ctx_base_top  = 0;

        if (dc_sign_level_coeff_neighbor_array->top_array[dc_sign_lvl_coeff_top_neighbor_idx] !=
            INVALID_NEIGHBOR_DATA) {
            k = 0;
            do {
                ctx_base_top +=
                    (dc_sign_level_coeff_neighbor_array->top_array[k + dc_sign_lvl_coeff_top_neighbor_idx] != 0);
            } while (++k < txb_w_unit);
        }
        if (dc_sign_level_coeff_neighbor_array->left_array[dc_sign_lvl_coeff_left_neighbor_idx] !=
            INVALID_NEIGHBOR_DATA) {
            k = 0;
            do {
                ctx_base_left +=
                    (dc_sign_level_coeff_neighbor_array->left_array[k + dc_sign_lvl_coeff_left_neighbor_idx] != 0);
            } while (++k < txb_h_unit);
        }
        const int32_t ctx_base   = ((ctx_base_left != 0) + (ctx_base_top != 0));
        const int32_t ctx_offset = (eb_num_pels_log2_lookup[plane_bsize] >
                                    eb_num_pels_log2_lookup[txsize_to_bsize[tx_size]])
            ? 10
            : 7;
        *txb_skip_ctx            = (int16_t)(ctx_base + ctx_offset);
    }
}

static void av1_write_tx_type(PictureParentControlSet* pcs, FRAME_CONTEXT* frame_context, MbModeInfo* mbmi,
                              AomWriter* ec_writer, uint32_t intraDir, TxType tx_type, TxSize tx_size) {
    FrameHeader*  frm_hdr  = &pcs->frm_hdr;
    const int32_t is_inter = mbmi->block_mi.use_intrabc || is_inter_mode(mbmi->block_mi.mode);
    if (get_ext_tx_types(tx_size, is_inter, frm_hdr->reduced_tx_set) > 1 &&
        (frm_hdr->quantization_params.base_q_idx > 0)) {
        const TxSize square_tx_size = txsize_sqr_map[tx_size];
        assert(square_tx_size <= EXT_TX_SIZES);

        const TxSetType tx_set_type = get_ext_tx_set_type(tx_size, is_inter, frm_hdr->reduced_tx_set);
        const int32_t   eset        = get_ext_tx_set(tx_size, is_inter, frm_hdr->reduced_tx_set);
        // eset == 0 should correspond to a set with only DCT_DCT and there
        // is no need to send the tx_type
        assert(eset > 0);
        assert(av1_ext_tx_used[tx_set_type][tx_type]);
        if (is_inter) {
            aom_write_symbol(ec_writer,
                             av1_ext_tx_ind[tx_set_type][tx_type],
                             frame_context->inter_ext_tx_cdf[eset][square_tx_size],
                             av1_num_ext_tx_set[tx_set_type]);
        } else {
            PredictionMode intra_dir;
            if (mbmi->block_mi.filter_intra_mode != FILTER_INTRA_MODES) {
                intra_dir = fimode_to_intradir[mbmi->block_mi.filter_intra_mode];
            } else {
                intra_dir = intraDir;
            }

            assert(intra_dir < 13);
            assert(square_tx_size < 4);
            aom_write_symbol(ec_writer,
                             av1_ext_tx_ind[tx_set_type][tx_type],
                             frame_context->intra_ext_tx_cdf[eset][square_tx_size][intra_dir],
                             av1_num_ext_tx_set[tx_set_type]);
        }
    }
}

static int32_t av1_write_coeffs_txb_1d(PictureParentControlSet* ppcs, FRAME_CONTEXT* frame_context, MbModeInfo* mbmi,
                                       AomWriter* ec_writer, EcBlkStruct* blk_ptr, TxSize tx_size, uint32_t pu_index,
                                       uint32_t txb_index, uint32_t intraLumaDir, int32_t* coeff_buffer_ptr,
                                       const uint16_t coeff_stride, COMPONENT_TYPE component_type, int16_t txb_skip_ctx,
                                       int16_t dc_sign_ctx, int16_t eob) {
    (void)pu_index;
    (void)coeff_stride;
    const TxSize txs_ctx = get_txsize_entropy_ctx(tx_size);
    TxType       tx_type = component_type == COMPONENT_LUMA ? blk_ptr->tx_type[txb_index] : blk_ptr->tx_type_uv;
    const ScanOrder* const scan_order = get_scan_order(tx_size, tx_type);
    const int16_t* const   scan       = scan_order->scan;
    int32_t                c;
    const int16_t          bwl    = (const uint16_t)get_txb_bwl(tx_size);
    const uint16_t         width  = (const uint16_t)get_txb_wide(tx_size);
    const uint16_t         height = (const uint16_t)get_txb_high(tx_size);

    uint8_t        levels_buf[TX_PAD_2D];
    uint8_t* const levels = set_levels(levels_buf, width);
    DECLARE_ALIGNED(16, int8_t, coeff_contexts[MAX_TX_SQUARE]);

    assert(txs_ctx < TX_SIZES);

    aom_write_symbol(ec_writer, eob == 0, frame_context->txb_skip_cdf[txs_ctx][txb_skip_ctx], 2);

    assert(IMPLIES((component_type == 0 && eob == 0), tx_type == DCT_DCT));
    assert(IMPLIES((is_inter_mode(mbmi->block_mi.mode) && component_type == 0 && eob == 0 && txb_index == 0),
                   blk_ptr->tx_type_uv == DCT_DCT));
    if (eob == 0) {
        return 0;
    }
    svt_av1_txb_init_levels(coeff_buffer_ptr, width, height, levels);
    if (component_type == COMPONENT_LUMA) {
        av1_write_tx_type(ppcs, frame_context, mbmi, ec_writer, intraLumaDir, tx_type, tx_size);
    }
    int       eob_extra;
    const int eob_pt         = get_eob_pos_token(eob, &eob_extra);
    const int eob_multi_size = txsize_log2_minus4[tx_size];
    const int eob_multi_ctx  = (tx_type_to_class[tx_type] == TX_CLASS_2D) ? 0 : 1;
    switch (eob_multi_size) {
    case 0:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf16[component_type][eob_multi_ctx], 5);
        break;
    case 1:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf32[component_type][eob_multi_ctx], 6);
        break;
    case 2:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf64[component_type][eob_multi_ctx], 7);
        break;
    case 3:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf128[component_type][eob_multi_ctx], 8);
        break;
    case 4:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf256[component_type][eob_multi_ctx], 9);
        break;
    case 5:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf512[component_type][eob_multi_ctx], 10);
        break;
    default:
        aom_write_symbol(ec_writer, eob_pt - 1, frame_context->eob_flag_cdf1024[component_type][eob_multi_ctx], 11);
        break;
    }
    const int eob_offset_bits = svt_aom_eob_offset_bits[eob_pt];
    if (eob_offset_bits > 0) {
        const int eob_ctx   = eob_pt - 3;
        int       eob_shift = eob_offset_bits - 1;
        int       bit       = (eob_extra & (1 << eob_shift)) ? 1 : 0;
        aom_write_symbol(ec_writer, bit, frame_context->eob_extra_cdf[txs_ctx][component_type][eob_ctx], 2);
        for (int i = 1; i < eob_offset_bits; i++) {
            eob_shift = eob_offset_bits - 1 - i;
            bit       = (eob_extra & (1 << eob_shift)) ? 1 : 0;
            aom_write_bit(ec_writer, bit);
        }
    }

    svt_av1_get_nz_map_contexts(levels, scan, eob, tx_size, tx_type_to_class[tx_type], coeff_contexts);

    for (c = eob - 1; c >= 0; --c) {
        const int16_t pos       = scan[c];
        const int32_t v         = coeff_buffer_ptr[pos];
        const int16_t coeff_ctx = coeff_contexts[pos];
        int32_t       level     = ABS(v);

        if (c == eob - 1) {
            aom_write_symbol(ec_writer,
                             AOMMIN(level, 3) - 1,
                             frame_context->coeff_base_eob_cdf[txs_ctx][component_type][coeff_ctx],
                             3);
        } else {
            aom_write_symbol(
                ec_writer, AOMMIN(level, 3), frame_context->coeff_base_cdf[txs_ctx][component_type][coeff_ctx], 4);
        }
        if (level > NUM_BASE_LEVELS) {
            // level is above 1.
            int32_t base_range = level - 1 - NUM_BASE_LEVELS;
            int16_t br_ctx     = get_br_ctx(levels, pos, bwl, tx_type_to_class[tx_type]);
            for (int32_t idx = 0; idx < COEFF_BASE_RANGE; idx += BR_CDF_SIZE - 1) {
                const int32_t k = AOMMIN(base_range - idx, BR_CDF_SIZE - 1);
                aom_write_symbol(ec_writer,
                                 k,
                                 frame_context->coeff_br_cdf[AOMMIN(txs_ctx, TX_32X32)][component_type][br_ctx],
                                 BR_CDF_SIZE);
                if (k < BR_CDF_SIZE - 1) {
                    break;
                }
            }
        }
    }
    // Loop to code all signs in the transform block,
    // starting with the sign of DC (if applicable)

    int32_t cul_level = 0;
    for (c = 0; c < eob; ++c) {
        const int16_t pos   = scan[c];
        const int32_t v     = coeff_buffer_ptr[pos];
        int32_t       level = ABS(v);
        cul_level += level;

        const int32_t sign = (v < 0) ? 1 : 0;
        if (level) {
            if (c == 0) {
                aom_write_symbol(ec_writer, sign, frame_context->dc_sign_cdf[component_type][dc_sign_ctx], 2);
            } else {
                aom_write_bit(ec_writer, sign);
            }
            if (level > COEFF_BASE_RANGE + NUM_BASE_LEVELS) {
                write_golomb(ec_writer, level - COEFF_BASE_RANGE - 1 - NUM_BASE_LEVELS);
            }
        }
    }

    cul_level = AOMMIN(COEFF_CONTEXT_MASK, cul_level);
    // DC value
    set_dc_sign(&cul_level, coeff_buffer_ptr[0]);
    return cul_level;
}

static EbErrorType av1_encode_tx_coef_y(PictureControlSet* pcs, EntropyCodingContext* ec_ctx,
                                        FRAME_CONTEXT* frame_context, AomWriter* ec_writer, MbModeInfo* mbmi,
                                        EcBlkStruct* blk_ptr, uint32_t blk_org_x, uint32_t blk_org_y,
                                        uint32_t intraLumaDir, BlockSize plane_bsize, EbPictureBufferDesc* coeff_ptr,
                                        NeighborArrayUnit* luma_dc_sign_level_coeff_na) {
    EbErrorType     return_error = EB_ErrorNone;
    bool            is_inter     = is_inter_mode(mbmi->block_mi.mode) || mbmi->block_mi.use_intrabc;
    const BlockSize bsize        = mbmi->bsize;
    const uint8_t   tx_depth     = mbmi->block_mi.tx_depth;
    const uint16_t  txb_count    = tx_blocks_per_depth[bsize][tx_depth];
    const TxSize    tx_size      = tx_depth_to_tx_size[tx_depth][bsize];
    const int       tx_width     = tx_size_wide[tx_size];
    const int       tx_height    = tx_size_high[tx_size];

    for (uint16_t tx_index = 0; tx_index < txb_count; tx_index++) {
        uint16_t txb_itr = tx_index;

        const uint32_t coeff1d_offset = ec_ctx->coded_area_sb;

        int32_t* coeff_buffer = (int32_t*)coeff_ptr->buffer_y + coeff1d_offset;

        int16_t txb_skip_ctx = 0;
        int16_t dc_sign_ctx  = 0;

        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_LUMA,
                            luma_dc_sign_level_coeff_na,
                            blk_org_x + tx_org[bsize][is_inter][tx_depth][txb_itr].x,
                            blk_org_y + tx_org[bsize][is_inter][tx_depth][txb_itr].y,
                            plane_bsize,
                            tx_size,
                            &txb_skip_ctx,
                            &dc_sign_ctx);

        int32_t cul_level_y = av1_write_coeffs_txb_1d(pcs->ppcs,
                                                      frame_context,
                                                      mbmi,
                                                      ec_writer,
                                                      blk_ptr,
                                                      tx_size,
                                                      0,
                                                      txb_itr,
                                                      intraLumaDir,
                                                      coeff_buffer,
                                                      coeff_ptr->stride_y,
                                                      COMPONENT_LUMA,
                                                      txb_skip_ctx,
                                                      dc_sign_ctx,
                                                      blk_ptr->eob.y[txb_itr]);

        // Update the luma Dc Sign Level Coeff Neighbor Array
        uint8_t dc_sign_level_coeff = (uint8_t)cul_level_y;
        svt_aom_neighbor_array_unit_mode_write(luma_dc_sign_level_coeff_na,
                                               (uint8_t*)&dc_sign_level_coeff,
                                               blk_org_x + tx_org[bsize][is_inter][tx_depth][txb_itr].x,
                                               blk_org_y + tx_org[bsize][is_inter][tx_depth][txb_itr].y,
                                               tx_width,
                                               tx_height,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

        ec_ctx->coded_area_sb += tx_width * tx_height;
    }

    return return_error;
}

static void av1_encode_tx_coef_uv(PictureControlSet* pcs, EntropyCodingContext* ec_ctx, FRAME_CONTEXT* frame_context,
                                  AomWriter* ec_writer, EcBlkStruct* blk_ptr, uint32_t blk_org_x, uint32_t blk_org_y,
                                  uint32_t intraLumaDir, EbPictureBufferDesc* coeff_ptr,
                                  NeighborArrayUnit* cr_dc_sign_level_coeff_na,
                                  NeighborArrayUnit* cb_dc_sign_level_coeff_na) {
    const bool has_uv = is_chroma_reference(blk_org_y >> 2, blk_org_x >> 2, ec_ctx->mbmi->bsize, 1, 1);

    if (!has_uv) {
        return;
    }
    const int32_t   is_inter       = is_inter_mode(ec_ctx->mbmi->block_mi.mode) || ec_ctx->mbmi->block_mi.use_intrabc;
    const BlockSize bsize          = ec_ctx->mbmi->bsize;
    const BlockSize bsize_uv       = get_plane_block_size(bsize, 1, 1);
    const uint8_t   tx_depth       = ec_ctx->mbmi->block_mi.tx_depth;
    const TxSize    chroma_tx_size = av1_get_max_uv_txsize(ec_ctx->mbmi->bsize, 1, 1);
    const int       tx_width_uv    = tx_size_wide[chroma_tx_size];
    const int       tx_height_uv   = tx_size_high[chroma_tx_size];
    unsigned        txb_count      = 1;

    for (unsigned tx_index = 0; tx_index < txb_count; ++tx_index) {
        // cb
        int32_t* coeff_buffer = (int32_t*)coeff_ptr->buffer_cb + ec_ctx->coded_area_sb_uv;
        int16_t  txb_skip_ctx = 0;
        int16_t  dc_sign_ctx  = 0;

        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_CHROMA,
                            cb_dc_sign_level_coeff_na,
                            ROUND_UV(blk_org_x + tx_org[bsize][is_inter][tx_depth][tx_index].x) >> 1,
                            ROUND_UV(blk_org_y + tx_org[bsize][is_inter][tx_depth][tx_index].y) >> 1,
                            bsize_uv,
                            chroma_tx_size,
                            &txb_skip_ctx,
                            &dc_sign_ctx);

        int32_t cul_level_cb = av1_write_coeffs_txb_1d(pcs->ppcs,
                                                       frame_context,
                                                       ec_ctx->mbmi,
                                                       ec_writer,
                                                       blk_ptr,
                                                       chroma_tx_size,
                                                       0,
                                                       tx_index,
                                                       intraLumaDir,
                                                       coeff_buffer,
                                                       coeff_ptr->stride_cb,
                                                       COMPONENT_CHROMA,
                                                       txb_skip_ctx,
                                                       dc_sign_ctx,
                                                       blk_ptr->eob.u[tx_index]);

        // cr
        coeff_buffer = (int32_t*)coeff_ptr->buffer_cr + ec_ctx->coded_area_sb_uv;
        txb_skip_ctx = 0;
        dc_sign_ctx  = 0;

        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_CHROMA,
                            cr_dc_sign_level_coeff_na,
                            ROUND_UV(blk_org_x + tx_org[bsize][is_inter][tx_depth][tx_index].x) >> 1,
                            ROUND_UV(blk_org_y + tx_org[bsize][is_inter][tx_depth][tx_index].y) >> 1,
                            bsize_uv,
                            chroma_tx_size,
                            &txb_skip_ctx,
                            &dc_sign_ctx);

        int32_t cul_level_cr = av1_write_coeffs_txb_1d(pcs->ppcs,
                                                       frame_context,
                                                       ec_ctx->mbmi,
                                                       ec_writer,
                                                       blk_ptr,
                                                       chroma_tx_size,
                                                       0,
                                                       tx_index,
                                                       intraLumaDir,
                                                       coeff_buffer,
                                                       coeff_ptr->stride_cr,
                                                       COMPONENT_CHROMA,
                                                       txb_skip_ctx,
                                                       dc_sign_ctx,
                                                       blk_ptr->eob.v[tx_index]);
        // Update the cb Dc Sign Level Coeff Neighbor Array
        uint8_t dc_sign_level_coeff = (uint8_t)cul_level_cb;
        svt_aom_neighbor_array_unit_mode_write(cb_dc_sign_level_coeff_na,
                                               &dc_sign_level_coeff,
                                               ROUND_UV(blk_org_x + tx_org[bsize][is_inter][tx_depth][tx_index].x) >> 1,
                                               ROUND_UV(blk_org_y + tx_org[bsize][is_inter][tx_depth][tx_index].y) >> 1,
                                               tx_width_uv,
                                               tx_height_uv,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        // Update the cr DC Sign Level Coeff Neighbor Array
        dc_sign_level_coeff = (uint8_t)cul_level_cr;
        svt_aom_neighbor_array_unit_mode_write(cr_dc_sign_level_coeff_na,
                                               &dc_sign_level_coeff,
                                               ROUND_UV(blk_org_x + tx_org[bsize][is_inter][tx_depth][tx_index].x) >> 1,
                                               ROUND_UV(blk_org_y + tx_org[bsize][is_inter][tx_depth][tx_index].y) >> 1,
                                               tx_width_uv,
                                               tx_height_uv,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

        ec_ctx->coded_area_sb_uv += tx_width_uv * tx_height_uv;
    }
}

/************************************
******* Av1EncodeTuCoeff
**************************************/
static EbErrorType av1_encode_coeff_1d(PictureControlSet* pcs, EntropyCodingContext* ec_ctx,
                                       FRAME_CONTEXT* frame_context, AomWriter* ec_writer, EcBlkStruct* blk_ptr,
                                       uint32_t blk_org_x, uint32_t blk_org_y, uint32_t intraLumaDir,
                                       BlockSize luma_bsize, EbPictureBufferDesc* coeff_ptr,
                                       NeighborArrayUnit* luma_dc_sign_level_coeff_na,
                                       NeighborArrayUnit* cr_dc_sign_level_coeff_na,
                                       NeighborArrayUnit* cb_dc_sign_level_coeff_na) {
    EbErrorType return_error = EB_ErrorNone;
    int32_t     is_inter     = is_inter_mode(ec_ctx->mbmi->block_mi.mode) || ec_ctx->mbmi->block_mi.use_intrabc;
    if (ec_ctx->mbmi->block_mi.tx_depth) {
        av1_encode_tx_coef_y(pcs,
                             ec_ctx,
                             frame_context,
                             ec_writer,
                             ec_ctx->mbmi,
                             blk_ptr,
                             blk_org_x,
                             blk_org_y,
                             intraLumaDir,
                             luma_bsize,
                             coeff_ptr,
                             luma_dc_sign_level_coeff_na);

        av1_encode_tx_coef_uv(pcs,
                              ec_ctx,
                              frame_context,
                              ec_writer,
                              blk_ptr,
                              blk_org_x,
                              blk_org_y,
                              intraLumaDir,
                              coeff_ptr,
                              cr_dc_sign_level_coeff_na,
                              cb_dc_sign_level_coeff_na);
    } else {
        // Transform partitioning free path (except the 128x128 case)
        int32_t cul_level_y, cul_level_cb = 0, cul_level_cr = 0;

        const bool     has_uv       = is_chroma_reference(blk_org_y >> 2, blk_org_x >> 2, luma_bsize, 1, 1);
        const uint8_t  tx_depth     = ec_ctx->mbmi->block_mi.tx_depth;
        const uint16_t txb_count    = tx_blocks_per_depth[luma_bsize][tx_depth];
        const TxSize   tx_size      = tx_depth_to_tx_size[tx_depth][luma_bsize];
        const int      tx_width     = tx_size_wide[tx_size];
        const int      tx_height    = tx_size_high[tx_size];
        const TxSize   tx_size_uv   = av1_get_max_uv_txsize(luma_bsize, 1, 1);
        const int      tx_width_uv  = tx_size_wide[tx_size_uv];
        const int      tx_height_uv = tx_size_high[tx_size_uv];
        for (uint8_t txb_itr = 0; txb_itr < txb_count; txb_itr++) {
            int32_t* coeff_buffer;

            const uint32_t coeff1d_offset = ec_ctx->coded_area_sb;

            coeff_buffer = (int32_t*)coeff_ptr->buffer_y + coeff1d_offset;

            {
                int16_t txb_skip_ctx = 0;
                int16_t dc_sign_ctx  = 0;

                svt_aom_get_txb_ctx(pcs,
                                    COMPONENT_LUMA,
                                    luma_dc_sign_level_coeff_na,
                                    blk_org_x + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].x,
                                    blk_org_y + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].y,
                                    luma_bsize,
                                    tx_size,
                                    &txb_skip_ctx,
                                    &dc_sign_ctx);

                cul_level_y = av1_write_coeffs_txb_1d(pcs->ppcs,
                                                      frame_context,
                                                      ec_ctx->mbmi,
                                                      ec_writer,
                                                      blk_ptr,
                                                      tx_size,
                                                      0,
                                                      txb_itr,
                                                      intraLumaDir,
                                                      coeff_buffer,
                                                      coeff_ptr->stride_y,
                                                      COMPONENT_LUMA,
                                                      txb_skip_ctx,
                                                      dc_sign_ctx,
                                                      blk_ptr->eob.y[txb_itr]);
            }

            if (has_uv) {
                const BlockSize bsize_uv = get_plane_block_size(luma_bsize, 1, 1);
                // cb
                coeff_buffer = (int32_t*)coeff_ptr->buffer_cb + ec_ctx->coded_area_sb_uv;
                {
                    int16_t txb_skip_ctx = 0;
                    int16_t dc_sign_ctx  = 0;

                    svt_aom_get_txb_ctx(pcs,
                                        COMPONENT_CHROMA,
                                        cb_dc_sign_level_coeff_na,
                                        ROUND_UV(blk_org_x + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].x) >> 1,
                                        ROUND_UV(blk_org_y + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].y) >> 1,
                                        bsize_uv,
                                        tx_size_uv,
                                        &txb_skip_ctx,
                                        &dc_sign_ctx);

                    cul_level_cb = av1_write_coeffs_txb_1d(pcs->ppcs,
                                                           frame_context,
                                                           ec_ctx->mbmi,
                                                           ec_writer,
                                                           blk_ptr,
                                                           tx_size_uv,
                                                           0,
                                                           txb_itr,
                                                           intraLumaDir,
                                                           coeff_buffer,
                                                           coeff_ptr->stride_cb,
                                                           COMPONENT_CHROMA,
                                                           txb_skip_ctx,
                                                           dc_sign_ctx,
                                                           blk_ptr->eob.u[txb_itr]);
                }

                // cr
                coeff_buffer = (int32_t*)coeff_ptr->buffer_cr + ec_ctx->coded_area_sb_uv;
                {
                    int16_t txb_skip_ctx = 0;
                    int16_t dc_sign_ctx  = 0;

                    svt_aom_get_txb_ctx(pcs,
                                        COMPONENT_CHROMA,
                                        cr_dc_sign_level_coeff_na,
                                        ROUND_UV(blk_org_x + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].x) >> 1,
                                        ROUND_UV(blk_org_y + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].y) >> 1,
                                        bsize_uv,
                                        tx_size_uv,
                                        &txb_skip_ctx,
                                        &dc_sign_ctx);

                    cul_level_cr = av1_write_coeffs_txb_1d(pcs->ppcs,
                                                           frame_context,
                                                           ec_ctx->mbmi,
                                                           ec_writer,
                                                           blk_ptr,
                                                           tx_size_uv,
                                                           0,
                                                           txb_itr,
                                                           intraLumaDir,
                                                           coeff_buffer,
                                                           coeff_ptr->stride_cr,
                                                           COMPONENT_CHROMA,
                                                           txb_skip_ctx,
                                                           dc_sign_ctx,
                                                           blk_ptr->eob.v[txb_itr]);
                }
            }

            // Update the luma Dc Sign Level Coeff Neighbor Array
            uint8_t dc_sign_level_coeff = (uint8_t)cul_level_y;
            svt_aom_neighbor_array_unit_mode_write(luma_dc_sign_level_coeff_na,
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   blk_org_x + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].x,
                                                   blk_org_y + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].y,
                                                   tx_width,
                                                   tx_height,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

            // Update the cb Dc Sign Level Coeff Neighbor Array
            if (has_uv) {
                dc_sign_level_coeff = (uint8_t)cul_level_cb;
                svt_aom_neighbor_array_unit_mode_write(
                    cb_dc_sign_level_coeff_na,
                    &dc_sign_level_coeff,
                    ROUND_UV(blk_org_x + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].x) >> 1,
                    ROUND_UV(blk_org_y + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].y) >> 1,
                    tx_width_uv,
                    tx_height_uv,
                    NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
                // Update the cr DC Sign Level Coeff Neighbor Array
                dc_sign_level_coeff = (uint8_t)cul_level_cr;
                svt_aom_neighbor_array_unit_mode_write(
                    cr_dc_sign_level_coeff_na,
                    &dc_sign_level_coeff,
                    ROUND_UV(blk_org_x + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].x) >> 1,
                    ROUND_UV(blk_org_y + tx_org[luma_bsize][is_inter][tx_depth][txb_itr].y) >> 1,
                    tx_width_uv,
                    tx_height_uv,
                    NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
                ec_ctx->coded_area_sb_uv += tx_width_uv * tx_height_uv;
            }
            ec_ctx->coded_area_sb += tx_width * tx_height;
        }
    }
    return return_error;
}

/*********************************************************************
* encode_partition_av1
*   Encodes the partition
*********************************************************************/
// Return the number of elements in the partition CDF when
// partitioning the (square) block with luma block size of bsize.
int32_t svt_aom_partition_cdf_length(BlockSize bsize) {
    if (bsize <= BLOCK_8X8) {
        return PARTITION_TYPES;
    } else if (bsize == BLOCK_128X128) {
        return EXT_PARTITION_TYPES - 2;
    } else {
        return EXT_PARTITION_TYPES;
    }
}

static void encode_partition_av1(PictureControlSet* pcs, FRAME_CONTEXT* frame_context, AomWriter* ec_writer,
                                 BlockSize bsize, PartitionType p, uint32_t blk_org_x, uint32_t blk_org_y,
                                 NeighborArrayUnit* partition_context_na) {
    const int32_t is_partition_point = bsize >= BLOCK_8X8;

    if (!is_partition_point) {
        return;
    }

    const int32_t hbs      = (mi_size_wide[bsize] << 2) >> 1;
    const int32_t has_rows = (blk_org_y + hbs) < pcs->ppcs->aligned_height;
    const int32_t has_cols = (blk_org_x + hbs) < pcs->ppcs->aligned_width;

    uint32_t partition_context_left_neighbor_index = get_neighbor_array_unit_left_index(partition_context_na,
                                                                                        blk_org_y);
    uint32_t partition_context_top_neighbor_index  = get_neighbor_array_unit_top_index(partition_context_na, blk_org_x);

    uint32_t context_index = 0;

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
    int32_t       above = (above_ctx >> bsl) & 1, left = (left_ctx >> bsl) & 1;

    assert(mi_size_wide_log2[bsize] == mi_size_high_log2[bsize]);
    assert(bsl >= 0);
    assert(p < CDF_SIZE(EXT_PARTITION_TYPES));

    context_index = (left * 2 + above) + bsl * PARTITION_PLOFFSET;

    if (!has_rows && !has_cols) {
        assert(p == PARTITION_SPLIT);
        return;
    }

    if (has_rows && has_cols) {
        aom_write_symbol(
            ec_writer, p, frame_context->partition_cdf[context_index], svt_aom_partition_cdf_length(bsize));
    } else if (!has_rows && has_cols) {
        AomCdfProb cdf[CDF_SIZE(2)];
        partition_gather_vert_alike(cdf, frame_context->partition_cdf[context_index], bsize);
        aom_write_symbol(ec_writer, p == PARTITION_SPLIT, cdf, 2);
    } else {
        AomCdfProb cdf[CDF_SIZE(2)];
        partition_gather_horz_alike(cdf, frame_context->partition_cdf[context_index], bsize);
        aom_write_symbol(ec_writer, p == PARTITION_SPLIT, cdf, 2);
    }

    return;
}

uint8_t av1_get_skip_context(const MacroBlockD* xd) {
    const MbModeInfo* const above_mi   = xd->above_mbmi;
    const MbModeInfo* const left_mi    = xd->left_mbmi;
    const uint8_t           above_skip = above_mi ? above_mi->block_mi.skip : 0;
    const uint8_t           left_skip  = left_mi ? left_mi->block_mi.skip : 0;
    return above_skip + left_skip;
}

/*********************************************************************
 * encode_skip_coeff_av1
 *   Encodes the skip coefficient flag
 *********************************************************************/
static void encode_skip_coeff_av1(EcBlkStruct* blk_ptr, FRAME_CONTEXT* frame_context, AomWriter* ec_writer,
                                  bool skip_coeff_flag) {
    // TODO: need to code in syntax for segmentation map + skip
    uint8_t ctx = av1_get_skip_context(blk_ptr->av1xd);
    aom_write_symbol(ec_writer, skip_coeff_flag ? 1 : 0, frame_context->skip_cdfs[ctx], 2);
}

/* Get the contexts (left and top) for writing the intra luma mode for key frames. Intended to
 * be used for key frame only. */
void svt_aom_get_kf_y_mode_ctx(const MacroBlockD* xd, uint8_t* above_ctx, uint8_t* left_ctx) {
    PredictionMode intra_luma_left_mode = DC_PRED;
    PredictionMode intra_luma_top_mode  = DC_PRED;
    if (xd->left_available) {
        // When called for key frame, neighbouring mode should be intra
        assert(!is_inter_block(&xd->mi[-1]->block_mi) || is_intrabc_block(&xd->mi[-1]->block_mi));
        intra_luma_left_mode = xd->mi[-1]->block_mi.mode;
    }
    if (xd->up_available) {
        // When called for key frame, neighbouring mode should be intra
        assert(!is_inter_block(&xd->mi[-xd->mi_stride]->block_mi) ||
               is_intrabc_block(&xd->mi[-xd->mi_stride]->block_mi));
        intra_luma_top_mode = xd->mi[-xd->mi_stride]->block_mi.mode;
    }

    *above_ctx = intra_mode_context[intra_luma_top_mode];
    *left_ctx  = intra_mode_context[intra_luma_left_mode];
}

/*********************************************************************
*   Encodes the Intra Luma Mode for a key frame
*********************************************************************/
static void encode_intra_luma_mode_kf_av1(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, MbModeInfo* mbmi,
                                          EcBlkStruct* blk_ptr, BlockSize bsize, uint32_t luma_mode) {
    uint8_t top_context, left_context;
    svt_aom_get_kf_y_mode_ctx(blk_ptr->av1xd, &top_context, &left_context);
    aom_write_symbol(ec_writer, luma_mode, frame_context->kf_y_cdf[top_context][left_context], INTRA_MODES);

    if (bsize >= BLOCK_8X8 && av1_is_directional_mode(mbmi->block_mi.mode)) {
        aom_write_symbol(ec_writer,
                         mbmi->block_mi.angle_delta[PLANE_TYPE_Y] + MAX_ANGLE_DELTA,
                         frame_context->angle_delta_cdf[luma_mode - V_PRED],
                         2 * MAX_ANGLE_DELTA + 1);
    }

    return;
}

/*********************************************************************
* encode_intra_luma_mode_nonkey_av1
*   Encodes the Intra Luma Mode for non Key frames
*********************************************************************/
static void encode_intra_luma_mode_nonkey_av1(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, MbModeInfo* mbmi,
                                              BlockSize bsize, uint32_t luma_mode) {
    aom_write_symbol(ec_writer, luma_mode, frame_context->y_mode_cdf[eb_size_group_lookup[bsize]], INTRA_MODES);

    if (bsize >= BLOCK_8X8 && av1_is_directional_mode(mbmi->block_mi.mode)) {
        aom_write_symbol(ec_writer,
                         mbmi->block_mi.angle_delta[PLANE_TYPE_Y] + MAX_ANGLE_DELTA,
                         frame_context->angle_delta_cdf[luma_mode - V_PRED],
                         2 * MAX_ANGLE_DELTA + 1);
    }

    return;
}

static void write_cfl_alphas(FRAME_CONTEXT* const ec_ctx, int32_t idx, int32_t joint_sign, AomWriter* w) {
    aom_write_symbol(w, joint_sign, ec_ctx->cfl_sign_cdf, CFL_JOINT_SIGNS);
    // Magnitudes are only signaled for nonzero codes.
    if (CFL_SIGN_U(joint_sign) != CFL_SIGN_ZERO) {
        AomCdfProb* cdf_u = ec_ctx->cfl_alpha_cdf[CFL_CONTEXT_U(joint_sign)];
        aom_write_symbol(w, CFL_IDX_U(idx), cdf_u, CFL_ALPHABET_SIZE);
    }
    if (CFL_SIGN_V(joint_sign) != CFL_SIGN_ZERO) {
        AomCdfProb* cdf_v = ec_ctx->cfl_alpha_cdf[CFL_CONTEXT_V(joint_sign)];
        aom_write_symbol(w, CFL_IDX_V(idx), cdf_v, CFL_ALPHABET_SIZE);
    }
}

/*********************************************************************
* encode_intra_chroma_mode_av1
*   Encodes the Intra Chroma Mode
*********************************************************************/
static void encode_intra_chroma_mode_av1(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, MbModeInfo* mbmi,
                                         BlockSize bsize, uint32_t luma_mode, uint32_t chroma_mode,
                                         uint8_t cflAllowed) {
    aom_write_symbol(
        ec_writer, chroma_mode, frame_context->uv_mode_cdf[cflAllowed][luma_mode], UV_INTRA_MODES - !cflAllowed);

    if (chroma_mode == UV_CFL_PRED) {
        write_cfl_alphas(frame_context, mbmi->block_mi.cfl_alpha_idx, mbmi->block_mi.cfl_alpha_signs, ec_writer);
    }

    if (bsize >= BLOCK_8X8 && av1_is_directional_mode(get_uv_mode(mbmi->block_mi.uv_mode))) {
        aom_write_symbol(ec_writer,
                         mbmi->block_mi.angle_delta[PLANE_TYPE_UV] + MAX_ANGLE_DELTA,
                         frame_context->angle_delta_cdf[chroma_mode - V_PRED],
                         2 * MAX_ANGLE_DELTA + 1);
    }

    return;
}

uint8_t av1_get_skip_mode_context(const MacroBlockD* xd) {
    const MbModeInfo* const above_mi        = xd->above_mbmi;
    const MbModeInfo* const left_mi         = xd->left_mbmi;
    const int               above_skip_mode = above_mi ? above_mi->block_mi.skip_mode : 0;
    const int               left_skip_mode  = left_mi ? left_mi->block_mi.skip_mode : 0;
    return above_skip_mode + left_skip_mode;
}

/*********************************************************************
 * encode_skip_mode_av1
 *   Encodes the skip Mode flag
 *********************************************************************/
static void encode_skip_mode_av1(const EcBlkStruct* blk_ptr, FRAME_CONTEXT* frame_context, AomWriter* ec_writer,
                                 bool skip_mode_flag) {
    // TODO: not coded in syntax for skip mode/ref-frame/global-mv in segmentation map
    const uint8_t context_index = av1_get_skip_mode_context(blk_ptr->av1xd);

    aom_write_symbol(ec_writer, skip_mode_flag ? 1 : 0, frame_context->skip_mode_cdfs[context_index], 2);
}

/*******************************************************************************
* The mode info data structure has a one element border above and to the
* left of the entries corresponding to real macroblocks.
* The prediction flags in these dummy entries are initialized to 0.
* 0 - inter/inter, inter/--, --/inter, --/--
* 1 - intra/inter, inter/intra
* 2 - intra/--, --/intra
* 3 - intra/intra
 ******************************************************************************/
uint8_t svt_av1_get_intra_inter_context(const MacroBlockD* xd) {
    const MbModeInfo* const above_mbmi = xd->above_mbmi;
    const MbModeInfo* const left_mbmi  = xd->left_mbmi;
    const int               has_above  = xd->up_available;
    const int               has_left   = xd->left_available;

    if (has_above && has_left) { // both edges available
        const int above_intra = !is_inter_block(&above_mbmi->block_mi);
        const int left_intra  = !is_inter_block(&left_mbmi->block_mi);
        return left_intra && above_intra ? 3 : left_intra || above_intra;
    } else if (has_above || has_left) { // one edge available
        return 2 * !is_inter_block(has_above ? &above_mbmi->block_mi : &left_mbmi->block_mi);
    } else {
        return 0;
    }
}

/*********************************************************************
 * encode_pred_mode_av1
 *   Encodes the Prediction Mode
 *********************************************************************/
static void write_is_inter(const EcBlkStruct* blk_ptr, FRAME_CONTEXT* frame_context, AomWriter* ec_writer,
                           int32_t is_inter) {
    const uint8_t ctx = svt_av1_get_intra_inter_context(blk_ptr->av1xd);
    aom_write_symbol(ec_writer, is_inter, frame_context->intra_inter_cdf[ctx], 2);
}

//****************************************************************************************************//

/*********************************************************************
* svt_aom_motion_mode_allowed
*   checks the motion modes that are allowed for the current block
*********************************************************************/
MotionMode svt_aom_motion_mode_allowed(const PictureControlSet* pcs, uint16_t num_proj_ref,
                                       uint32_t overlappable_neighbors, const BlockSize bsize, MvReferenceFrame rf0,
                                       MvReferenceFrame rf1, PredictionMode mode) {
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    if (!frm_hdr->is_motion_mode_switchable) {
        return SIMPLE_TRANSLATION;
    }

    if (frm_hdr->force_integer_mv == 0) {
        const TransformationType gm_type = pcs->ppcs->global_motion[rf0].wmtype;
        if (is_global_mv_block(mode, bsize, gm_type)) {
            return SIMPLE_TRANSLATION;
        }
    }
    if (is_motion_variation_allowed_bsize(bsize) && is_inter_singleref_mode(mode) && rf1 != INTRA_FRAME &&
        !(rf1 > INTRA_FRAME)) // is_motion_variation_allowed_compound
    {
        if (overlappable_neighbors == 0) {
            return SIMPLE_TRANSLATION;
        }

        if (frm_hdr->allow_warped_motion &&
            /* TODO(JS): when scale is added, put: !av1_is_scaled(&(xd->block_refs[0]->sf)) && */
            num_proj_ref >= 1) {
            if (frm_hdr->force_integer_mv) {
                return OBMC_CAUSAL;
            }
            return WARPED_CAUSAL;
        }
        return OBMC_CAUSAL;
    } else {
        return SIMPLE_TRANSLATION;
    }
}

/*********************************************************************
* write_motion_mode
*   Encodes the Motion Mode (obmc or warped)
*********************************************************************/
static void write_motion_mode(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, BlockSize bsize, MbModeInfo* mbmi,
                              MotionMode motion_mode, MvReferenceFrame rf0, MvReferenceFrame rf1, EcBlkStruct* blk_ptr,
                              PictureControlSet* pcs) {
    MotionMode last_motion_mode_allowed = svt_aom_motion_mode_allowed(
        pcs, mbmi->block_mi.num_proj_ref, blk_ptr->overlappable_neighbors, bsize, rf0, rf1, mbmi->block_mi.mode);
    switch (last_motion_mode_allowed) {
    case SIMPLE_TRANSLATION:
        break;
    case OBMC_CAUSAL:
        aom_write_symbol(ec_writer, motion_mode == OBMC_CAUSAL, frame_context->obmc_cdf[bsize], 2);
        break;
    default:
        aom_write_symbol(ec_writer, motion_mode, frame_context->motion_mode_cdf[bsize], MOTION_MODES);
    }

    return;
}

//****************************************************************************************************//

EbErrorType svt_aom_encode_slice_finish(EntropyCoder* ec) {
    EbErrorType return_error = EB_ErrorNone;

    aom_stop_encode(&ec->ec_writer);

    return return_error;
}

EbErrorType svt_aom_reset_entropy_coder(EncodeContext* enc_ctx, EntropyCoder* ec, uint32_t qp, SliceType slice_type) {
    EbErrorType return_error = EB_ErrorNone;

    (void)enc_ctx;
    (void)slice_type;
    svt_av1_default_coef_probs(ec->fc, qp);
    svt_aom_init_mode_probs(ec->fc);

    return return_error;
}

static void entropy_tile_info_dctor(EbPtr p) {
    EntropyTileInfo* obj = (EntropyTileInfo*)p;
    EB_DELETE(obj->ec);
}

EbErrorType svt_aom_entropy_tile_info_ctor(EntropyTileInfo* eti, uint32_t buf_size) {
    EbErrorType return_error = EB_ErrorNone;
    eti->dctor               = entropy_tile_info_dctor;
    EB_NEW(eti->ec, svt_aom_entropy_coder_ctor, buf_size);
    eti->entropy_coding_tile_done = false;
    return return_error;
}

static void bitstream_dctor(EbPtr p) {
    Bitstream* obj = (Bitstream*)p;
    EB_DELETE(obj->output_bitstream_ptr);
}

EbErrorType svt_aom_bitstream_ctor(Bitstream* bitstream_ptr, uint32_t buffer_size) {
    bitstream_ptr->dctor = bitstream_dctor;
    EB_NEW(bitstream_ptr->output_bitstream_ptr, svt_aom_output_bitstream_unit_ctor, buffer_size);
    return EB_ErrorNone;
}

void svt_aom_bitstream_reset(Bitstream* bitstream_ptr) {
    svt_aom_output_bitstream_reset(bitstream_ptr->output_bitstream_ptr);
}

int svt_aom_bitstream_get_bytes_count(const Bitstream* bitstream_ptr) {
    const OutputBitstreamUnit* unit = bitstream_ptr->output_bitstream_ptr;
    return (int)(unit->buffer_av1 - unit->buffer_begin_av1);
}

void svt_aom_bitstream_copy(const Bitstream* bitstream_ptr, void* dest, int size) {
    const OutputBitstreamUnit* unit = bitstream_ptr->output_bitstream_ptr;
    svt_memcpy(dest, unit->buffer_begin_av1, size);
}

static void entropy_coder_dctor(EbPtr p) {
    EntropyCoder*        obj                  = (EntropyCoder*)p;
    OutputBitstreamUnit* output_bitstream_ptr = (OutputBitstreamUnit*)obj->ec_output_bitstream_ptr;
    EB_DELETE(output_bitstream_ptr);

    EB_FREE(obj->fc);
}

EbErrorType svt_aom_entropy_coder_ctor(EntropyCoder* ec, uint32_t buffer_size) {
    OutputBitstreamUnit* output_bitstream_ptr;

    ec->dctor = entropy_coder_dctor;

    EB_MALLOC_OBJECT(ec->fc);

    EB_NEW(output_bitstream_ptr, svt_aom_output_bitstream_unit_ctor, buffer_size);
    ec->ec_output_bitstream_ptr = output_bitstream_ptr;

    return EB_ErrorNone;
}

//*******************************************************************************************//
//*******************************************************************************************//
//*******************************************************************************************//
//*******************************************************************************************//
// aom_integer.c
static const size_t   k_maximum_leb_128_size  = 8;
static const uint64_t k_maximum_leb_128_value = 0xFFFFFFFFFFFFFF; // 2 ^ 56 - 1

size_t svt_aom_uleb_size_in_bytes(uint64_t value) {
    size_t size = 0;
    do {
        ++size;
    } while ((value >>= 7) != 0);
    return size;
}

int32_t svt_aom_uleb_encode(uint64_t value, size_t available, uint8_t* coded_value, size_t* coded_size) {
    const size_t leb_size = svt_aom_uleb_size_in_bytes(value);
    if (value > k_maximum_leb_128_value || leb_size > k_maximum_leb_128_size || leb_size > available || !coded_value ||
        !coded_size) {
        return -1;
    }

    for (size_t i = 0; i < leb_size; ++i) {
        uint8_t byte = value & 0x7f;
        value >>= 7;

        if (value != 0) {
            byte |= 0x80; // Signal that more bytes follow.
        }

        *(coded_value + i) = byte;
    }

    *coded_size = leb_size;
    return 0;
}

int32_t svt_aom_wb_is_byte_aligned(const AomWriteBitBuffer* wb) {
    return (wb->bit_offset % CHAR_BIT == 0);
}

uint32_t svt_aom_wb_bytes_written(const AomWriteBitBuffer* wb) {
    return wb->bit_offset / CHAR_BIT + (wb->bit_offset % CHAR_BIT > 0);
}

INLINE static void svt_aom_wb_write_bit_inlined(AomWriteBitBuffer* wb, int32_t bit) {
    const int32_t off = (int32_t)wb->bit_offset;
    const int32_t p   = off / CHAR_BIT;
    const int32_t q   = CHAR_BIT - 1 - off % CHAR_BIT;
    if (q == CHAR_BIT - 1) {
        // zero next char and write bit
        wb->bit_buffer[p] = (uint8_t)(bit << q);
    } else {
        wb->bit_buffer[p] &= ~(1 << q);
        wb->bit_buffer[p] |= bit << q;
    }
    wb->bit_offset = off + 1;
}

INLINE static void svt_aom_wb_write_literal_inlined(AomWriteBitBuffer* wb, int32_t data, int32_t bits) {
    int32_t bit;
    for (bit = bits - 1; bit >= 0; bit--) {
        svt_aom_wb_write_bit(wb, (data >> bit) & 1);
    }
}

void NOINLINE svt_aom_wb_write_bit(AomWriteBitBuffer* wb, int32_t bit) {
    svt_aom_wb_write_bit_inlined(wb, bit);
}

void NOINLINE svt_aom_wb_write_literal(AomWriteBitBuffer* wb, int32_t data, int32_t bits) {
    svt_aom_wb_write_literal_inlined(wb, data, bits);
}

void NOINLINE svt_aom_wb_write_inv_signed_literal(AomWriteBitBuffer* wb, int32_t data, int32_t bits) {
    svt_aom_wb_write_literal_inlined(wb, data, bits + 1);
}

//*******************************************************************************************//

static void write_inter_mode(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, PredictionMode mode,
                             const int16_t mode_ctx, uint32_t blk_org_x, uint32_t blk_org_y) {
    (void)blk_org_x;
    (void)blk_org_y;
    int16_t newmv_ctx = mode_ctx & NEWMV_CTX_MASK;
    assert(newmv_ctx < NEWMV_MODE_CONTEXTS);
    aom_write_symbol(ec_writer, mode != NEWMV, frame_context->newmv_cdf[newmv_ctx], 2);

    if (mode != NEWMV) {
        const int16_t zeromv_ctx = (mode_ctx >> GLOBALMV_OFFSET) & GLOBALMV_CTX_MASK;
        aom_write_symbol(ec_writer, mode != GLOBALMV, frame_context->zeromv_cdf[zeromv_ctx], 2);

        if (mode != GLOBALMV) {
            int16_t refmv_ctx = (mode_ctx >> REFMV_OFFSET) & REFMV_CTX_MASK;
            assert(refmv_ctx < REFMV_MODE_CONTEXTS);
            aom_write_symbol(ec_writer, mode != NEARESTMV, frame_context->refmv_cdf[refmv_ctx], 2);
        }
    }
}

//extern INLINE int8_t av1_ref_frame_type(const MvReferenceFrame *const rf);
static void write_drl_idx(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, MbModeInfo* mbmi,

                          EcBlkStruct* blk_ptr) {
    const int32_t new_mv = mbmi->block_mi.mode == NEWMV || mbmi->block_mi.mode == NEW_NEWMV;
    if (new_mv) {
        int32_t idx;
        for (idx = 0; idx < 2; ++idx) {
            if (blk_ptr->drl_ctx[idx] != -1) {
                uint8_t drl_ctx = (uint8_t)blk_ptr->drl_ctx[idx];

                aom_write_symbol(ec_writer, blk_ptr->drl_index != idx, frame_context->drl_cdf[drl_ctx], 2);

                if (blk_ptr->drl_index == idx) {
                    return;
                }
            }
        }
        return;
    }

    if (have_nearmv_in_inter_mode(mbmi->block_mi.mode)) {
        int32_t idx;
        // TODO(jingning): Temporary solution to compensate the NEARESTMV offset.
        for (idx = 1; idx < 3; ++idx) {
            if (blk_ptr->drl_ctx_near[idx - 1] != -1) {
                uint8_t drl_ctx = (uint8_t)blk_ptr->drl_ctx_near[idx - 1];

                aom_write_symbol(ec_writer, blk_ptr->drl_index != (idx - 1), frame_context->drl_cdf[drl_ctx], 2);

                if (blk_ptr->drl_index == (idx - 1)) {
                    return;
                }
            }
        }
        return;
    }
}

static void encode_mv_component(AomWriter* w, int32_t comp, NmvComponent* mvcomp, MvSubpelPrecision precision) {
    int32_t       offset;
    const int32_t sign     = comp < 0;
    const int32_t mag      = sign ? -comp : comp;
    const int32_t mv_class = svt_av1_get_mv_class(mag - 1, &offset);
    const int32_t d        = offset >> 3; // int32_t mv data
    const int32_t fr       = (offset >> 1) & 3; // fractional mv data
    const int32_t hp       = offset & 1; // high precision mv data

    assert(comp != 0);

    // Sign
    aom_write_symbol(w, sign, mvcomp->sign_cdf, 2);

    // Class
    aom_write_symbol(w, mv_class, mvcomp->classes_cdf, MV_CLASSES);

    // Integer bits
    if (mv_class == MV_CLASS_0) {
        aom_write_symbol(w, d, mvcomp->class0_cdf, CLASS0_SIZE);
    } else {
        int32_t       i;
        const int32_t n = mv_class + CLASS0_BITS - 1; // number of bits
        for (i = 0; i < n; ++i) {
            aom_write_symbol(w, (d >> i) & 1, mvcomp->bits_cdf[i], 2);
        }
    }
    // Fractional bits
    if (precision > MV_SUBPEL_NONE) {
        aom_write_symbol(w, fr, mv_class == MV_CLASS_0 ? mvcomp->class0_fp_cdf[d] : mvcomp->fp_cdf, MV_FP_SIZE);
    }

    // High precision bit
    if (precision > MV_SUBPEL_LOW_PRECISION) {
        aom_write_symbol(w, hp, mv_class == MV_CLASS_0 ? mvcomp->class0_hp_cdf : mvcomp->hp_cdf, 2);
    }
}

// can't mark the parameter as const due to MSVC not supporting c99 fully.
#ifdef _MSC_VER
static MvJointType av1_get_mv_joint_diff(const int32_t diff[2]) {
#else
static MvJointType av1_get_mv_joint_diff(const int32_t diff[const 2]) {
#endif
    if (diff[0] == 0) {
        return diff[1] == 0 ? MV_JOINT_ZERO : MV_JOINT_HNZVZ;
    }
    return diff[1] == 0 ? MV_JOINT_HZVNZ : MV_JOINT_HNZVNZ;
}

void svt_av1_encode_mv(PictureParentControlSet* pcs, AomWriter* ec_writer, const Mv* mv, const Mv* ref,
                       NmvContext* mvctx, int32_t usehp) {
    // The y-component (row component) of the MV is coded first
    int32_t           diff[2] = {mv->y - ref->y, mv->x - ref->x};
    const MvJointType j       = av1_get_mv_joint_diff(diff);

    if (pcs->frm_hdr.force_integer_mv) {
        usehp = MV_SUBPEL_NONE;
    }
    aom_write_symbol(ec_writer, j, mvctx->joints_cdf, MV_JOINTS);
    if (mv_joint_vertical(j)) {
        encode_mv_component(ec_writer, diff[0], &mvctx->comps[0], (MvSubpelPrecision)usehp);
    }

    if (mv_joint_horizontal(j)) {
        encode_mv_component(ec_writer, diff[1], &mvctx->comps[1], (MvSubpelPrecision)usehp);
    }

    // If auto_mv_step_size is enabled then keep track of the largest
    // motion vector component used.
    //if (cpi->sf.mv.auto_mv_step_size) {
    //    uint32_t maxv = AOMMAX(abs(mv->row), abs(mv->col)) >> 3;
    //    cpi->max_mv_magnitude = AOMMAX(maxv, cpi->max_mv_magnitude);
    //}
}

//Returns a context number for the given MB prediction signal
static InterpFilter svt_aom_get_ref_filter_type(const BlockModeInfo* ref_mbmi, int dir, MvReferenceFrame ref_frame) {
    return ((ref_mbmi->ref_frame[0] == ref_frame || ref_mbmi->ref_frame[1] == ref_frame)
                ? av1_extract_interp_filter(ref_mbmi->interp_filters, dir & 0x01)
                : SWITCHABLE_FILTERS);
}

/* Get the context for the interpolation filter when SWITCHABLE filter is specified
at the frame level.  Used for computing rate and for entropy coding. */
int svt_aom_get_pred_context_switchable_interp(MvReferenceFrame rf0, MvReferenceFrame rf1, const MacroBlockD* xd,
                                               int dir) {
    /* When calling the function from MD, the current MBMI may not be updated yet, so pass
       the ref frames instead of getting them from the current mbmi (as you could below):

            const MbModeInfo* const mbmi = &xd->mi[0]->mbmi;
            const int ctx_offset = (mbmi->block_mi.ref_frame[1] > INTRA_FRAME) * INTER_FILTER_COMP_OFFSET;
            assert(dir == 0 || dir == 1);
            const MvReferenceFrame ref_frame = mbmi->block_mi.ref_frame[0];
    */

    const int32_t ctx_offset = (rf1 > INTRA_FRAME) * INTER_FILTER_COMP_OFFSET;
    assert(dir == 0 || dir == 1);
    MvReferenceFrame ref_frame = rf0;

    // Note:
    // The mode info data structure has a one element border above and to the
    // left of the entries corresponding to real macroblocks.
    // The prediction flags in these dummy entries are initialized to 0.
    int filter_type_ctx = ctx_offset + (dir & 0x01) * INTER_FILTER_DIR_OFFSET;
    int left_type       = SWITCHABLE_FILTERS;
    int above_type      = SWITCHABLE_FILTERS;

    if (xd->left_available) {
        left_type = svt_aom_get_ref_filter_type(&xd->mi[-1]->block_mi, dir, ref_frame);
    }

    if (xd->up_available) {
        above_type = svt_aom_get_ref_filter_type(&xd->mi[-xd->mi_stride]->block_mi, dir, ref_frame);
    }

    if (left_type == above_type) {
        filter_type_ctx += left_type;
    } else if (left_type == SWITCHABLE_FILTERS) {
        assert(above_type != SWITCHABLE_FILTERS);
        filter_type_ctx += above_type;
    } else if (above_type == SWITCHABLE_FILTERS) {
        assert(left_type != SWITCHABLE_FILTERS);
        filter_type_ctx += left_type;
    } else {
        filter_type_ctx += SWITCHABLE_FILTERS;
    }
    return filter_type_ctx;
}

int svt_aom_is_nontrans_global_motion(const BlockModeInfo* block_mi, const BlockSize bsize,
                                      PictureParentControlSet* pcs) {
    // First check if all modes are GLOBALMV
    if (block_mi->mode != GLOBALMV && block_mi->mode != GLOBAL_GLOBALMV) {
        return 0;
    }

    if (MIN(mi_size_wide[bsize], mi_size_high[bsize]) < 2) {
        return 0;
    }
    const uint8_t is_compound = is_inter_compound_mode(block_mi->mode);
    // Now check if all global motion is non translational
    for (int ref = 0; ref < 1 + is_compound; ++ref) {
        if (pcs->global_motion[block_mi->ref_frame[ref]].wmtype == TRANSLATION) {
            return 0;
        }
    }
    return 1;
}

static int av1_is_interp_needed(const BlockModeInfo* block_mi, const BlockSize bsize, PictureParentControlSet* pcs) {
    if (block_mi->skip_mode) {
        return 0;
    }

    if (block_mi->motion_mode == WARPED_CAUSAL) {
        return 0;
    }

    if (svt_aom_is_nontrans_global_motion(block_mi, bsize, pcs)) {
        return 0;
    }

    return 1;
}

static void write_mb_interp_filter(BlockSize bsize, MvReferenceFrame rf0, MvReferenceFrame rf1,
                                   PictureParentControlSet* pcs, AomWriter* ec_writer, MbModeInfo* mbmi,
                                   EcBlkStruct* blk_ptr, EntropyCoder* ec) {
    FrameHeader* const frm_hdr = &pcs->frm_hdr;

    if (frm_hdr->interpolation_filter != SWITCHABLE || !av1_is_interp_needed(&mbmi->block_mi, bsize, pcs)) {
        return;
    }

    const int max_dir = pcs->scs->seq_header.enable_dual_filter ? 2 : 1;
    for (int dir = 0; dir < max_dir; ++dir) {
        const int    ctx    = svt_aom_get_pred_context_switchable_interp(rf0, rf1, blk_ptr->av1xd, dir);
        InterpFilter filter = av1_extract_interp_filter(mbmi->block_mi.interp_filters, dir);
        assert(ctx < SWITCHABLE_FILTER_CONTEXTS);
        assert(filter < CDF_SIZE(SWITCHABLE_FILTERS));
        aom_write_symbol(ec_writer, filter, ec->fc->switchable_interp_cdf[ctx], SWITCHABLE_FILTERS);
    }
}

static void write_inter_compound_mode(FRAME_CONTEXT* frame_context, AomWriter* ec_writer, PredictionMode mode,
                                      const int16_t mode_ctx) {
    assert(is_inter_compound_mode(mode));
    aom_write_symbol(
        ec_writer, INTER_COMPOUND_OFFSET(mode), frame_context->inter_compound_mode_cdf[mode_ctx], INTER_COMPOUND_MODES);
}

int svt_aom_get_reference_mode_context_new(const MacroBlockD* xd);

AomCdfProb* svt_aom_get_reference_mode_cdf(const MacroBlockD* xd) {
    return xd->tile_ctx->comp_inter_cdf[svt_aom_get_reference_mode_context_new(xd)];
}

int svt_aom_get_comp_reference_type_context_new(const MacroBlockD* xd);

// == Uni-directional contexts ==

int svt_av1_get_pred_context_uni_comp_ref_p(const MacroBlockD* xd);

int svt_av1_get_pred_context_uni_comp_ref_p1(const MacroBlockD* xd);

int svt_av1_get_pred_context_uni_comp_ref_p2(const MacroBlockD* xd);

AomCdfProb* svt_aom_get_comp_reference_type_cdf(const MacroBlockD* xd) {
    const int pred_context = svt_aom_get_comp_reference_type_context_new(xd);
    return xd->tile_ctx->comp_ref_type_cdf[pred_context];
}

AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_uni_comp_ref_p(xd);
    return xd->tile_ctx->uni_comp_ref_cdf[pred_context][0];
}

AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p1(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_uni_comp_ref_p1(xd);
    return xd->tile_ctx->uni_comp_ref_cdf[pred_context][1];
}

AomCdfProb* svt_aom_get_pred_cdf_uni_comp_ref_p2(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_uni_comp_ref_p2(xd);
    return xd->tile_ctx->uni_comp_ref_cdf[pred_context][2];
}

AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_comp_ref_p(xd);
    return xd->tile_ctx->comp_ref_cdf[pred_context][0];
}

AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p1(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_comp_ref_p1(xd);
    return xd->tile_ctx->comp_ref_cdf[pred_context][1];
}

AomCdfProb* svt_aom_get_pred_cdf_comp_ref_p2(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_comp_ref_p2(xd);
    return xd->tile_ctx->comp_ref_cdf[pred_context][2];
}

AomCdfProb* svt_aom_get_pred_cdf_comp_bwdref_p(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_comp_bwdref_p(xd);
    return xd->tile_ctx->comp_bwdref_cdf[pred_context][0];
}

AomCdfProb* svt_aom_get_pred_cdf_comp_bwdref_p1(const MacroBlockD* xd) {
    const int pred_context = svt_av1_get_pred_context_comp_bwdref_p1(xd);
    return xd->tile_ctx->comp_bwdref_cdf[pred_context][1];
}

int svt_aom_get_comp_reference_type_context_new(const MacroBlockD* xd) {
    int                     pred_context;
    const MbModeInfo* const above_mbmi     = xd->above_mbmi;
    const MbModeInfo* const left_mbmi      = xd->left_mbmi;
    const int               above_in_image = xd->up_available;
    const int               left_in_image  = xd->left_available;

    if (above_in_image && left_in_image) { // both edges available
        const int above_intra = !is_inter_block(&above_mbmi->block_mi);
        const int left_intra  = !is_inter_block(&left_mbmi->block_mi);

        if (above_intra && left_intra) { // intra/intra
            pred_context = 2;
        } else if (above_intra || left_intra) { // intra/inter
            const MbModeInfo* inter_mbmi = above_intra ? left_mbmi : above_mbmi;

            if (!has_second_ref(&inter_mbmi->block_mi)) { // single pred
                pred_context = 2;
            } else { // comp pred
                pred_context = 1 + 2 * has_uni_comp_refs(&inter_mbmi->block_mi);
            }
        } else { // inter/inter
            const int              a_sg = !has_second_ref(&above_mbmi->block_mi);
            const int              l_sg = !has_second_ref(&left_mbmi->block_mi);
            const MvReferenceFrame frfa = above_mbmi->block_mi.ref_frame[0];
            const MvReferenceFrame frfl = left_mbmi->block_mi.ref_frame[0];

            if (a_sg && l_sg) { // single/single
                pred_context = 1 + 2 * (!(IS_BACKWARD_REF_FRAME(frfa) ^ IS_BACKWARD_REF_FRAME(frfl)));
            } else if (l_sg || a_sg) { // single/comp
                const int uni_rfc = a_sg ? has_uni_comp_refs(&left_mbmi->block_mi)
                                         : has_uni_comp_refs(&above_mbmi->block_mi);

                if (!uni_rfc) { // comp bidir
                    pred_context = 1;
                } else { // comp unidir
                    pred_context = 3 + (!(IS_BACKWARD_REF_FRAME(frfa) ^ IS_BACKWARD_REF_FRAME(frfl)));
                }
            } else { // comp/comp
                const int a_uni_rfc = has_uni_comp_refs(&above_mbmi->block_mi);
                const int l_uni_rfc = has_uni_comp_refs(&left_mbmi->block_mi);

                if (!a_uni_rfc && !l_uni_rfc) { // bidir/bidir
                    pred_context = 0;
                } else if (!a_uni_rfc || !l_uni_rfc) { // unidir/bidir
                    pred_context = 2;
                } else { // unidir/unidir
                    pred_context = 3 + (!((frfa == BWDREF_FRAME) ^ (frfl == BWDREF_FRAME)));
                }
            }
        }
    } else if (above_in_image || left_in_image) { // one edge available
        const MbModeInfo* edge_mbmi = above_in_image ? above_mbmi : left_mbmi;

        if (!is_inter_block(&edge_mbmi->block_mi)) { // intra
            pred_context = 2;
        } else { // inter
            if (!has_second_ref(&edge_mbmi->block_mi)) { // single pred
                pred_context = 2;
            } else { // comp pred
                pred_context = 4 * has_uni_comp_refs(&edge_mbmi->block_mi);
            }
        }
    } else { // no edges available
        pred_context = 2;
    }

    assert(pred_context >= 0 && pred_context < COMP_REF_TYPE_CONTEXTS);
    return pred_context;
}

// Returns a context number for the given MB prediction signal
//
// Signal the uni-directional compound reference frame pair as either
// (BWDREF, ALTREF), or (LAST, LAST2) / (LAST, LAST3) / (LAST, GOLDEN),
// conditioning on the pair is known as uni-directional.
//
// 3 contexts: Voting is used to compare the count of forward references with
//             that of backward references from the spatial neighbors.
int svt_av1_get_pred_context_uni_comp_ref_p(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of forward references (L, L2, L3, or G)
    const int frf_count = ref_counts[LAST_FRAME] + ref_counts[LAST2_FRAME] + ref_counts[LAST3_FRAME] +
        ref_counts[GOLDEN_FRAME];
    // Count of backward references (b or A)
    const int brf_count = ref_counts[BWDREF_FRAME] + ref_counts[ALTREF2_FRAME] + ref_counts[ALTREF_FRAME];

    const int pred_context = (frf_count == brf_count) ? 1 : ((frf_count < brf_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < UNI_COMP_REF_CONTEXTS);
    return pred_context;
}

// Returns a context number for the given MB prediction signal
//
// Signal the uni-directional compound reference frame pair as
// either (LAST, LAST2), or (LAST, LAST3) / (LAST, GOLDEN),
// conditioning on the pair is known as one of the above three.
//
// 3 contexts: Voting is used to compare the count of LAST2_FRAME with the
//             total count of LAST3/GOLDEN from the spatial neighbors.
int svt_av1_get_pred_context_uni_comp_ref_p1(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of LAST2
    const int last2_count = ref_counts[LAST2_FRAME];
    // Count of LAST3 or GOLDEN
    const int last3_or_gld_count = ref_counts[LAST3_FRAME] + ref_counts[GOLDEN_FRAME];

    const int pred_context = (last2_count == last3_or_gld_count) ? 1 : ((last2_count < last3_or_gld_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < UNI_COMP_REF_CONTEXTS);
    return pred_context;
}

// Returns a context number for the given MB prediction signal
//
// Signal the uni-directional compound reference frame pair as
// either (LAST, LAST3) or (LAST, GOLDEN),
// conditioning on the pair is known as one of the above two.
//
// 3 contexts: Voting is used to compare the count of LAST3_FRAME with the
//             total count of GOLDEN_FRAME from the spatial neighbors.
int svt_av1_get_pred_context_uni_comp_ref_p2(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of LAST3
    const int last3_count = ref_counts[LAST3_FRAME];
    // Count of GOLDEN
    const int gld_count = ref_counts[GOLDEN_FRAME];

    const int pred_context = (last3_count == gld_count) ? 1 : ((last3_count < gld_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < UNI_COMP_REF_CONTEXTS);
    return pred_context;
}

int svt_aom_get_reference_mode_context_new(const MacroBlockD* xd) {
    int                     ctx;
    const MbModeInfo* const above_mbmi = xd->above_mbmi;
    const MbModeInfo* const left_mbmi  = xd->left_mbmi;
    const int               has_above  = xd->up_available;
    const int               has_left   = xd->left_available;

    // Note:
    // The mode info data structure has a one element border above and to the
    // left of the entries corresponding to real macroblocks.
    // The prediction flags in these dummy entries are initialized to 0.
    if (has_above && has_left) { // both edges available
        if (!has_second_ref(&above_mbmi->block_mi) && !has_second_ref(&left_mbmi->block_mi)) {
            // neither edge uses comp pred (0/1)
            ctx = IS_BACKWARD_REF_FRAME(above_mbmi->block_mi.ref_frame[0]) ^
                IS_BACKWARD_REF_FRAME(left_mbmi->block_mi.ref_frame[0]);
        } else if (!has_second_ref(&above_mbmi->block_mi)) {
            // one of two edges uses comp pred (2/3)
            ctx = 2 +
                (IS_BACKWARD_REF_FRAME(above_mbmi->block_mi.ref_frame[0]) || !is_inter_block(&above_mbmi->block_mi));
        } else if (!has_second_ref(&left_mbmi->block_mi)) {
            // one of two edges uses comp pred (2/3)
            ctx = 2 +
                (IS_BACKWARD_REF_FRAME(left_mbmi->block_mi.ref_frame[0]) || !is_inter_block(&left_mbmi->block_mi));
        } else { // both edges use comp pred (4)
            ctx = 4;
        }
    } else if (has_above || has_left) { // one edge available
        const MbModeInfo* edge_mbmi = has_above ? above_mbmi : left_mbmi;

        if (!has_second_ref(&edge_mbmi->block_mi)) {
            // edge does not use comp pred (0/1)
            ctx = IS_BACKWARD_REF_FRAME(edge_mbmi->block_mi.ref_frame[0]);
        } else {
            // edge uses comp pred (3)
            ctx = 3;
        }
    } else { // no edges available (1)
        ctx = 1;
    }
    assert(ctx >= 0 && ctx < COMP_INTER_CONTEXTS);
    return ctx;
}

void svt_aom_collect_neighbors_ref_counts_new(MacroBlockD* const xd) {
    av1_zero(xd->neighbors_ref_counts);

    uint8_t* const ref_counts = xd->neighbors_ref_counts;

    const MbModeInfo* const above_mbmi     = xd->above_mbmi;
    const MbModeInfo* const left_mbmi      = xd->left_mbmi;
    const int               above_in_image = xd->up_available;
    const int               left_in_image  = xd->left_available;

    // Above neighbor
    if (above_in_image && is_inter_block(&above_mbmi->block_mi)) {
        ref_counts[above_mbmi->block_mi.ref_frame[0]]++;
        if (has_second_ref(&above_mbmi->block_mi)) {
            ref_counts[above_mbmi->block_mi.ref_frame[1]]++;
        }
    }

    // Left neighbor
    if (left_in_image && is_inter_block(&left_mbmi->block_mi)) {
        ref_counts[left_mbmi->block_mi.ref_frame[0]]++;
        if (has_second_ref(&left_mbmi->block_mi)) {
            ref_counts[left_mbmi->block_mi.ref_frame[1]]++;
        }
    }
}

#define WRITE_REF_BIT(bname, pname) aom_write_symbol(w, bname, svt_aom_get_pred_cdf_##pname(xd), 2)

/***************************************************************************************/

// == Common context functions for both comp and single ref ==
//
// Obtain contexts to signal a reference frame to be either LAST/LAST2 or
// LAST3/GOLDEN.
static int32_t get_pred_context_ll2_or_l3gld(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of LAST + LAST2
    const int32_t last_last2_count = ref_counts[LAST_FRAME] + ref_counts[LAST2_FRAME];
    // Count of LAST3 + GOLDEN
    const int32_t last3_gld_count = ref_counts[LAST3_FRAME] + ref_counts[GOLDEN_FRAME];

    const int32_t pred_context = (last_last2_count == last3_gld_count) ? 1
                                                                       : ((last_last2_count < last3_gld_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < REF_CONTEXTS);
    return pred_context;
}

// Obtain contexts to signal a reference frame to be either LAST or LAST2.
static int32_t get_pred_context_last_or_last2(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of LAST
    const int32_t last_count = ref_counts[LAST_FRAME];
    // Count of LAST2
    const int32_t last2_count = ref_counts[LAST2_FRAME];

    const int32_t pred_context = (last_count == last2_count) ? 1 : ((last_count < last2_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < REF_CONTEXTS);
    return pred_context;
}

// Obtain contexts to signal a reference frame to be either LAST3 or GOLDEN.
static int32_t get_pred_context_last3_or_gld(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of LAST3
    const int32_t last3_count = ref_counts[LAST3_FRAME];
    // Count of GOLDEN
    const int32_t gld_count = ref_counts[GOLDEN_FRAME];

    const int32_t pred_context = (last3_count == gld_count) ? 1 : ((last3_count < gld_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < REF_CONTEXTS);
    return pred_context;
}

// Obtain contexts to signal a reference frame be either BWDREF/ALTREF2, or
// ALTREF.
static int32_t get_pred_context_brfarf2_or_arf(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Counts of BWDREF, ALTREF2, or ALTREF frames (b, A2, or A)
    const int32_t brfarf2_count = ref_counts[BWDREF_FRAME] + ref_counts[ALTREF2_FRAME];
    const int32_t arf_count     = ref_counts[ALTREF_FRAME];

    const int32_t pred_context = (brfarf2_count == arf_count) ? 1 : ((brfarf2_count < arf_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < REF_CONTEXTS);
    return pred_context;
}

// Obtain contexts to signal a reference frame be either BWDREF or ALTREF2.
static int32_t get_pred_context_brf_or_arf2(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of BWDREF frames (b)
    const int32_t brf_count = ref_counts[BWDREF_FRAME];
    // Count of ALTREF2 frames (A2)
    const int32_t arf2_count = ref_counts[ALTREF2_FRAME];

    const int32_t pred_context = (brf_count == arf2_count) ? 1 : ((brf_count < arf2_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < REF_CONTEXTS);
    return pred_context;
}

// == Context functions for comp ref ==
//
// Returns a context number for the given MB prediction signal
// Signal the first reference frame for a compound mode be either
// GOLDEN/LAST3, or LAST/LAST2.
int32_t svt_av1_get_pred_context_comp_ref_p(const MacroBlockD* xd) {
    return get_pred_context_ll2_or_l3gld(xd);
}

// Returns a context number for the given MB prediction signal
// Signal the first reference frame for a compound mode be LAST,
// conditioning on that it is known either LAST/LAST2.
int32_t svt_av1_get_pred_context_comp_ref_p1(const MacroBlockD* xd) {
    return get_pred_context_last_or_last2(xd);
}

// Returns a context number for the given MB prediction signal
// Signal the first reference frame for a compound mode be GOLDEN,
// conditioning on that it is known either GOLDEN or LAST3.
int32_t svt_av1_get_pred_context_comp_ref_p2(const MacroBlockD* xd) {
    return get_pred_context_last3_or_gld(xd);
}

// Signal the 2nd reference frame for a compound mode be either
// ALTREF, or ALTREF2/BWDREF.
int32_t svt_av1_get_pred_context_comp_bwdref_p(const MacroBlockD* xd) {
    return get_pred_context_brfarf2_or_arf(xd);
}

// Signal the 2nd reference frame for a compound mode be either
// ALTREF2 or BWDREF.
int32_t svt_av1_get_pred_context_comp_bwdref_p1(const MacroBlockD* xd) {
    return get_pred_context_brf_or_arf2(xd);
}

// == Context functions for single ref ==
//
// For the bit to signal whether the single reference is a forward reference
// frame or a backward reference frame.
int32_t svt_av1_get_pred_context_single_ref_p1(const MacroBlockD* xd) {
    const uint8_t* const ref_counts = &xd->neighbors_ref_counts[0];

    // Count of forward reference frames
    const int32_t fwd_count = ref_counts[LAST_FRAME] + ref_counts[LAST2_FRAME] + ref_counts[LAST3_FRAME] +
        ref_counts[GOLDEN_FRAME];
    // Count of backward reference frames
    const int32_t bwd_count = ref_counts[BWDREF_FRAME] + ref_counts[ALTREF2_FRAME] + ref_counts[ALTREF_FRAME];

    const int32_t pred_context = (fwd_count == bwd_count) ? 1 : ((fwd_count < bwd_count) ? 0 : 2);

    assert(pred_context >= 0 && pred_context < REF_CONTEXTS);
    return pred_context;
}

AomCdfProb* svt_aom_get_pred_cdf_single_ref_p1(const MacroBlockD* xd) {
    return xd->tile_ctx->single_ref_cdf[svt_av1_get_pred_context_single_ref_p1(xd)][0];
}

AomCdfProb* svt_aom_get_pred_cdf_single_ref_p2(const MacroBlockD* xd) {
    return xd->tile_ctx->single_ref_cdf[svt_av1_get_pred_context_single_ref_p2(xd)][1];
}

AomCdfProb* svt_aom_get_pred_cdf_single_ref_p3(const MacroBlockD* xd) {
    return xd->tile_ctx->single_ref_cdf[svt_av1_get_pred_context_single_ref_p3(xd)][2];
}

AomCdfProb* svt_aom_get_pred_cdf_single_ref_p4(const MacroBlockD* xd) {
    return xd->tile_ctx->single_ref_cdf[svt_av1_get_pred_context_single_ref_p4(xd)][3];
}

AomCdfProb* svt_aom_get_pred_cdf_single_ref_p5(const MacroBlockD* xd) {
    return xd->tile_ctx->single_ref_cdf[svt_av1_get_pred_context_single_ref_p5(xd)][4];
}

AomCdfProb* svt_aom_get_pred_cdf_single_ref_p6(const MacroBlockD* xd) {
    return xd->tile_ctx->single_ref_cdf[svt_av1_get_pred_context_single_ref_p6(xd)][5];
}

// For the bit to signal whether the single reference is ALTREF_FRAME or
// non-ALTREF backward reference frame, knowing that it shall be either of
// these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p2(const MacroBlockD* xd) {
    return get_pred_context_brfarf2_or_arf(xd);
}

// For the bit to signal whether the single reference is LAST3/GOLDEN or
// LAST2/LAST, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p3(const MacroBlockD* xd) {
    return get_pred_context_ll2_or_l3gld(xd);
}

// For the bit to signal whether the single reference is LAST2_FRAME or
// LAST_FRAME, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p4(const MacroBlockD* xd) {
    return get_pred_context_last_or_last2(xd);
}

// For the bit to signal whether the single reference is GOLDEN_FRAME or
// LAST3_FRAME, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p5(const MacroBlockD* xd) {
    return get_pred_context_last3_or_gld(xd);
}

// For the bit to signal whether the single reference is ALTREF2_FRAME or
// BWDREF_FRAME, knowing that it shall be either of these 2 choices.
int32_t svt_av1_get_pred_context_single_ref_p6(const MacroBlockD* xd) {
    return get_pred_context_brf_or_arf2(xd);
}

/***************************************************************************************/

static void write_ref_frames(PictureParentControlSet* pcs, const MacroBlockD* xd, AomWriter* w) {
    FrameHeader*            frm_hdr     = &pcs->frm_hdr;
    const MbModeInfo* const mbmi        = xd->mi[0];
    const int               is_compound = has_second_ref(&mbmi->block_mi);
    {
        // does the feature use compound prediction or not
        // (if not specified at the frame/segment level)
        if (frm_hdr->reference_mode == REFERENCE_MODE_SELECT) {
            if (is_comp_ref_allowed(mbmi->bsize)) {
                aom_write_symbol(w, is_compound, svt_aom_get_reference_mode_cdf(xd), 2);
            }
        } else {
            assert((!is_compound) == (frm_hdr->reference_mode == SINGLE_REFERENCE));
        }

        if (is_compound) {
            const CompReferenceType comp_ref_type = has_uni_comp_refs(&mbmi->block_mi) ? UNIDIR_COMP_REFERENCE
                                                                                       : BIDIR_COMP_REFERENCE;
            aom_write_symbol(w, comp_ref_type, svt_aom_get_comp_reference_type_cdf(xd), 2);

            if (comp_ref_type == UNIDIR_COMP_REFERENCE) {
                const int bit = mbmi->block_mi.ref_frame[0] == BWDREF_FRAME;
                WRITE_REF_BIT(bit, uni_comp_ref_p);

                if (!bit) {
                    assert(mbmi->block_mi.ref_frame[0] == LAST_FRAME);
                    const int bit1 = mbmi->block_mi.ref_frame[1] == LAST3_FRAME ||
                        mbmi->block_mi.ref_frame[1] == GOLDEN_FRAME;
                    WRITE_REF_BIT(bit1, uni_comp_ref_p1);
                    if (bit1) {
                        const int bit2 = mbmi->block_mi.ref_frame[1] == GOLDEN_FRAME;
                        WRITE_REF_BIT(bit2, uni_comp_ref_p2);
                    }
                } else {
                    assert(mbmi->block_mi.ref_frame[1] == ALTREF_FRAME);
                }
                return;
            }

            assert(comp_ref_type == BIDIR_COMP_REFERENCE);

            const int bit = (mbmi->block_mi.ref_frame[0] == GOLDEN_FRAME || mbmi->block_mi.ref_frame[0] == LAST3_FRAME);
            WRITE_REF_BIT(bit, comp_ref_p);

            if (!bit) {
                const int bit1 = mbmi->block_mi.ref_frame[0] == LAST2_FRAME;
                WRITE_REF_BIT(bit1, comp_ref_p1);
            } else {
                const int bit2 = mbmi->block_mi.ref_frame[0] == GOLDEN_FRAME;
                WRITE_REF_BIT(bit2, comp_ref_p2);
            }

            const int bit_bwd = mbmi->block_mi.ref_frame[1] == ALTREF_FRAME;
            WRITE_REF_BIT(bit_bwd, comp_bwdref_p);

            if (!bit_bwd) {
                WRITE_REF_BIT(mbmi->block_mi.ref_frame[1] == ALTREF2_FRAME, comp_bwdref_p1);
            }
        } else {
            const int bit0 = (mbmi->block_mi.ref_frame[0] <= ALTREF_FRAME &&
                              mbmi->block_mi.ref_frame[0] >= BWDREF_FRAME);
            WRITE_REF_BIT(bit0, single_ref_p1);

            if (bit0) {
                const int bit1 = mbmi->block_mi.ref_frame[0] == ALTREF_FRAME;
                WRITE_REF_BIT(bit1, single_ref_p2);
                if (!bit1) {
                    WRITE_REF_BIT(mbmi->block_mi.ref_frame[0] == ALTREF2_FRAME, single_ref_p6);
                }
            } else {
                const int bit2 = (mbmi->block_mi.ref_frame[0] == LAST3_FRAME ||
                                  mbmi->block_mi.ref_frame[0] == GOLDEN_FRAME);
                WRITE_REF_BIT(bit2, single_ref_p3);
                if (!bit2) {
                    const int bit3 = mbmi->block_mi.ref_frame[0] != LAST_FRAME;
                    WRITE_REF_BIT(bit3, single_ref_p4);
                } else {
                    const int bit4 = mbmi->block_mi.ref_frame[0] != LAST3_FRAME;
                    WRITE_REF_BIT(bit4, single_ref_p5);
                }
            }
        }
    }
}

static void encode_restoration_mode(PictureParentControlSet* pcs, AomWriteBitBuffer* wb) {
    FrameHeader* frm_hdr = &pcs->frm_hdr;
    //SVT_ERROR("encode_restoration_mode might not work. Double check the reference code\n");
    assert(!frm_hdr->all_lossless);
    // move out side of the function
    //if (!cm->seq_params.enable_restoration) return;

    if (frm_hdr->allow_intrabc) {
        return;
    }

    const int32_t num_planes = 3; // av1_num_planes(cm);
    int32_t       all_none = 1, chroma_none = 1;
    for (int32_t p = 0; p < num_planes; ++p) {
        RestorationInfo* rsi = &pcs->child_pcs->rst_info[p];

        if (rsi->frame_restoration_type != RESTORE_NONE) {
            all_none = 0;
            chroma_none &= (int32_t)(p == 0);
        }
        switch (rsi->frame_restoration_type) {
        case RESTORE_NONE:
            svt_aom_wb_write_bit(wb, 0);
            svt_aom_wb_write_bit(wb, 0);
            break;
        case RESTORE_WIENER:
            svt_aom_wb_write_bit(wb, 1);
            svt_aom_wb_write_bit(wb, 0);
            break;
        case RESTORE_SGRPROJ:
            svt_aom_wb_write_bit(wb, 1);
            svt_aom_wb_write_bit(wb, 1);
            break;
        case RESTORE_SWITCHABLE:
            svt_aom_wb_write_bit(wb, 0);
            svt_aom_wb_write_bit(wb, 1);
            break;
        default:
            assert(0);
        }
    }
    if (!all_none) {
        const int32_t    sb_size = pcs->scs->seq_header.sb_size == BLOCK_128X128 ? 128 : 64;
        RestorationInfo* rsi     = &pcs->child_pcs->rst_info[0];
        assert(rsi->restoration_unit_size >= sb_size);
        assert(RESTORATION_UNITSIZE_MAX == 256);

        if (sb_size == 64) {
            svt_aom_wb_write_bit(wb, rsi->restoration_unit_size > 64);
        }
        if (rsi->restoration_unit_size > 64) {
            svt_aom_wb_write_bit(wb, rsi->restoration_unit_size > 128);
        }
    }
    if (!chroma_none) {
        svt_aom_wb_write_bit(
            wb, pcs->child_pcs->rst_info[1].restoration_unit_size != pcs->child_pcs->rst_info[0].restoration_unit_size);
        assert(pcs->child_pcs->rst_info[1].restoration_unit_size == pcs->child_pcs->rst_info[0].restoration_unit_size ||
               pcs->child_pcs->rst_info[1].restoration_unit_size ==
                   (pcs->child_pcs->rst_info[0].restoration_unit_size >> 1));
        assert(pcs->child_pcs->rst_info[2].restoration_unit_size == pcs->child_pcs->rst_info[1].restoration_unit_size);
    }
}

static void encode_segmentation(PictureParentControlSet* pcs, AomWriteBitBuffer* wb) {
    SegmentationParams* segmentation_params = &pcs->frm_hdr.segmentation_params;
    svt_aom_wb_write_bit(wb, segmentation_params->segmentation_enabled);
    if (segmentation_params->segmentation_enabled) {
        if (!(pcs->frm_hdr.primary_ref_frame == PRIMARY_REF_NONE)) {
            svt_aom_wb_write_bit(wb, segmentation_params->segmentation_update_map);
            if (segmentation_params->segmentation_update_map) {
                svt_aom_wb_write_bit(wb, segmentation_params->segmentation_temporal_update);
            }
            svt_aom_wb_write_bit(wb, segmentation_params->segmentation_update_data);
        }
        if (segmentation_params->segmentation_update_data) {
            for (int i = 0; i < MAX_SEGMENTS; i++) {
                for (int j = 0; j < SEG_LVL_MAX; j++) {
                    svt_aom_wb_write_bit(wb, segmentation_params->feature_enabled[i][j]);
                    if (segmentation_params->feature_enabled[i][j]) {
                        //TODO: add clamping
                        if (svt_aom_segmentation_feature_signed[j]) {
                            svt_aom_wb_write_inv_signed_literal(
                                wb, segmentation_params->feature_data[i][j], svt_aom_segmentation_feature_bits[j]);
                        } else {
                            svt_aom_wb_write_literal(
                                wb, segmentation_params->feature_data[i][j], svt_aom_segmentation_feature_bits[j]);
                        }
                    }
                }
            }
        }
    }
}

static void encode_loopfilter(PictureParentControlSet* pcs, AomWriteBitBuffer* wb) {
    FrameHeader* frm_hdr = &pcs->frm_hdr;
    assert(!frm_hdr->coded_lossless);
    if (frm_hdr->allow_intrabc) {
        return;
    }

    LoopFilter* lf = &frm_hdr->loop_filter_params;

    // Encode the loop filter level and type
    svt_aom_wb_write_literal(wb, lf->filter_level[0], 6);
    svt_aom_wb_write_literal(wb, lf->filter_level[1], 6);
    if (lf->filter_level[0] || lf->filter_level[1]) {
        svt_aom_wb_write_literal(wb, lf->filter_level_u, 6);
        svt_aom_wb_write_literal(wb, lf->filter_level_v, 6);
    }
    svt_aom_wb_write_literal(wb, lf->sharpness_level, 3);

    // Write out loop filter deltas applied at the MB level based on mode or
    // ref frame (if they are enabled).
    svt_aom_wb_write_bit(wb, lf->mode_ref_delta_enabled);
    if (lf->mode_ref_delta_enabled) {
        SVT_ERROR("Loop Filter is not supported yet \n");
        /* svt_aom_wb_write_bit(wb, lf->mode_ref_delta_update);
        if (lf->mode_ref_delta_update) {
        const int32_t prime_idx = pcs->primary_ref_frame;
        const int32_t buf_idx =
        prime_idx == PRIMARY_REF_NONE ? -1 : cm->frame_refs[prime_idx].idx;
        int8_t last_ref_deltas[TOTAL_REFS_PER_FRAME];
        if (prime_idx == PRIMARY_REF_NONE || buf_idx < 0) {
        av1_set_default_ref_deltas(last_ref_deltas);
        } else {
        svt_memcpy(last_ref_deltas, cm->buffer_pool->frame_bufs[buf_idx].ref_deltas,
        TOTAL_REFS_PER_FRAME);
        }
        for (i = 0; i < TOTAL_REFS_PER_FRAME; i++) {
        const int32_t delta = lf->ref_deltas[i];
        const int32_t changed = delta != last_ref_deltas[i];
        svt_aom_wb_write_bit(wb, changed);
        if (changed) svt_aom_wb_write_inv_signed_literal(wb, delta, 6);
        }
        int8_t last_mode_deltas[MAX_MODE_LF_DELTAS];
        if (prime_idx == PRIMARY_REF_NONE || buf_idx < 0) {
        av1_set_default_mode_deltas(last_mode_deltas);
        } else {
        svt_memcpy(last_mode_deltas,
        cm->buffer_pool->frame_bufs[buf_idx].mode_deltas,
        MAX_MODE_LF_DELTAS);
        }

        for (i = 0; i < MAX_MODE_LF_DELTAS; i++) {
        const int32_t delta = lf->mode_deltas[i];
        const int32_t changed = delta != last_mode_deltas[i];
        svt_aom_wb_write_bit(wb, changed);
        if (changed) svt_aom_wb_write_inv_signed_literal(wb, delta, 6);
        }
        }*/
    }
}

static void encode_cdef(const PictureParentControlSet* pcs, AomWriteBitBuffer* wb) {
    //assert(!cm->coded_lossless);
    // moved out side
    //if (!cm->seq_params.cdef_level) return;

    const FrameHeader* frm_hdr = &pcs->frm_hdr;

    if (frm_hdr->allow_intrabc) {
        return;
    }

    svt_aom_wb_write_literal(wb, frm_hdr->cdef_params.cdef_damping - 3, 2);
    //cdef_pri_damping & cdef_sec_damping consolidated to cdef_damping
    //assert(pcs->cdef_pri_damping == pcs->cdef_sec_damping);
    svt_aom_wb_write_literal(wb, frm_hdr->cdef_params.cdef_bits, 2);
    for (int32_t i = 0; i < pcs->nb_cdef_strengths; i++) {
        svt_aom_wb_write_literal(wb, frm_hdr->cdef_params.cdef_y_strength[i], CDEF_STRENGTH_BITS);
        svt_aom_wb_write_literal(wb, frm_hdr->cdef_params.cdef_uv_strength[i], CDEF_STRENGTH_BITS);
    }
}

static void write_delta_q(AomWriteBitBuffer* wb, int32_t delta_q) {
    if (delta_q != 0) {
        svt_aom_wb_write_bit(wb, 1);
        svt_aom_wb_write_inv_signed_literal(wb, delta_q, 6);
    } else {
        svt_aom_wb_write_bit(wb, 0);
    }
}

static void encode_quantization(const PictureParentControlSet* const pcs, AomWriteBitBuffer* wb) {
    const FrameHeader* frm_hdr = &pcs->frm_hdr;
    svt_aom_wb_write_literal(wb, frm_hdr->quantization_params.base_q_idx, QINDEX_BITS);
    write_delta_q(wb, frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_Y]);
    int32_t diff_uv_delta = (frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_U] !=
                             frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_V]) ||
        (frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_U] != frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_V]);

    if (diff_uv_delta) {
        svt_aom_wb_write_bit(wb, diff_uv_delta);
    }
    write_delta_q(wb, frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_U]);
    write_delta_q(wb, frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_U]);
    if (diff_uv_delta) {
        write_delta_q(wb, frm_hdr->quantization_params.delta_q_dc[AOM_PLANE_V]);
        write_delta_q(wb, frm_hdr->quantization_params.delta_q_ac[AOM_PLANE_V]);
    }
    svt_aom_wb_write_bit(wb, frm_hdr->quantization_params.using_qmatrix);
    if (frm_hdr->quantization_params.using_qmatrix) {
        svt_aom_wb_write_literal(wb, frm_hdr->quantization_params.qm[AOM_PLANE_Y], QM_LEVEL_BITS);
        svt_aom_wb_write_literal(wb, frm_hdr->quantization_params.qm[AOM_PLANE_U], QM_LEVEL_BITS);
        if (!diff_uv_delta) {
            assert(frm_hdr->quantization_params.qm[AOM_PLANE_U] == frm_hdr->quantization_params.qm[AOM_PLANE_V]);
        } else {
            svt_aom_wb_write_literal(wb, frm_hdr->quantization_params.qm[AOM_PLANE_V], QM_LEVEL_BITS);
        }
    }
}

static void write_tile_info_max_tile(const PictureParentControlSet* const pcs, AomWriteBitBuffer* wb) {
    Av1Common* cm = pcs->av1_cm;
    svt_aom_wb_write_bit(wb, cm->tiles_info.uniform_tile_spacing_flag);

    if (cm->tiles_info.uniform_tile_spacing_flag) {
        // Uniform spaced tiles with power-of-two number of rows and columns
        // tile columns
        int32_t ones = cm->log2_tile_cols - cm->tiles_info.min_log2_tile_cols;
        while (ones--) {
            svt_aom_wb_write_bit(wb, 1);
        }
        if (cm->log2_tile_cols < cm->tiles_info.max_log2_tile_cols) {
            svt_aom_wb_write_bit(wb, 0);
        }
        // rows
        cm->tiles_info.min_log2_tile_rows = AOMMAX(cm->tiles_info.min_log2_tiles - cm->log2_tile_cols, 0);
        ones                              = cm->log2_tile_rows - cm->tiles_info.min_log2_tile_rows;
        while (ones--) {
            svt_aom_wb_write_bit(wb, 1);
        }
        if (cm->log2_tile_rows < cm->tiles_info.max_log2_tile_rows) {
            svt_aom_wb_write_bit(wb, 0);
        }
    } else {
        // Explicit tiles with configurable tile widths and heights
        SVT_ERROR("NON uniform_tile_spacing_flag not supported yet\n");
        //// columns
        // int sb_size_log2 = pcs->scs->seq_header.sb_size_log2;
        //for (i = 0; i < cm->tile_cols; i++) {
        //    size_sb = (cm->tile_col_start_mi[i + 1] - cm->tile_col_start_mi[i]) >> sb_size_log2;
        //    wb_write_uniform(wb, AOMMIN(width_sb, cm->max_tile_width_sb),
        //        size_sb - 1);
        //    width_sb -= size_sb;
        //}
        //assert(width_sb == 0);

        //// rows
        //for (i = 0; i < cm->tile_rows; i++) {
        //    size_sb = (cm->tile_row_start_mi[i + 1] - cm->tile_row_start_mi[i]) >> sb_size_log2;
        //    wb_write_uniform(wb, AOMMIN(height_sb, cm->max_tile_height_sb),
        //        size_sb - 1);
        //    height_sb -= size_sb;
        //}
        //assert(height_sb == 0);
    }
}

void svt_av1_get_tile_limits(PictureParentControlSet* pcs) {
    Av1Common* cm = pcs->av1_cm;

    int32_t mi_cols                  = ALIGN_POWER_OF_TWO(cm->mi_cols, pcs->log2_sb_size);
    int32_t mi_rows                  = ALIGN_POWER_OF_TWO(cm->mi_rows, pcs->log2_sb_size);
    int32_t sb_cols                  = mi_cols >> pcs->log2_sb_size;
    int32_t sb_rows                  = mi_rows >> pcs->log2_sb_size;
    int32_t sb_size_log2             = pcs->log2_sb_size + MI_SIZE_LOG2;
    cm->tiles_info.max_tile_width_sb = MAX_TILE_WIDTH >> sb_size_log2;
    int32_t max_tile_area_sb         = MAX_TILE_AREA >> (2 * sb_size_log2);

    cm->tiles_info.min_log2_tile_cols = tile_log2(cm->tiles_info.max_tile_width_sb, sb_cols);
    cm->tiles_info.max_log2_tile_cols = tile_log2(1, AOMMIN(sb_cols, MAX_TILE_COLS));
    cm->tiles_info.max_log2_tile_rows = tile_log2(1, AOMMIN(sb_rows, MAX_TILE_ROWS));
    cm->tiles_info.min_log2_tile_rows = 0; // CHKN Tiles
    cm->tiles_info.min_log2_tiles     = tile_log2(max_tile_area_sb, sb_cols * sb_rows);
    cm->tiles_info.min_log2_tiles     = AOMMAX(cm->tiles_info.min_log2_tiles, cm->tiles_info.min_log2_tile_cols);
}

void svt_av1_calculate_tile_cols(PictureParentControlSet* pcs) {
    Av1Common* const cm = pcs->av1_cm;

    const int mi_cols      = ALIGN_POWER_OF_TWO(cm->mi_cols, pcs->log2_sb_size);
    const int mi_rows      = ALIGN_POWER_OF_TWO(cm->mi_rows, pcs->log2_sb_size);
    const int sb_cols      = mi_cols >> pcs->log2_sb_size;
    const int sb_rows      = mi_rows >> pcs->log2_sb_size;
    const int sb_size_log2 = pcs->log2_sb_size;

    if (cm->tiles_info.uniform_tile_spacing_flag) {
        int size_sb = ALIGN_POWER_OF_TWO(sb_cols, cm->log2_tile_cols);
        size_sb >>= cm->log2_tile_cols;
        assert(size_sb > 0);
        int i = 0;
        for (int start_sb = 0; start_sb < sb_cols; i++) {
            cm->tiles_info.tile_col_start_mi[i] = start_sb << sb_size_log2;
            start_sb += size_sb;
        }
        cm->tiles_info.tile_cols            = i;
        cm->tiles_info.tile_col_start_mi[i] = sb_cols << sb_size_log2;
        cm->tiles_info.min_log2_tile_rows   = AOMMAX(cm->tiles_info.min_log2_tiles - cm->log2_tile_cols, 0);
        cm->tiles_info.max_tile_height_sb   = sb_rows >> cm->tiles_info.min_log2_tile_rows;

        cm->tile_width = size_sb << pcs->log2_sb_size;
        cm->tile_width = AOMMIN(cm->tile_width, cm->mi_cols);
    } else {
        int max_tile_area_sb = (sb_rows * sb_cols);
        int widest_tile_sb   = 1;
        cm->log2_tile_cols   = tile_log2(1, cm->tiles_info.tile_cols);
        for (int i = 0; i < cm->tiles_info.tile_cols; i++) {
            int size_sb = (cm->tiles_info.tile_col_start_mi[i + 1] - cm->tiles_info.tile_col_start_mi[i]) >>
                sb_size_log2;
            widest_tile_sb = AOMMAX(widest_tile_sb, size_sb);
        }
        if (cm->tiles_info.min_log2_tiles) {
            max_tile_area_sb >>= (cm->tiles_info.min_log2_tiles + 1);
        }

        cm->tiles_info.max_tile_height_sb = AOMMAX(max_tile_area_sb / widest_tile_sb, 1);
    }
}

void svt_av1_calculate_tile_rows(PictureParentControlSet* pcs) {
    Av1Common* const cm = pcs->av1_cm;

    int mi_rows      = ALIGN_POWER_OF_TWO(cm->mi_rows, pcs->log2_sb_size);
    int sb_rows      = mi_rows >> pcs->log2_sb_size;
    int sb_size_log2 = pcs->log2_sb_size;

    if (cm->tiles_info.uniform_tile_spacing_flag) {
        int size_sb = ALIGN_POWER_OF_TWO(sb_rows, cm->log2_tile_rows);
        size_sb >>= cm->log2_tile_rows;
        assert(size_sb > 0);
        int i = 0;
        for (int start_sb = 0; start_sb < sb_rows; i++) {
            cm->tiles_info.tile_row_start_mi[i] = start_sb << sb_size_log2;
            start_sb += size_sb;
        }
        cm->tiles_info.tile_rows            = i;
        cm->tiles_info.tile_row_start_mi[i] = sb_rows << sb_size_log2;

        cm->tile_height = size_sb << pcs->log2_sb_size;
        cm->tile_height = AOMMIN(cm->tile_height, cm->mi_rows);
    } else {
        cm->log2_tile_rows = tile_log2(1, cm->tiles_info.tile_rows);
    }
}

void svt_aom_set_tile_info(PictureParentControlSet* pcs) {
    /*  Tiling algorithm:
        input : log2_tile_count ==> tile_count = 1<<log2_tile_count

        step1) compute pic_size_in_sb
        step2) then round up to the closed n.tile_count.
        step3) tile_size = rounded_pic_size_in_sb / tile_count.
        step4) we fill tiles of size tile_size until we reach the end of the pic

        Note that: the last tile could have smaller size, and the final number
        of tiles could be less than tile_count
     */

    Av1Common* cm = pcs->av1_cm;
    //to connect later if non uniform tile spacing is needed.

    svt_av1_get_tile_limits(pcs);

    // configure tile columns
    cm->tiles_info.uniform_tile_spacing_flag = 1;
    cm->log2_tile_cols                       = AOMMAX(pcs->log2_tile_cols, cm->tiles_info.min_log2_tile_cols);
    cm->log2_tile_cols                       = AOMMIN(cm->log2_tile_cols, cm->tiles_info.max_log2_tile_cols);

    svt_av1_calculate_tile_cols(pcs);

    // configure tile rows
    if (cm->tiles_info.uniform_tile_spacing_flag) {
        cm->log2_tile_rows = AOMMAX(pcs->log2_tile_rows, cm->tiles_info.min_log2_tile_rows);
        cm->log2_tile_rows = AOMMIN(cm->log2_tile_rows, cm->tiles_info.max_log2_tile_rows);
    } else {
        int       i            = 0;
        const int mi_rows      = ALIGN_POWER_OF_TWO(cm->mi_rows, pcs->log2_sb_size);
        const int sb_rows      = mi_rows >> pcs->log2_sb_size;
        const int sb_size_log2 = pcs->scs->seq_header.sb_size_log2;
        for (int start_sb = 0; start_sb < sb_rows && i < MAX_TILE_ROWS; i++) {
            cm->tiles_info.tile_row_start_mi[i] = start_sb << sb_size_log2;
            start_sb += cm->tiles_info.max_tile_height_sb;
        }
        cm->tiles_info.tile_rows            = i;
        cm->tiles_info.tile_row_start_mi[i] = sb_rows << sb_size_log2;
    }
    svt_av1_calculate_tile_rows(pcs);
}

static void write_tile_info(const PictureParentControlSet* const pcs, AomWriteBitBuffer* wb) {
    Av1Common* const cm                     = pcs->av1_cm;
    uint16_t         tile_cnt               = cm->tiles_info.tile_rows * cm->tiles_info.tile_cols;
    pcs->child_pcs->tile_size_bytes_minus_1 = 0;
    svt_av1_get_tile_limits((PictureParentControlSet*)pcs);
    write_tile_info_max_tile(pcs, wb);

    if (pcs->av1_cm->tiles_info.tile_rows * pcs->av1_cm->tiles_info.tile_cols > 1) {
        // tile id used for cdf update
        // Force each frame to update their data so future frames can use it,
        // even if the current frame did not use it.  This enables REF frames to
        // have the feature off, while NREF frames can have it on.  Used for multi-threading.
        svt_aom_wb_write_literal(wb,
                                 pcs->av1_cm->tiles_info.tile_rows * pcs->av1_cm->tiles_info.tile_cols - 1,
                                 pcs->av1_cm->log2_tile_cols + pcs->av1_cm->log2_tile_rows);

        // Number of bytes in tile size - 1
        uint32_t max_tile_size = 0;
        for (int tile_idx = 0; tile_idx < tile_cnt - 1; tile_idx++) {
            max_tile_size = AOMMAX(max_tile_size, pcs->child_pcs->ec_info[tile_idx]->ec->ec_writer.pos);
        }
        if (max_tile_size >> 24 != 0) {
            pcs->child_pcs->tile_size_bytes_minus_1 = 3;
        } else if (max_tile_size >> 16 != 0) {
            pcs->child_pcs->tile_size_bytes_minus_1 = 2;
        } else if (max_tile_size >> 8 != 0) {
            pcs->child_pcs->tile_size_bytes_minus_1 = 1;
        } else {
            pcs->child_pcs->tile_size_bytes_minus_1 = 0;
        }

        svt_aom_wb_write_literal(wb, pcs->child_pcs->tile_size_bytes_minus_1, 2); //Jing: Change 3 to smaller size
    }
}

static AOM_INLINE void write_render_size(AomWriteBitBuffer* wb, PictureParentControlSet* ppcs) {
    int render_and_frame_size_different = 0;
    if (ppcs->frame_resize_enabled) {
        render_and_frame_size_different = 1;
    }
    svt_aom_wb_write_bit(wb, render_and_frame_size_different);
    if (!render_and_frame_size_different) {
        return;
    }
    uint32_t render_width_minus_1  = ppcs->render_width - 1;
    uint32_t render_height_minus_1 = ppcs->render_height - 1;
    svt_aom_wb_write_literal(wb, render_width_minus_1, 16);
    svt_aom_wb_write_literal(wb, render_height_minus_1, 16);
}

static AOM_INLINE void write_superres_scale(AomWriteBitBuffer* wb, PictureParentControlSet* pcs) {
    SequenceControlSet* scs            = pcs->scs;
    Av1Common*          cm             = pcs->av1_cm;
    uint8_t             superres_denom = cm->frm_size.superres_denominator;

    if (!scs->seq_header.enable_superres) {
        assert(cm->frm_size.superres_denominator == SCALE_NUMERATOR);
        return;
    }

    // First bit is whether to to scale or not
    if (superres_denom == SCALE_NUMERATOR) {
        svt_aom_wb_write_bit(wb, 0); // no scaling
    } else {
        svt_aom_wb_write_bit(wb, 1); // scaling, write scale factor
        assert(superres_denom >= SUPERRES_SCALE_DENOMINATOR_MIN);
        assert(superres_denom < SUPERRES_SCALE_DENOMINATOR_MIN + (1 << SUPERRES_SCALE_BITS));
        svt_aom_wb_write_literal(wb, superres_denom - SUPERRES_SCALE_DENOMINATOR_MIN, SUPERRES_SCALE_BITS);
    }
}

static void write_frame_size(PictureParentControlSet* pcs, int32_t frame_size_override, AomWriteBitBuffer* wb) {
    SequenceControlSet* scs = pcs->scs;
    (void)(*pcs);
    (void)frame_size_override;
    Av1Common*    cm           = pcs->av1_cm;
    const int32_t coded_width  = cm->frm_size.superres_upscaled_width - 1;
    const int32_t coded_height = cm->frm_size.superres_upscaled_height - 1;

    if (frame_size_override) {
        int32_t num_bits_width  = scs->seq_header.frame_width_bits;
        int32_t num_bits_height = scs->seq_header.frame_height_bits;
        svt_aom_wb_write_literal(wb, coded_width, num_bits_width);
        svt_aom_wb_write_literal(wb, coded_height, num_bits_height);
    }

    write_superres_scale(wb, pcs);
    write_render_size(wb, pcs);
}

static void write_profile(BitstreamProfile profile, AomWriteBitBuffer* wb) {
    assert(profile >= PROFILE_0 && profile < MAX_PROFILES);
    svt_aom_wb_write_literal(wb, profile, PROFILE_BITS);
}

static AOM_INLINE void write_bitdepth(const SequenceControlSet* const scs, AomWriteBitBuffer* wb) {
    // Profile 0/1: [0] for 8 bit, [1]  10-bit
    // Profile   2: [0] for 8 bit, [10] 10-bit, [11] - 12-bit
    svt_aom_wb_write_bit(wb, scs->static_config.encoder_bit_depth == EB_EIGHT_BIT ? 0 : 1);
    if (scs->static_config.profile == PROFESSIONAL_PROFILE && scs->static_config.encoder_bit_depth != EB_EIGHT_BIT) {
        SVT_ERROR("Profile 2 Not supported\n");
        svt_aom_wb_write_bit(wb, scs->static_config.encoder_bit_depth == EB_TEN_BIT ? 0 : 1);
    }
}

static AOM_INLINE void write_color_config(const SequenceControlSet* const scs, AomWriteBitBuffer* wb) {
    write_bitdepth(scs, wb);
    const int is_monochrome = 0; // monochrome is not supported yet
    // monochrome bit
    if (scs->static_config.profile != HIGH_PROFILE) {
        svt_aom_wb_write_bit(wb, is_monochrome);
    } else {
        assert(!is_monochrome);
    }
    if (scs->static_config.color_primaries == EB_CICP_CP_UNSPECIFIED &&
        scs->static_config.transfer_characteristics == EB_CICP_TC_UNSPECIFIED &&
        scs->static_config.matrix_coefficients == EB_CICP_MC_UNSPECIFIED) {
        svt_aom_wb_write_bit(wb, 0); // No color description present
    } else {
        svt_aom_wb_write_bit(wb, 1); // Color description present
        svt_aom_wb_write_literal(wb, scs->static_config.color_primaries, 8);
        svt_aom_wb_write_literal(wb, scs->static_config.transfer_characteristics, 8);
        svt_aom_wb_write_literal(wb, scs->static_config.matrix_coefficients, 8);
    }
    /* if (is_monochrome) {
        // 0: [16, 235] (i.e. xvYCC), 1: [0, 255]
        svt_aom_wb_write_bit(wb, scs->static_config.color_range);
        return;
    } */
    if (scs->static_config.color_primaries == EB_CICP_CP_BT_709 &&
        scs->static_config.transfer_characteristics == EB_CICP_TC_SRGB &&
        scs->static_config.matrix_coefficients == EB_CICP_MC_IDENTITY) {
        /* assert(scs->subsampling_x == 0 && scs->subsampling_y == 0);
        assert(scs->static_config.profile == HIGH_PROFILE ||
               (scs->static_config.profile == PROFESSIONAL_PROFILE && scs->encoder_bit_depth == EB_TWELVE_BIT)); */
    } else {
        // 0: [16, 235] (i.e. xvYCC), 1: [0, 255]
        svt_aom_wb_write_bit(wb, scs->static_config.color_range);
        if (scs->static_config.profile == MAIN_PROFILE) {
            // 420 only
            assert(scs->subsampling_x == 1 && scs->subsampling_y == 1);
        } else if (scs->static_config.profile == HIGH_PROFILE) {
            // 444 only
            assert(scs->subsampling_x == 0 && scs->subsampling_y == 0);
        } else if (scs->static_config.profile == PROFESSIONAL_PROFILE) {
            if (scs->encoder_bit_depth == EB_TWELVE_BIT) {
                // 420, 444 or 422
                svt_aom_wb_write_bit(wb, scs->subsampling_x);
                if (scs->subsampling_x == 0) {
                    assert(scs->subsampling_y == 0 && "4:4:0 subsampling not allowed in AV1");
                } else {
                    svt_aom_wb_write_bit(wb, scs->subsampling_y);
                }
            } else {
                // 422 only
                assert(scs->subsampling_x == 1 && scs->subsampling_y == 0);
            }
        }
        if (scs->static_config.matrix_coefficients == EB_CICP_MC_IDENTITY) {
            assert(scs->subsampling_x == 0 && scs->subsampling_y == 0);
        }
        if (scs->subsampling_x == 1 && scs->subsampling_y == 1) {
            svt_aom_wb_write_literal(wb, scs->static_config.chroma_sample_position, 2);
        }
    }
    bool separate_uv_delta_q = (scs->static_config.chroma_u_ac_qindex_offset !=
                                    scs->static_config.chroma_v_ac_qindex_offset ||
                                scs->static_config.chroma_u_dc_qindex_offset !=
                                    scs->static_config.chroma_v_dc_qindex_offset);
    svt_aom_wb_write_bit(wb, separate_uv_delta_q);
}

static void write_sequence_header(SequenceControlSet* scs, AomWriteBitBuffer* wb) {
    const int32_t max_frame_width   = scs->seq_header.max_frame_width;
    const int32_t max_frame_height  = scs->seq_header.max_frame_height;
    unsigned      frame_width_bits  = svt_log2f(max_frame_width);
    unsigned      frame_height_bits = svt_log2f(max_frame_height);
    if (max_frame_width > (1 << frame_width_bits)) {
        ++frame_width_bits;
    }
    if (max_frame_height > (1 << frame_height_bits)) {
        ++frame_height_bits;
    }
    scs->seq_header.frame_width_bits  = frame_width_bits;
    scs->seq_header.frame_height_bits = frame_height_bits;

    svt_aom_wb_write_literal(wb, frame_width_bits - 1, 4);
    svt_aom_wb_write_literal(wb, frame_height_bits - 1, 4);
    svt_aom_wb_write_literal(wb, max_frame_width - 1, frame_width_bits);
    svt_aom_wb_write_literal(wb, max_frame_height - 1, frame_height_bits);

    if (!scs->seq_header.reduced_still_picture_header) {
        //scs->frame_id_numbers_present_flag = 0;
        //    cm->large_scale_tile ? 0 : cm->error_resilient_mode;

        svt_aom_wb_write_bit(wb, scs->seq_header.frame_id_numbers_present_flag);
        if (scs->seq_header.frame_id_numbers_present_flag) {
            // We must always have delta_frame_id_length < frame_id_length,
            // in order for a frame to be referenced with a unique delta.
            // Avoid wasting bits by using a coding that enforces this restriction.
            svt_aom_wb_write_literal(wb, scs->seq_header.delta_frame_id_length - 2, 4);
            svt_aom_wb_write_literal(
                wb, ((scs->seq_header.frame_id_length) - (scs->seq_header.delta_frame_id_length) - 1), 3);
        }
    }

    svt_aom_wb_write_bit(wb, scs->seq_header.sb_size == BLOCK_128X128 ? 1 : 0);
    //    svt_aom_write_sb_size(seq_params, wb);
    svt_aom_wb_write_bit(wb, scs->seq_header.filter_intra_level);
    svt_aom_wb_write_bit(wb, scs->seq_header.enable_intra_edge_filter);

    if (!scs->seq_header.reduced_still_picture_header) {
        svt_aom_wb_write_bit(wb, scs->seq_header.enable_interintra_compound);
        svt_aom_wb_write_bit(wb, scs->seq_header.enable_masked_compound);
        //        svt_aom_wb_write_bit(wb, scs->static_config.enable_warped_motion);
        svt_aom_wb_write_bit(wb, scs->seq_header.enable_warped_motion);
        svt_aom_wb_write_bit(wb, scs->seq_header.enable_dual_filter);

        svt_aom_wb_write_bit(wb, scs->seq_header.order_hint_info.enable_order_hint);

        if (scs->seq_header.order_hint_info.enable_order_hint) {
            svt_aom_wb_write_bit(wb, scs->seq_header.order_hint_info.enable_jnt_comp);
            svt_aom_wb_write_bit(wb, scs->seq_header.order_hint_info.enable_ref_frame_mvs);
        }

        if (scs->seq_header.seq_force_screen_content_tools == 2) {
            svt_aom_wb_write_bit(wb, 1);
        } else {
            svt_aom_wb_write_bit(wb, 0);
            svt_aom_wb_write_bit(wb, scs->seq_header.seq_force_screen_content_tools);
        }
        //
        if (scs->seq_header.seq_force_screen_content_tools > 0) {
            if (scs->seq_header.seq_force_integer_mv == 2) {
                svt_aom_wb_write_bit(wb, 1);
            } else {
                svt_aom_wb_write_bit(wb, 0);
                svt_aom_wb_write_bit(wb, scs->seq_header.seq_force_integer_mv);
            }
        } else {
            assert(scs->seq_header.seq_force_integer_mv == 2);
        }
        if (scs->seq_header.order_hint_info.enable_order_hint) {
            svt_aom_wb_write_literal(wb, scs->seq_header.order_hint_info.order_hint_bits - 1, 3);
        }
    }

    svt_aom_wb_write_bit(wb, scs->seq_header.enable_superres);
    svt_aom_wb_write_bit(wb, scs->seq_header.cdef_level);
    svt_aom_wb_write_bit(wb, scs->seq_header.enable_restoration);
}

// Recenters a non-negative literal v around a reference r
static uint16_t recenter_nonneg(uint16_t r, uint16_t v) {
    if (v > (r << 1)) {
        return v;
    } else if (v >= r) {
        return ((v - r) << 1);
    } else {
        return ((r - v) << 1) - 1;
    }
}

// Recenters a non-negative literal v in [0, n-1] around a
// reference r also in [0, n-1]
static uint16_t recenter_finite_nonneg(uint16_t n, uint16_t r, uint16_t v) {
    if ((r << 1) <= n) {
        return recenter_nonneg(r, v);
    } else {
        return recenter_nonneg(n - 1 - r, n - 1 - v);
    }
}

// Encodes a value v in [0, n-1] quasi-uniformly
void svt_aom_write_primitive_quniform(AomWriter* w, uint16_t n, uint16_t v) {
    if (n <= 1) {
        return;
    }
    const int32_t l = get_msb(n - 1) + 1;
    const int32_t m = (1 << l) - n;
    if (v < m) {
        aom_write_literal(w, v, l - 1);
    } else {
        aom_write_literal(w, m + ((v - m) >> 1), l - 1);
        aom_write_bit(w, (v - m) & 1);
    }
}

static void aom_wb_write_primitive_quniform(AomWriteBitBuffer* wb, uint16_t n, uint16_t v) {
    if (n <= 1) {
        return;
    }
    const int32_t l = get_msb(n - 1) + 1;
    const int32_t m = (1 << l) - n;
    if (v < m) {
        svt_aom_wb_write_literal(wb, v, l - 1);
    } else {
        svt_aom_wb_write_literal(wb, m + ((v - m) >> 1), l - 1);
        svt_aom_wb_write_bit(wb, (v - m) & 1);
    }
}

int32_t svt_aom_count_primitive_quniform(uint16_t n, uint16_t v) {
    if (n <= 1) {
        return 0;
    }
    const int32_t l = get_msb(n - 1) + 1;
    const int32_t m = (1 << l) - n;
    return v < m ? l - 1 : l;
}

// Finite subexponential code that codes a symbol v in [0, n-1] with parameter k
void svt_aom_write_primitive_subexpfin(AomWriter* w, uint16_t n, uint16_t k, uint16_t v) {
    int32_t i  = 0;
    int32_t mk = 0;
    while (1) {
        int32_t b = (i ? k + i - 1 : k);
        int32_t a = (1 << b);
        if (n <= mk + 3 * a) {
            svt_aom_write_primitive_quniform(w, (uint16_t)(n - mk), (uint16_t)(v - mk));
            break;
        } else {
            int32_t t = (v >= mk + a);
            aom_write_bit(w, t);
            if (t) {
                i = i + 1;
                mk += a;
            } else {
                aom_write_literal(w, v - mk, b);
                break;
            }
        }
    }
}

static void aom_wb_write_primitive_subexpfin(AomWriteBitBuffer* wb, uint16_t n, uint16_t k, uint16_t v) {
    int32_t i  = 0;
    int32_t mk = 0;
    while (1) {
        int32_t b = (i ? k + i - 1 : k);
        int32_t a = (1 << b);
        if (n <= mk + 3 * a) {
            aom_wb_write_primitive_quniform(wb, (uint16_t)(n - mk), (uint16_t)(v - mk));
            break;
        } else {
            int32_t t = (v >= mk + a);
            svt_aom_wb_write_bit(wb, t);
            if (t) {
                i = i + 1;
                mk += a;
            } else {
                svt_aom_wb_write_literal(wb, v - mk, b);
                break;
            }
        }
    }
}

int32_t svt_aom_count_primitive_subexpfin(uint16_t n, uint16_t k, uint16_t v) {
    int32_t count = 0;
    int32_t i     = 0;
    int32_t mk    = 0;
    while (1) {
        int32_t b = (i ? k + i - 1 : k);
        int32_t a = (1 << b);
        if (n <= mk + 3 * a) {
            count += svt_aom_count_primitive_quniform((uint16_t)(n - mk), (uint16_t)(v - mk));
            break;
        } else {
            int32_t t = (v >= mk + a);
            count++;
            if (t) {
                i = i + 1;
                mk += a;
            } else {
                count += b;
                break;
            }
        }
    }
    return count;
}

// Finite subexponential code that codes a symbol v in[0, n - 1] with parameter k
// based on a reference ref also in [0, n-1].
// Recenters symbol around r first and then uses a finite subexponential code.
void svt_aom_write_primitive_refsubexpfin(AomWriter* w, uint16_t n, uint16_t k, uint16_t ref, uint16_t v) {
    svt_aom_write_primitive_subexpfin(w, n, k, recenter_finite_nonneg(n, ref, v));
}

static void aom_wb_write_primitive_refsubexpfin(AomWriteBitBuffer* wb, uint16_t n, uint16_t k, uint16_t ref,
                                                uint16_t v) {
    aom_wb_write_primitive_subexpfin(wb, n, k, recenter_finite_nonneg(n, ref, v));
}

void svt_aom_wb_write_signed_primitive_refsubexpfin(AomWriteBitBuffer* wb, uint16_t n, uint16_t k, int16_t ref,
                                                    int16_t v) {
    ref += n - 1;
    v += n - 1;
    const uint16_t scaled_n = (n << 1) - 1;
    aom_wb_write_primitive_refsubexpfin(wb, scaled_n, k, ref, v);
}

int32_t svt_aom_count_primitive_refsubexpfin(uint16_t n, uint16_t k, uint16_t ref, uint16_t v) {
    return svt_aom_count_primitive_subexpfin(n, k, recenter_finite_nonneg(n, ref, v));
}

static void write_global_motion_params(const WarpedMotionParams* params, const WarpedMotionParams* ref_params,
                                       AomWriteBitBuffer* wb, int32_t allow_hp) {
    const TransformationType type = params->wmtype;
    svt_aom_wb_write_bit(wb, type != IDENTITY);
    if (type != IDENTITY) {
        svt_aom_wb_write_bit(wb, type == ROTZOOM);
        if (type != ROTZOOM) {
            svt_aom_wb_write_bit(wb, type == TRANSLATION);
        }
    }

    if (type >= ROTZOOM) {
        int16_t ref2 = (int16_t)((ref_params->wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS));
        int16_t v2   = (int16_t)((params->wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS));

        int16_t ref3 = (int16_t)(ref_params->wmmat[3] >> GM_ALPHA_PREC_DIFF);
        int16_t v3   = (int16_t)(params->wmmat[3] >> GM_ALPHA_PREC_DIFF);

        svt_aom_wb_write_signed_primitive_refsubexpfin(
            wb,
            GM_ALPHA_MAX + 1,
            SUBEXPFIN_K,
            ref2 /*(ref_params->wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)*/,
            v2 /*(int16_t)((params->wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS))*/);
        svt_aom_wb_write_signed_primitive_refsubexpfin(wb,
                                                       GM_ALPHA_MAX + 1,
                                                       SUBEXPFIN_K,
                                                       ref3 /*(ref_params->wmmat[3] >> GM_ALPHA_PREC_DIFF)*/,
                                                       v3 /*(int16_t)(params->wmmat[3] >> GM_ALPHA_PREC_DIFF)*/);
    }

    if (type >= AFFINE) {
        int16_t ref4 = (int16_t)(ref_params->wmmat[4] >> GM_ALPHA_PREC_DIFF);
        int16_t v4   = (int16_t)(params->wmmat[4] >> GM_ALPHA_PREC_DIFF);

        int16_t ref5 = (int16_t)((ref_params->wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS));
        int16_t v5   = (int16_t)((params->wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS));

        svt_aom_wb_write_signed_primitive_refsubexpfin(wb,
                                                       GM_ALPHA_MAX + 1,
                                                       SUBEXPFIN_K,
                                                       ref4 /*(ref_params->wmmat[4] >> GM_ALPHA_PREC_DIFF)*/,
                                                       v4 /*(int16_t)(params->wmmat[4] >> GM_ALPHA_PREC_DIFF)*/);
        svt_aom_wb_write_signed_primitive_refsubexpfin(
            wb,
            GM_ALPHA_MAX + 1,
            SUBEXPFIN_K,
            ref5 /*(ref_params->wmmat[5] >> GM_ALPHA_PREC_DIFF) -    (1 << GM_ALPHA_PREC_BITS)*/,
            v5 /*(int16_t)(params->wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)*/);
    }

    if (type >= TRANSLATION) {
        const int32_t trans_bits      = (type == TRANSLATION) ? GM_ABS_TRANS_ONLY_BITS - !allow_hp : GM_ABS_TRANS_BITS;
        const int32_t trans_prec_diff = (type == TRANSLATION) ? GM_TRANS_ONLY_PREC_DIFF + !allow_hp
                                                              : GM_TRANS_PREC_DIFF;
        svt_aom_wb_write_signed_primitive_refsubexpfin(wb,
                                                       (1 << trans_bits) + 1,
                                                       SUBEXPFIN_K,
                                                       (int16_t)(ref_params->wmmat[0] >> trans_prec_diff),
                                                       (int16_t)(params->wmmat[0] >> trans_prec_diff));
        svt_aom_wb_write_signed_primitive_refsubexpfin(wb,
                                                       (1 << trans_bits) + 1,
                                                       SUBEXPFIN_K,
                                                       (int16_t)(ref_params->wmmat[1] >> trans_prec_diff),
                                                       (int16_t)(params->wmmat[1] >> trans_prec_diff));
    }
}

static void write_global_motion(PictureParentControlSet* pcs, AomWriteBitBuffer* wb)

{
    int32_t      frame;
    FrameHeader* frm_hdr = &pcs->frm_hdr;
    for (frame = LAST_FRAME; frame <= ALTREF_FRAME; ++frame) {
        const WarpedMotionParams* ref_params = (frm_hdr->primary_ref_frame != PRIMARY_REF_NONE)
            ? &pcs->child_pcs->ref_global_motion[frame]
            : &default_warp_params;
        write_global_motion_params(&pcs->global_motion[frame], ref_params, wb, frm_hdr->allow_high_precision_mv);
        // The logic in the commented out code below
        // does not work currently and causes mismatches when resize is on.
        // Fix it before turning the optimization back on.
        /*
        Yv12BufferConfig *ref_buf = get_ref_frame_buffer(cpi, frame);
        if (cpi->source->y_crop_width == ref_buf->y_crop_width &&
        cpi->source->y_crop_height == ref_buf->y_crop_height) {
        write_global_motion_params(&cm->global_motion[frame],
        &cm->prev_frame->global_motion[frame], wb,
        cm->allow_high_precision_mv);
        } else {
        assert(cm->global_motion[frame].wmtype == IDENTITY &&
        "Invalid warp type for frames of different resolutions");
        }
        */
        /*
        SVT_LOG("Frame %d/%d: Enc Ref %d: %d %d %d %d\n",
        cm->current_video_frame, cm->show_frame, frame,
        cm->global_motion[frame].wmmat[0],
        cm->global_motion[frame].wmmat[1], cm->global_motion[frame].wmmat[2],
        cm->global_motion[frame].wmmat[3]);
        */
    }
}

#if CONFIG_ENABLE_FILM_GRAIN
static void write_film_grain_params(PictureParentControlSet* pcs, AomWriteBitBuffer* wb) {
    FrameHeader*  frm_hdr = &pcs->frm_hdr;
    AomFilmGrain* pars    = &frm_hdr->film_grain_params;

    svt_aom_wb_write_bit(wb, pars->apply_grain);
    if (!pars->apply_grain) {
        return;
    }

    svt_aom_wb_write_literal(wb, pars->random_seed, 16);

    if (frm_hdr->frame_type == INTER_FRAME) {
        EbReferenceObject* ref_obj_0 = (EbReferenceObject*)pcs->child_pcs->ref_pic_ptr_array[REF_LIST_0][0]->object_ptr;
        int32_t            ref_idx   = 0;
        pars->update_parameters      = 1;

        if (!pars->ignore_ref) {
            if (svt_aom_film_grain_params_equal(&ref_obj_0->film_grain_params, pars)) {
                pars->update_parameters = 0;
                ref_idx                 = get_ref_frame_map_idx(pcs, LAST_FRAME);
            } else if (pcs->child_pcs->slice_type == B_SLICE) {
                EbReferenceObject* ref_obj_1 =
                    (EbReferenceObject*)pcs->child_pcs->ref_pic_ptr_array[REF_LIST_1][0]->object_ptr;
                if (svt_aom_film_grain_params_equal(&ref_obj_1->film_grain_params, pars)) {
                    pars->update_parameters = 0;
                    ref_idx = get_ref_frame_map_idx(pcs, ALTREF_FRAME); //todo: will it always be ALF_REF in L1?
                }
            }
        }

        svt_aom_wb_write_bit(wb, pars->update_parameters);
        if (!pars->update_parameters) {
            svt_aom_wb_write_literal(wb, ref_idx, 3);
            return;
        }
    } else {
        pars->update_parameters = 1;
    }

    // Scaling functions parameters
    svt_aom_wb_write_literal(wb, pars->num_y_points, 4); // max 14
    for (int32_t i = 0; i < pars->num_y_points; i++) {
        svt_aom_wb_write_literal(wb, pars->scaling_points_y[i][0], 8);
        svt_aom_wb_write_literal(wb, pars->scaling_points_y[i][1], 8);
    }

    if (!pcs->scs->seq_header.color_config.mono_chrome) {
        svt_aom_wb_write_bit(wb, pars->chroma_scaling_from_luma);
    } else {
        pars->chroma_scaling_from_luma = 0; // for monochrome override to 0
    }

    if (pcs->scs->seq_header.color_config.mono_chrome || pars->chroma_scaling_from_luma ||
        // todo: add corresponding check when subsampling variables are present
        ((pcs->scs->subsampling_x == 1) && (pcs->scs->subsampling_y == 1) && (pars->num_y_points == 0))) {
        pars->num_cb_points = 0;
        pars->num_cr_points = 0;
    } else {
        svt_aom_wb_write_literal(wb, pars->num_cb_points, 4); // max 10
        for (int32_t i = 0; i < pars->num_cb_points; i++) {
            svt_aom_wb_write_literal(wb, pars->scaling_points_cb[i][0], 8);
            svt_aom_wb_write_literal(wb, pars->scaling_points_cb[i][1], 8);
        }

        svt_aom_wb_write_literal(wb, pars->num_cr_points, 4); // max 10
        for (int32_t i = 0; i < pars->num_cr_points; i++) {
            svt_aom_wb_write_literal(wb, pars->scaling_points_cr[i][0], 8);
            svt_aom_wb_write_literal(wb, pars->scaling_points_cr[i][1], 8);
        }
    }

    svt_aom_wb_write_literal(wb, pars->scaling_shift - 8, 2); // 8 + value

    // AR coefficients
    // Only sent if the corresponsing scaling function has
    // more than 0 points

    svt_aom_wb_write_literal(wb, pars->ar_coeff_lag, 2);

    int32_t num_pos_luma   = 2 * pars->ar_coeff_lag * (pars->ar_coeff_lag + 1);
    int32_t num_pos_chroma = num_pos_luma;
    if (pars->num_y_points > 0) {
        ++num_pos_chroma;
    }

    if (pars->num_y_points) {
        for (int32_t i = 0; i < num_pos_luma; i++) {
            svt_aom_wb_write_literal(wb, pars->ar_coeffs_y[i] + 128, 8);
        }
    }

    if (pars->num_cb_points || pars->chroma_scaling_from_luma) {
        for (int32_t i = 0; i < num_pos_chroma; i++) {
            svt_aom_wb_write_literal(wb, pars->ar_coeffs_cb[i] + 128, 8);
        }
    }

    if (pars->num_cr_points || pars->chroma_scaling_from_luma) {
        for (int32_t i = 0; i < num_pos_chroma; i++) {
            svt_aom_wb_write_literal(wb, pars->ar_coeffs_cr[i] + 128, 8);
        }
    }

    svt_aom_wb_write_literal(wb, pars->ar_coeff_shift - 6, 2); // 8 + value

    svt_aom_wb_write_literal(wb, pars->grain_scale_shift, 2);

    if (pars->num_cb_points) {
        svt_aom_wb_write_literal(wb, pars->cb_mult, 8);
        svt_aom_wb_write_literal(wb, pars->cb_luma_mult, 8);
        svt_aom_wb_write_literal(wb, pars->cb_offset, 9);
    }

    if (pars->num_cr_points) {
        svt_aom_wb_write_literal(wb, pars->cr_mult, 8);
        svt_aom_wb_write_literal(wb, pars->cr_luma_mult, 8);
        svt_aom_wb_write_literal(wb, pars->cr_offset, 9);
    }

    svt_aom_wb_write_bit(wb, pars->overlap_flag);

    svt_aom_wb_write_bit(wb, pars->clip_to_restricted_range);
}
#endif

static uint32_t get_ref_order_hint(PictureParentControlSet* pcs, MvReferenceFrame ref_frame) {
    int32_t ref_idx = get_ref_frame_map_idx(pcs, ref_frame);
    if (ref_idx == INVALID_IDX) {
        return INVALID_IDX;
    }
    return pcs->dpb_order_hint[ref_idx];
}

static void write_frame_size_with_refs(PictureParentControlSet* pcs, AomWriteBitBuffer* wb) {
#if DEBUG_SFRAME
    fprintf(stderr,
            "\nFrame %d, dpb buf order hint %u,%u,%u,%u,%u,%u,%u\n",
            (int)pcs->picture_number,
            get_ref_order_hint(pcs, LAST_FRAME),
            get_ref_order_hint(pcs, LAST2_FRAME),
            get_ref_order_hint(pcs, LAST3_FRAME),
            get_ref_order_hint(pcs, GOLDEN_FRAME),
            get_ref_order_hint(pcs, BWDREF_FRAME),
            get_ref_order_hint(pcs, ALTREF2_FRAME),
            get_ref_order_hint(pcs, ALTREF_FRAME));
#endif
    for (uint32_t ref_frame = LAST_FRAME; ref_frame <= ALTREF_FRAME; ++ref_frame) {
        int32_t  found          = 0;
        uint32_t ref_order_hint = get_ref_order_hint(pcs, ref_frame);
        if ((int32_t)ref_order_hint != INVALID_IDX) {
            for (uint8_t i = 0; i < pcs->ref_list0_count; ++i) {
                EbReferenceObject* ref =
                    (EbReferenceObject*)pcs->child_pcs->ref_pic_ptr_array[REF_LIST_0][i]->object_ptr;
                if (ref->order_hint != ref_order_hint) {
                    continue;
                }
                // Both super-res upscaled size and render size should be checked as per spec 5.9.7,
                // but in current implementation, render_and_frame_size_different is fixed to 0, see
                // function write_render_size()
                found = pcs->enhanced_pic->width == ref->reference_picture->width &&
                    pcs->enhanced_pic->height == ref->reference_picture->height;
                if (found) {
                    break;
                }
            }
            if (!found) {
                for (uint8_t i = 0; i < pcs->ref_list1_count; ++i) {
                    EbReferenceObject* ref =
                        (EbReferenceObject*)pcs->child_pcs->ref_pic_ptr_array[REF_LIST_1][i]->object_ptr;
                    if (ref->order_hint != ref_order_hint) {
                        continue;
                    }
                    found = pcs->enhanced_pic->width == ref->reference_picture->width &&
                        pcs->enhanced_pic->height == ref->reference_picture->height;
                    if (found) {
                        break;
                    }
                }
            }
        }

        svt_aom_wb_write_bit(wb, found);
        if (found) {
            write_superres_scale(wb, pcs);
            return;
        }
    }

    // not found
    const int frame_size_override = 1; // Always equal to 1 in this function
    write_frame_size(pcs, frame_size_override, wb);
}

// New function based on HLS R18
static void write_uncompressed_header_obu(SequenceControlSet* scs /*Av1Comp *cpi*/, PictureParentControlSet* pcs,
                                          AomWriteBitBuffer* wb, uint8_t show_existing) {
    // Av1Common *const cm = &cpi->common;
    // MacroBlockD *const xd = &cpi->td.mb.e_mbd;
    Av1Common* const cm       = pcs->av1_cm;
    uint16_t         tile_cnt = cm->tiles_info.tile_rows * cm->tiles_info.tile_cols;

    FrameHeader* frm_hdr = &pcs->frm_hdr;
    if (!scs->seq_header.reduced_still_picture_header) {
        if (show_existing) {
            //SVT_ERROR("show_existing_frame not supported yet\n");
            //RefCntBuffer *const frame_bufs = cm->buffer_pool->frame_bufs;
            //const int32_t frame_to_show = cm->ref_frame_map[cpi->show_existing_frame];

            //if (frame_to_show < 0 || frame_bufs[frame_to_show].ref_count < 1) {
            //    aom_internal_error(&cm->error, AOM_CODEC_UNSUP_BITSTREAM,
            //        "Buffer %d does not contain a reconstructed frame",
            //        frame_to_show);
            //}
            //ref_cnt_fb(frame_bufs, &cm->new_fb_idx, frame_to_show);

            svt_aom_wb_write_bit(wb, 1); // show_existing_frame
            svt_aom_wb_write_literal(wb, frm_hdr->show_existing_frame, 3);
            if (scs->seq_header.frame_id_numbers_present_flag) {
                SVT_ERROR("frame_id_numbers_present_flag not supported yet\n");
                /*int32_t frame_id_len = cm->seq_params.frame_id_length;
                int32_t display_frame_id = cm->ref_frame_id[cpi->show_existing_frame];
                svt_aom_wb_write_literal(wb, display_frame_id, frame_id_len);*/
            }

            //        if (cm->reset_decoder_state &&
            //            frame_bufs[frame_to_show].frame_type != KEY_FRAME) {
            //            aom_internal_error(
            //                &cm->error, AOM_CODEC_UNSUP_BITSTREAM,
            //                "show_existing_frame to reset state on KEY_FRAME only");
            //        }

            return;
        } else {
            svt_aom_wb_write_bit(wb, 0); // show_existing_frame
        }

        svt_aom_wb_write_literal(wb, frm_hdr->frame_type, 2);

        svt_aom_wb_write_bit(wb, frm_hdr->show_frame);

        if (!frm_hdr->show_frame) {
            svt_aom_wb_write_bit(wb, frm_hdr->showable_frame);
        }
        if (frm_hdr->frame_type == S_FRAME) {
            assert(frm_hdr->error_resilient_mode);
        } else if (!(frm_hdr->frame_type == KEY_FRAME && frm_hdr->show_frame)) {
            svt_aom_wb_write_bit(wb, frm_hdr->error_resilient_mode);
        }
    }

    svt_aom_wb_write_bit(wb, frm_hdr->disable_cdf_update);

    if (scs->seq_header.seq_force_screen_content_tools == 2) {
        svt_aom_wb_write_bit(wb, frm_hdr->allow_screen_content_tools);
    } else {
        assert(frm_hdr->allow_screen_content_tools == scs->seq_header.seq_force_screen_content_tools);
    }

    if (frm_hdr->allow_screen_content_tools) {
        if (scs->seq_header.seq_force_integer_mv == 2) {
            svt_aom_wb_write_bit(wb, frm_hdr->force_integer_mv);
        } else {
            assert(frm_hdr->force_integer_mv == scs->seq_header.seq_force_integer_mv);
        }
    } else {
        assert(frm_hdr->force_integer_mv == 0);
    }

    const int32_t frame_size_override_flag = frame_is_sframe(pcs) || pcs->frame_resize_enabled
        ? 1
        : ((pcs->av1_cm->frm_size.superres_upscaled_width != scs->seq_header.max_frame_width) ||
           (pcs->av1_cm->frm_size.superres_upscaled_height != scs->seq_header.max_frame_height));

    if (!scs->seq_header.reduced_still_picture_header) {
        if (scs->seq_header.frame_id_numbers_present_flag) {
            int32_t frame_id_len = scs->seq_header.frame_id_length;
            svt_aom_wb_write_literal(wb, frm_hdr->current_frame_id, frame_id_len);
        }

        //if (cm->width > cm->seq_params.max_frame_width ||
        //    cm->height > cm->seq_params.max_frame_height) {
        //    aom_internal_error(&cm->error, AOM_CODEC_UNSUP_BITSTREAM,
        //        "Frame dimensions are larger than the maximum values");
        //}

        if (!frame_is_sframe(pcs)) {
            svt_aom_wb_write_bit(wb, frame_size_override_flag);
        }

        if (scs->seq_header.order_hint_info.enable_order_hint) {
            svt_aom_wb_write_literal(wb, (int32_t)pcs->frame_offset, scs->seq_header.order_hint_info.order_hint_bits);
        }

        if (!frm_hdr->error_resilient_mode && !frame_is_intra_only(pcs)) {
            svt_aom_wb_write_literal(wb, frm_hdr->primary_ref_frame, PRIMARY_REF_BITS);
        }
    } else { // reduced_still_picture_header
        assert(frame_size_override_flag == 0);
    }
    if (frm_hdr->frame_type == KEY_FRAME) {
        if (!frm_hdr->show_frame) {
            svt_aom_wb_write_literal(wb, pcs->av1_ref_signal.refresh_frame_mask, REF_FRAMES);
        }
    } else {
        if (frm_hdr->frame_type == INTRA_ONLY_FRAME) {
            // pcs->refresh_frame_mask = get_refresh_mask(cpi);
            int32_t updated_fb = -1;
            for (int32_t i = 0; i < REF_FRAMES; i++) {
                // If more than one frame is refreshed, it doesn't matter which one
                // we pick, so pick the first.
                if (pcs->av1_ref_signal.refresh_frame_mask & (1 << i)) {
                    updated_fb = i;
                    break;
                }
            }
            assert(updated_fb >= 0);
            pcs->fb_of_context_type[pcs->frame_context_idx] = updated_fb;

            svt_aom_wb_write_literal(wb, pcs->av1_ref_signal.refresh_frame_mask, REF_FRAMES);
        } else if (frm_hdr->frame_type == INTER_FRAME || frame_is_sframe(pcs)) {
            //pcs->refresh_frame_mask = get_refresh_mask(cpi);
            if (frm_hdr->frame_type == INTER_FRAME) {
                svt_aom_wb_write_literal(wb, pcs->av1_ref_signal.refresh_frame_mask, REF_FRAMES);
            } else {
                assert(frame_is_sframe(pcs) && pcs->av1_ref_signal.refresh_frame_mask == 0xFF);
            }

            // write ref order hint map into bitstream
            if (pcs->frm_hdr.error_resilient_mode && scs->seq_header.order_hint_info.enable_order_hint) {
                for (int32_t ref_idx = 0; ref_idx < REF_FRAMES; ref_idx++) {
                    svt_aom_wb_write_literal(
                        wb, pcs->dpb_order_hint[ref_idx], scs->seq_header.order_hint_info.order_hint_bits);
                }
            }

            int32_t updated_fb = -1;
            for (int32_t i = 0; i < REF_FRAMES; i++) {
                // If more than one frame is refreshed, it doesn't matter which one
                // we pick, so pick the first.
                if (pcs->av1_ref_signal.refresh_frame_mask & (1 << i)) {
                    updated_fb = i;
                    break;
                }
            }
            // large scale tile sometimes won't refresh any fbs
            if (updated_fb >= 0) {
                pcs->fb_of_context_type[pcs->frame_context_idx] = updated_fb;
            }
        }
    }

#if DEBUG_SFRAME
    {
        uint32_t* _ref_frame_map = pcs->dpb_order_hint;
        fprintf(stderr,
                "\nFrame %d, use_ref_frame_mvs %u, ref_order_hint_map %d,%d,%d,%d,%d,%d,%d,%d\n",
                (int)pcs->picture_number,
                frm_hdr->use_ref_frame_mvs,
                _ref_frame_map[0],
                _ref_frame_map[1],
                _ref_frame_map[2],
                _ref_frame_map[3],
                _ref_frame_map[4],
                _ref_frame_map[5],
                _ref_frame_map[6],
                _ref_frame_map[7]);
    }
#endif

    if (frm_hdr->frame_type == KEY_FRAME) {
        write_frame_size(pcs, frame_size_override_flag, wb);
        assert(av1_superres_unscaled(&(pcs->av1_cm->frm_size)) || !(frm_hdr->allow_intrabc));
        if (frm_hdr->allow_screen_content_tools && av1_superres_unscaled(&(pcs->av1_cm->frm_size))) {
            svt_aom_wb_write_bit(wb, frm_hdr->allow_intrabc);
        }
        // all eight fbs are refreshed, pick one that will live long enough
        pcs->fb_of_context_type[REGULAR_FRAME] = 0;
    } else {
        if (frm_hdr->frame_type == INTRA_ONLY_FRAME) {
            write_frame_size(pcs, frame_size_override_flag, wb);
            assert(av1_superres_unscaled(&(pcs->av1_cm->frm_size)) || !(frm_hdr->allow_intrabc));
            if (frm_hdr->allow_screen_content_tools && av1_superres_unscaled(&(pcs->av1_cm->frm_size))) {
                svt_aom_wb_write_bit(wb, frm_hdr->allow_intrabc);
            }
        } else if (frm_hdr->frame_type == INTER_FRAME || frame_is_sframe(pcs)) {
            MvReferenceFrame ref_frame;

            assert(frm_hdr->frame_refs_short_signaling == 0);
            // NOTE: Error resilient mode turns off frame_refs_short_signaling
            //       automatically.
            if (scs->seq_header.order_hint_info.enable_order_hint) {
                svt_aom_wb_write_bit(wb, frm_hdr->frame_refs_short_signaling);
            }

            if (frm_hdr->frame_refs_short_signaling) {
                svt_aom_wb_write_literal(wb, get_ref_frame_map_idx(pcs, LAST_FRAME), REF_FRAMES_LOG2);
                svt_aom_wb_write_literal(wb, get_ref_frame_map_idx(pcs, GOLDEN_FRAME), REF_FRAMES_LOG2);
            }
            for (ref_frame = LAST_FRAME; ref_frame <= ALTREF_FRAME; ++ref_frame) {
                assert(get_ref_frame_map_idx(pcs, ref_frame) != INVALID_IDX);
                if (!frm_hdr->frame_refs_short_signaling) {
                    svt_aom_wb_write_literal(wb, get_ref_frame_map_idx(pcs, ref_frame), REF_FRAMES_LOG2);
                }

                if (scs->seq_header.frame_id_numbers_present_flag) {
                    SVT_ERROR("frame_id_numbers_present_flag not supported yet\n");
                    //int32_t i = get_ref_frame_map_idx(cpi, ref_frame);
                    //int32_t frame_id_len = cm->seq_params.frame_id_length;
                    //int32_t diff_len = cm->seq_params.delta_frame_id_length;
                    //int32_t delta_frame_id_minus1 =
                    //    ((cm->current_frame_id - cm->ref_frame_id[i] +
                    //    (1 << frame_id_len)) %
                    //    (1 << frame_id_len)) -
                    //    1;
                    //if (delta_frame_id_minus1 < 0 ||
                    //    delta_frame_id_minus1 >= (1 << diff_len))
                    //    cm->invalid_delta_frame_id_minus1 = 1;
                    //svt_aom_wb_write_literal(wb, delta_frame_id_minus1, diff_len);
                }
            }

            if (!pcs->frm_hdr.error_resilient_mode && frame_size_override_flag) {
                write_frame_size_with_refs(pcs, wb);
            } else {
                write_frame_size(pcs, frame_size_override_flag, wb);
            }

            if (frm_hdr->force_integer_mv) {
                frm_hdr->allow_high_precision_mv = 0;
            } else {
                svt_aom_wb_write_bit(wb, frm_hdr->allow_high_precision_mv);
            }
#define LOG_SWITCHABLE_FILTERS 2

            svt_aom_wb_write_bit(wb, pcs->frm_hdr.interpolation_filter == SWITCHABLE);
            if (pcs->frm_hdr.interpolation_filter != SWITCHABLE) {
                svt_aom_wb_write_literal(wb, pcs->frm_hdr.interpolation_filter, LOG_SWITCHABLE_FILTERS);
            }

            svt_aom_wb_write_bit(wb, frm_hdr->is_motion_mode_switchable);
            if (frame_might_allow_ref_frame_mvs(pcs, scs)) {
                svt_aom_wb_write_bit(wb, frm_hdr->use_ref_frame_mvs);
            }
        }
    }

    //if (scs->frame_id_numbers_present_flag)
    //    pcs->refresh_mask = get_refresh_mask(pcs);
    const int32_t might_bwd_adapt = !(scs->seq_header.reduced_still_picture_header) && !(frm_hdr->disable_cdf_update);
    if (pcs->large_scale_tile) {
        pcs->refresh_frame_context = REFRESH_FRAME_CONTEXT_DISABLED;
    }
    if (might_bwd_adapt) {
        svt_aom_wb_write_bit(wb, pcs->refresh_frame_context == REFRESH_FRAME_CONTEXT_DISABLED);
    }

    write_tile_info(pcs, /*saved_wb,*/ wb);

    encode_quantization(pcs, wb);
    encode_segmentation(pcs, wb);
    //svt_aom_wb_write_bit(wb, 0);
    //encode_segmentation(cm, xd, wb);
    //if (pcs->delta_q_present_flag)
    // assert(delta_q_allowed == 1 && frm_hdr->quantisation_params.base_q_idx > 0);

    if (frm_hdr->quantization_params.base_q_idx > 0) {
        svt_aom_wb_write_bit(wb, frm_hdr->delta_q_params.delta_q_present);
        if (frm_hdr->delta_q_params.delta_q_present) {
            svt_aom_wb_write_literal(wb, OD_ILOG_NZ(frm_hdr->delta_q_params.delta_q_res) - 1, 2);
            for (uint16_t tile_idx = 0; tile_idx < tile_cnt; tile_idx++) {
                pcs->prev_qindex[tile_idx] = frm_hdr->quantization_params.base_q_idx;
            }
            if (frm_hdr->allow_intrabc) {
                assert(frm_hdr->delta_lf_params.delta_lf_present == 0);
            } else {
                svt_aom_wb_write_bit(wb, frm_hdr->delta_lf_params.delta_lf_present);
            }
            if (frm_hdr->delta_lf_params.delta_lf_present) {
                svt_aom_wb_write_literal(wb, OD_ILOG_NZ(frm_hdr->delta_lf_params.delta_lf_res) - 1, 2);
                pcs->prev_delta_lf_from_base = 0;
                svt_aom_wb_write_bit(wb, frm_hdr->delta_lf_params.delta_lf_multi);
                const int32_t frame_lf_count = pcs->monochrome == 0 ? FRAME_LF_COUNT : FRAME_LF_COUNT - 2;
                for (int32_t lf_id = 0; lf_id < frame_lf_count; ++lf_id) {
                    pcs->prev_delta_lf[lf_id] = 0;
                }
            }
        }
    }

    if (frm_hdr->all_lossless) {
        assert(av1_superres_unscaled(&(pcs->av1_cm->frm_size)));
    } else {
        if (!frm_hdr->coded_lossless) {
            encode_loopfilter(pcs, wb);
            if (scs->seq_header.cdef_level) {
                encode_cdef(pcs, wb);
            }
        }

        if (scs->seq_header.enable_restoration) {
            encode_restoration_mode(pcs, wb);
        }
    }
    if (frm_hdr->coded_lossless) {
        assert(1); // assert(frm_hdr->tx_mode == ONLY_4X4);
    } else {
        svt_aom_wb_write_bit(wb, frm_hdr->tx_mode == TX_MODE_SELECT);
    }
    //write_tx_mode(cm, &pcs->tx_mode, wb);

    if (pcs->allow_comp_inter_inter) {
        const int32_t use_hybrid_pred = frm_hdr->reference_mode == REFERENCE_MODE_SELECT;

        svt_aom_wb_write_bit(wb, use_hybrid_pred);
    }

    if (frm_hdr->skip_mode_params.skip_mode_allowed) {
        svt_aom_wb_write_bit(wb, frm_hdr->skip_mode_params.skip_mode_flag);
    }

    if (frame_might_allow_warped_motion(pcs, scs)) {
        svt_aom_wb_write_bit(wb, frm_hdr->allow_warped_motion);
    } else {
        assert(!frm_hdr->allow_warped_motion);
    }

    svt_aom_wb_write_bit(wb, frm_hdr->reduced_tx_set);

    if (!frame_is_intra_only(pcs)) {
        //  SVT_ERROR("Global motion not supported yet\n");
        write_global_motion(pcs, wb);
    }
#if CONFIG_ENABLE_FILM_GRAIN
    if (scs->seq_header.film_grain_params_present && (frm_hdr->show_frame || frm_hdr->showable_frame)) {
        write_film_grain_params(pcs, wb);
    }
#endif
}

static uint32_t write_obu_header(ObuType obu_type, int32_t obuExtension, uint8_t* const dst) {
    AomWriteBitBuffer wb   = {dst, 0};
    uint32_t          size = 0;

    svt_aom_wb_write_literal(&wb, 0, 1); // forbidden bit.
    svt_aom_wb_write_literal(&wb, (int32_t)obu_type, 4);
    svt_aom_wb_write_literal(&wb, obuExtension ? 1 : 0, 1);
    svt_aom_wb_write_literal(&wb, 1, 1); // obu_has_payload_length_field
    svt_aom_wb_write_literal(&wb, 0, 1); // reserved

    if (obuExtension) {
        svt_aom_wb_write_literal(&wb, obuExtension & 0xFF, 8);
    }
    size = svt_aom_wb_bytes_written(&wb);
    return size;
}

static int32_t write_uleb_obu_size(uint32_t obu_header_size, uint32_t obu_payload_size, uint8_t* dest) {
    const uint32_t obu_size       = obu_payload_size;
    const uint32_t offset         = obu_header_size;
    size_t         coded_obu_size = 0;

    if (svt_aom_uleb_encode(obu_size, sizeof(obu_size), dest + offset, &coded_obu_size) != 0) {
        return AOM_CODEC_ERROR;
    }

    return AOM_CODEC_OK;
}

static size_t obu_mem_move(uint32_t obu_header_size, uint32_t obu_payload_size, uint8_t* data) {
    const size_t   length_field_size = svt_aom_uleb_size_in_bytes(obu_payload_size);
    const uint32_t move_dst_offset   = (uint32_t)length_field_size + obu_header_size;
    const uint32_t move_src_offset   = obu_header_size;
    const uint32_t move_size         = obu_payload_size;
    memmove(data + move_dst_offset, data + move_src_offset, move_size);
    return length_field_size;
}

static void add_trailing_bits(AomWriteBitBuffer* wb) {
    if (svt_aom_wb_is_byte_aligned(wb)) {
        svt_aom_wb_write_literal(wb, 0x80, 8);
    } else {
        // assumes that the other bits are already 0s
        svt_aom_wb_write_bit(wb, 1);
    }
}

// writes the type and payload of the provided metadata to the address dst as a metadata OBU
static uint32_t write_obu_metadata(SvtMetadataT* metadata, uint8_t* const dst) {
    if (!metadata || !metadata->payload) {
        return 0;
    }
    AomWriteBitBuffer wb   = {dst, 0};
    uint32_t          size = 0;
    svt_aom_wb_write_literal(&wb, metadata->type, 8);
    for (size_t i = 0; i < metadata->sz; ++i) {
        svt_aom_wb_write_literal(&wb, metadata->payload[i], 8);
    }
    add_trailing_bits(&wb);
    size = svt_aom_wb_bytes_written(&wb);
    return size;
}

static void write_bitstream_level(BitstreamLevel bl, AomWriteBitBuffer* wb) {
    uint8_t seq_level_idx = major_minor_to_seq_level_idx(bl);
    assert(is_valid_seq_level_idx(seq_level_idx));
    svt_aom_wb_write_literal(wb, seq_level_idx, LEVEL_BITS);
}

static uint32_t write_sequence_header_obu(SequenceControlSet* scs, uint8_t* const dst, uint8_t numberSpatialLayers) {
    AomWriteBitBuffer wb   = {dst, 0};
    uint32_t          size = 0;

    set_bitstream_level_tier(scs);

    write_profile((BitstreamProfile)scs->static_config.profile, &wb);

    // Still picture or not
    svt_aom_wb_write_bit(&wb, scs->seq_header.still_picture);
    assert(IMPLIES(!scs->seq_header.still_picture, !scs->seq_header.reduced_still_picture_header));

    // whether to use reduced still picture header
    svt_aom_wb_write_bit(&wb, scs->seq_header.reduced_still_picture_header);

    if (scs->seq_header.reduced_still_picture_header) {
        write_bitstream_level(scs->level[0], &wb);
    } else {
        svt_aom_wb_write_bit(&wb, scs->seq_header.timing_info.timing_info_present); // timing info present flag

        if (scs->seq_header.timing_info.timing_info_present) {
            // timing_info
            SVT_ERROR("timing_info_present not supported\n");
            /*write_timing_info_header(cm, &wb);
            svt_aom_wb_write_bit(&wb, cm->decoder_model_info_present_flag);
            if (cm->decoder_model_info_present_flag) write_decoder_model_info(cm, &wb);*/
        }
        svt_aom_wb_write_bit(&wb, scs->seq_header.initial_display_delay_present_flag);

        uint8_t operating_points_cnt_minus_1 = numberSpatialLayers > 1 ? numberSpatialLayers - 1 : 0;
        svt_aom_wb_write_literal(&wb, operating_points_cnt_minus_1, OP_POINTS_CNT_MINUS_1_BITS);
        int32_t i;
        for (i = 0; i < operating_points_cnt_minus_1 + 1; i++) {
            svt_aom_wb_write_literal(&wb, scs->seq_header.operating_point[i].op_idc, OP_POINTS_IDC_BITS);
            write_bitstream_level(scs->level[i], &wb);
            if (scs->level[i].major > 3) {
                svt_aom_wb_write_bit(&wb, scs->seq_header.operating_point[i].seq_tier);
            }
            if (scs->seq_header.decoder_model_info_present_flag) {
                SVT_ERROR("decoder_model_info_present_flag not supported\n");
                //svt_aom_wb_write_bit(&wb,
                //    cm->op_params[i].decoder_model_param_present_flag);
                //if (cm->op_params[i].decoder_model_param_present_flag)
                //    write_dec_model_op_parameters(cm, &wb, i);
            }
            if (scs->seq_header.initial_display_delay_present_flag) {
                SVT_ERROR("display_model_info_present_flag not supported\n");
                //svt_aom_wb_write_bit(&wb,
                //    cm->op_params[i].display_model_param_present_flag);
                //if (cm->op_params[i].display_model_param_present_flag) {
                //    assert(cm->op_params[i].initial_display_delay <= 10);
                //    svt_aom_wb_write_literal(&wb, cm->op_params[i].initial_display_delay - 1,
                //        4);
                //}
            }
        }
    }
    write_sequence_header(scs, &wb);

    write_color_config(scs, &wb);

    svt_aom_wb_write_bit(&wb, scs->seq_header.film_grain_params_present);

    add_trailing_bits(&wb);

    size = svt_aom_wb_bytes_written(&wb);
    return size;
}

static uint32_t write_tile_group_header(uint8_t* const dst, int startTile, int endTile, int tiles_log2,
                                        int tile_start_and_end_present_flag) {
    AomWriteBitBuffer wb   = {dst, 0};
    uint32_t          size = 0;

    if (!tiles_log2) {
        return size;
    }
    svt_aom_wb_write_bit(&wb, tile_start_and_end_present_flag);

    if (tile_start_and_end_present_flag) {
        svt_aom_wb_write_literal(&wb, startTile, tiles_log2);
        svt_aom_wb_write_literal(&wb, endTile, tiles_log2);
    }

    size = svt_aom_wb_bytes_written(&wb);
    return size;
}

static uint32_t write_frame_header_obu(SequenceControlSet* scs, PictureParentControlSet* pcs, uint8_t* const dst,
                                       uint8_t show_existing, int32_t appendTrailingBits) {
    AomWriteBitBuffer wb         = {dst, 0};
    uint32_t          total_size = 0;

    write_uncompressed_header_obu(scs, pcs, /* saved_wb,*/ &wb, show_existing);

    if (appendTrailingBits) {
        add_trailing_bits(&wb);
    }

    if (show_existing) {
        total_size = svt_aom_wb_bytes_written(&wb);
        return total_size;
    }

    total_size = svt_aom_wb_bytes_written(&wb);
    return total_size;
}

EbErrorType svt_aom_write_metadata_av1(Bitstream* bitstream_ptr, SvtMetadataArrayT* metadata,
                                       const EbAv1MetadataType type) {
    EbErrorType return_error = EB_ErrorNone;
    if (!metadata || !metadata->metadata_array) {
        return EB_ErrorBadParameter;
    }

    OutputBitstreamUnit* output_bitstream_ptr = (OutputBitstreamUnit*)bitstream_ptr->output_bitstream_ptr;
    uint8_t*             data                 = output_bitstream_ptr->buffer_av1;

    for (size_t i = 0; i < metadata->sz; i++) {
        SvtMetadataT* current_metadata = metadata->metadata_array[i];
        if (current_metadata && current_metadata->payload && current_metadata->type == type) {
            uint32_t      obu_header_size      = 0;
            int32_t       curr_data_size       = 0;
            const uint8_t obu_extension_header = 0;
            // A new tile group begins at this tile.  Write the obu header and
            // tile group header
            const ObuType obu_type = OBU_METADATA;
            curr_data_size         = write_obu_header(obu_type, obu_extension_header, data);
            obu_header_size        = curr_data_size;
            curr_data_size += write_obu_metadata(current_metadata, data + curr_data_size);
            const uint32_t obu_payload_size  = curr_data_size - obu_header_size;
            const size_t   length_field_size = obu_mem_move(obu_header_size, obu_payload_size, data);
            if (write_uleb_obu_size(obu_header_size, obu_payload_size, data) != AOM_CODEC_OK) {
                assert(0);
            }
            curr_data_size += (int32_t)length_field_size;
            data += curr_data_size;
        }
    }
    output_bitstream_ptr->buffer_av1 = data;
    return return_error;
}

/**************************************************
* EncodeFrameHeaderHeader
**************************************************/
EbErrorType svt_aom_write_frame_header_av1(Bitstream* bitstream_ptr, SequenceControlSet* scs, PictureControlSet* pcs,
                                           uint8_t show_existing) {
    EbErrorType              return_error         = EB_ErrorNone;
    OutputBitstreamUnit*     output_bitstream_ptr = (OutputBitstreamUnit*)bitstream_ptr->output_bitstream_ptr;
    PictureParentControlSet* ppcs                 = pcs->ppcs;
    Av1Common* const         cm                   = ppcs->av1_cm;
    uint16_t                 tile_cnt             = cm->tiles_info.tile_rows * cm->tiles_info.tile_cols;
    uint8_t*                 data                 = output_bitstream_ptr->buffer_av1;
    uint32_t                 obu_header_size      = 0;

    int32_t curr_data_size = 0;

    const uint8_t obu_extension_header = 0;

    // A new tile group begins at this tile.  Write the obu header and
    // tile group header
    const ObuType obu_type = show_existing ? OBU_FRAME_HEADER : OBU_FRAME;
    curr_data_size         = write_obu_header(obu_type, obu_extension_header, data);
    obu_header_size        = curr_data_size;

    curr_data_size += write_frame_header_obu(
        scs, ppcs, /*saved_wb,*/ data + curr_data_size, show_existing, show_existing);

    const int n_log2_tiles                    = ppcs->av1_cm->log2_tile_rows + ppcs->av1_cm->log2_tile_cols;
    const int tile_start_and_end_present_flag = 0;

    curr_data_size += write_tile_group_header(
        data + curr_data_size, 0, 0, n_log2_tiles, tile_start_and_end_present_flag);

    if (!show_existing) {
        // Add data from EC stream to Picture Stream.
        for (int tile_idx = 0; tile_idx < tile_cnt; tile_idx++) {
            const int32_t tile_size       = pcs->ec_info[tile_idx]->ec->ec_writer.pos;
            uint8_t       tile_size_bytes = 0;
            // tile_size += (tile_idx != tile_cnt - 1) ? 4 : 0;
            if (tile_idx != tile_cnt - 1 && tile_cnt > 1) {
                tile_size_bytes = pcs->tile_size_bytes_minus_1 + 1;
                mem_put_varsize(data + curr_data_size, tile_size_bytes, tile_size - 1);
            }
            OutputBitstreamUnit* ec_output_bitstream_ptr =
                (OutputBitstreamUnit*)pcs->ec_info[tile_idx]->ec->ec_output_bitstream_ptr;
            assert(output_bitstream_ptr->buffer_av1 >= output_bitstream_ptr->buffer_begin_av1);
            // Size of the buffer needed to store all data; if buffer is too small, increase buffer
            // size
            uint32_t data_size = (uint32_t)tile_size + curr_data_size + tile_size_bytes + 10 /*MAX length_field_size*/ +
                (uint32_t)(output_bitstream_ptr->buffer_av1 - output_bitstream_ptr->buffer_begin_av1);
            if (output_bitstream_ptr->size < data_size) {
                svt_realloc_output_bitstream_unit(output_bitstream_ptr,
                                                  data_size + 1); // plus one for good measure
                data = output_bitstream_ptr->buffer_av1;
            }
            svt_memcpy(data + curr_data_size + tile_size_bytes, ec_output_bitstream_ptr->buffer_begin_av1, tile_size);
            curr_data_size += (tile_size + tile_size_bytes);
        }
    }
    const uint32_t obu_payload_size  = curr_data_size - obu_header_size;
    const size_t   length_field_size = obu_mem_move(obu_header_size, obu_payload_size, data);
    if (write_uleb_obu_size(obu_header_size, obu_payload_size, data) != AOM_CODEC_OK) {
        assert(0);
    }
    curr_data_size += (int32_t)length_field_size;
    data += curr_data_size;

    output_bitstream_ptr->buffer_av1 = data;
    return return_error;
}

/**************************************************
* svt_aom_encode_sps_av1
**************************************************/
EbErrorType svt_aom_encode_sps_av1(Bitstream* bitstream_ptr, SequenceControlSet* scs) {
    EbErrorType          return_error             = EB_ErrorNone;
    OutputBitstreamUnit* output_bitstream_ptr     = (OutputBitstreamUnit*)bitstream_ptr->output_bitstream_ptr;
    uint8_t*             data                     = output_bitstream_ptr->buffer_av1;
    uint32_t             obu_header_size          = 0;
    uint32_t             obu_payload_size         = 0;
    const uint8_t        enhancement_layers_count = 0; // cm->enhancement_layers_count;

    // write sequence header obu if KEY_FRAME, preceded by 4-byte size
    obu_header_size = write_obu_header(OBU_SEQUENCE_HEADER, 0, data);

    obu_payload_size = write_sequence_header_obu(scs, /*cpi,*/ data + obu_header_size, enhancement_layers_count);

    const size_t length_field_size = obu_mem_move(obu_header_size, obu_payload_size, data);
    if (write_uleb_obu_size(obu_header_size, obu_payload_size, data) != AOM_CODEC_OK) {
        // return AOM_CODEC_ERROR;
    }

    data += obu_header_size + obu_payload_size + length_field_size;
    output_bitstream_ptr->buffer_av1 = data;
    return return_error;
}

/**************************************************
* svt_aom_encode_td_av1
**************************************************/
EbErrorType svt_aom_encode_td_av1(uint8_t* output_bitstream_ptr) {
    assert(output_bitstream_ptr != NULL);

    // move data and insert OBU_TD preceded by optional 4 byte size
    // OBUs are preceded/succeeded by an unsigned leb128 coded integer.
    write_uleb_obu_size(write_obu_header(OBU_TEMPORAL_DELIMITER, 0, output_bitstream_ptr), 0, output_bitstream_ptr);
    return EB_ErrorNone;
}

static void av1_write_delta_q_index(FRAME_CONTEXT* frame_context, int32_t delta_qindex, AomWriter* w) {
    int32_t sign     = delta_qindex < 0;
    int32_t abs      = sign ? -delta_qindex : delta_qindex;
    int32_t smallval = abs < DELTA_Q_SMALL ? 1 : 0;
    //FRAME_CONTEXT *ec_ctx = xd->tile_ctx;

    aom_write_symbol(w, AOMMIN(abs, DELTA_Q_SMALL), frame_context->delta_q_cdf, DELTA_Q_PROBS + 1);

    if (!smallval) {
        int32_t rem_bits = OD_ILOG_NZ(abs - 1) - 1;
        int32_t thr      = (1 << rem_bits) + 1;
        aom_write_literal(w, rem_bits - 1, 3);
        aom_write_literal(w, abs - thr, rem_bits);
    }
    if (abs > 0) {
        aom_write_bit(w, sign);
    }
}

static void write_cdef(SequenceControlSet* scs, PictureControlSet* pcs, EntropyCodingContext* ctx, AomWriter* w,
                       int32_t skip, int32_t mi_col, int32_t mi_row) {
    Av1Common*   cm      = pcs->ppcs->av1_cm;
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;

    if (frm_hdr->coded_lossless || frm_hdr->allow_intrabc) {
        // Initialize to indicate no CDEF for safety.
        frm_hdr->cdef_params.cdef_bits           = 0;
        frm_hdr->cdef_params.cdef_y_strength[0]  = 0;
        pcs->ppcs->nb_cdef_strengths             = 1;
        frm_hdr->cdef_params.cdef_uv_strength[0] = 0;
        return;
    }

    const int32_t     m    = ~((1 << (6 - MI_SIZE_LOG2)) - 1);
    const MbModeInfo* mbmi = pcs->mi_grid_base[(mi_row & m) * cm->mi_stride + (mi_col & m)];

    // Initialise when at top left part of the superblock
    if (!(mi_row & (scs->seq_header.sb_mi_size - 1)) && !(mi_col & (scs->seq_header.sb_mi_size - 1))) { // Top left?
        ctx->cdef_transmitted[0] = ctx->cdef_transmitted[1] = ctx->cdef_transmitted[2] = ctx->cdef_transmitted[3] =
            false;
    }

    // Emit CDEF param at first non-skip coding block
    const int32_t mask  = 1 << (6 - MI_SIZE_LOG2);
    const int32_t index = scs->seq_header.sb_size == BLOCK_128X128 ? !!(mi_col & mask) + 2 * !!(mi_row & mask) : 0;

    if (!ctx->cdef_transmitted[index] && !skip) {
        aom_write_literal(w, mbmi->cdef_strength, frm_hdr->cdef_params.cdef_bits);
        ctx->cdef_transmitted[index] = true;
    }
}

void svt_av1_reset_loop_restoration(EntropyCodingContext* ctx) {
    for (int32_t p = 0; p < MAX_MB_PLANE; ++p) {
        set_default_wiener(ctx->wiener_info + p);
        set_default_sgrproj(ctx->sgrproj_info + p);
    }
}

static void write_wiener_filter(int32_t wiener_win, const WienerInfo* wiener_info, WienerInfo* ref_wiener_info,
                                AomWriter* wb) {
    if (wiener_win == WIENER_WIN) {
        svt_aom_write_primitive_refsubexpfin(wb,
                                             WIENER_FILT_TAP0_MAXV - WIENER_FILT_TAP0_MINV + 1,
                                             WIENER_FILT_TAP0_SUBEXP_K,
                                             ref_wiener_info->vfilter[0] - WIENER_FILT_TAP0_MINV,
                                             wiener_info->vfilter[0] - WIENER_FILT_TAP0_MINV);
    } else {
        assert(wiener_info->vfilter[0] == 0 && wiener_info->vfilter[WIENER_WIN - 1] == 0);
    }
    svt_aom_write_primitive_refsubexpfin(wb,
                                         WIENER_FILT_TAP1_MAXV - WIENER_FILT_TAP1_MINV + 1,
                                         WIENER_FILT_TAP1_SUBEXP_K,
                                         ref_wiener_info->vfilter[1] - WIENER_FILT_TAP1_MINV,
                                         wiener_info->vfilter[1] - WIENER_FILT_TAP1_MINV);
    svt_aom_write_primitive_refsubexpfin(wb,
                                         WIENER_FILT_TAP2_MAXV - WIENER_FILT_TAP2_MINV + 1,
                                         WIENER_FILT_TAP2_SUBEXP_K,
                                         ref_wiener_info->vfilter[2] - WIENER_FILT_TAP2_MINV,
                                         wiener_info->vfilter[2] - WIENER_FILT_TAP2_MINV);
    if (wiener_win == WIENER_WIN) {
        svt_aom_write_primitive_refsubexpfin(wb,
                                             WIENER_FILT_TAP0_MAXV - WIENER_FILT_TAP0_MINV + 1,
                                             WIENER_FILT_TAP0_SUBEXP_K,
                                             ref_wiener_info->hfilter[0] - WIENER_FILT_TAP0_MINV,
                                             wiener_info->hfilter[0] - WIENER_FILT_TAP0_MINV);
    } else {
        assert(wiener_info->hfilter[0] == 0 && wiener_info->hfilter[WIENER_WIN - 1] == 0);
    }
    svt_aom_write_primitive_refsubexpfin(wb,
                                         WIENER_FILT_TAP1_MAXV - WIENER_FILT_TAP1_MINV + 1,
                                         WIENER_FILT_TAP1_SUBEXP_K,
                                         ref_wiener_info->hfilter[1] - WIENER_FILT_TAP1_MINV,
                                         wiener_info->hfilter[1] - WIENER_FILT_TAP1_MINV);
    svt_aom_write_primitive_refsubexpfin(wb,
                                         WIENER_FILT_TAP2_MAXV - WIENER_FILT_TAP2_MINV + 1,
                                         WIENER_FILT_TAP2_SUBEXP_K,
                                         ref_wiener_info->hfilter[2] - WIENER_FILT_TAP2_MINV,
                                         wiener_info->hfilter[2] - WIENER_FILT_TAP2_MINV);
    svt_memcpy(ref_wiener_info, wiener_info, sizeof(*wiener_info));
}

static void write_sgrproj_filter(const SgrprojInfo* sgrproj_info, SgrprojInfo* ref_sgrproj_info, AomWriter* wb) {
    aom_write_literal(wb, sgrproj_info->ep, SGRPROJ_PARAMS_BITS);
    const SgrParamsType* params = &svt_aom_eb_sgr_params[sgrproj_info->ep];

    if (params->r[0] == 0) {
        assert(sgrproj_info->xqd[0] == 0);
        svt_aom_write_primitive_refsubexpfin(wb,
                                             SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1,
                                             SGRPROJ_PRJ_SUBEXP_K,
                                             (uint16_t)(ref_sgrproj_info->xqd[1] - SGRPROJ_PRJ_MIN1),
                                             (uint16_t)(sgrproj_info->xqd[1] - SGRPROJ_PRJ_MIN1));
    } else if (params->r[1] == 0) {
        svt_aom_write_primitive_refsubexpfin(wb,
                                             SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1,
                                             SGRPROJ_PRJ_SUBEXP_K,
                                             (uint16_t)(ref_sgrproj_info->xqd[0] - SGRPROJ_PRJ_MIN0),
                                             (uint16_t)(sgrproj_info->xqd[0] - SGRPROJ_PRJ_MIN0));
    } else {
        svt_aom_write_primitive_refsubexpfin(wb,
                                             SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1,
                                             SGRPROJ_PRJ_SUBEXP_K,
                                             (uint16_t)(ref_sgrproj_info->xqd[0] - SGRPROJ_PRJ_MIN0),
                                             (uint16_t)(sgrproj_info->xqd[0] - SGRPROJ_PRJ_MIN0));
        svt_aom_write_primitive_refsubexpfin(wb,
                                             SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1,
                                             SGRPROJ_PRJ_SUBEXP_K,
                                             (uint16_t)(ref_sgrproj_info->xqd[1] - SGRPROJ_PRJ_MIN1),
                                             (uint16_t)(sgrproj_info->xqd[1] - SGRPROJ_PRJ_MIN1));
    }

    svt_memcpy(ref_sgrproj_info, sgrproj_info, sizeof(*sgrproj_info));
}

static void loop_restoration_write_sb_coeffs(PictureControlSet* pcs, FRAME_CONTEXT* frame_context,
                                             EntropyCodingContext* ctx, const RestorationUnitInfo* rui,
                                             AomWriter* const w, int32_t plane) {
    const RestorationInfo* rsi         = pcs->rst_info + plane;
    RestorationType        frame_rtype = rsi->frame_restoration_type;
    if (frame_rtype == RESTORE_NONE) {
        return;
    }

    //(void)counts;
    //    assert(!cm->all_lossless);

    const int32_t   wiener_win   = (plane > 0) ? WIENER_WIN_CHROMA : WIENER_WIN;
    WienerInfo*     wiener_info  = &ctx->wiener_info[plane];
    SgrprojInfo*    sgrproj_info = &ctx->sgrproj_info[plane];
    RestorationType unit_rtype   = rui->restoration_type;

    assert(unit_rtype < CDF_SIZE(RESTORE_SWITCHABLE_TYPES));

    if (frame_rtype == RESTORE_SWITCHABLE) {
        aom_write_symbol(w,
                         unit_rtype,
                         /*xd->tile_ctx->*/ frame_context->switchable_restore_cdf,
                         RESTORE_SWITCHABLE_TYPES);
#if CONFIG_ENTROPY_STATS
        ++counts->switchable_restore[unit_rtype];
#endif
        switch (unit_rtype) {
        case RESTORE_WIENER:
            write_wiener_filter(wiener_win, &rui->wiener_info, wiener_info, w);
            //SVT_LOG("POC:%i plane:%i v:%i %i %i  h:%i %i %i\n", piCSetPtr->picture_number, plane, rui->wiener_info.vfilter[0], rui->wiener_info.vfilter[1], rui->wiener_info.vfilter[2], rui->wiener_info.hfilter[0], rui->wiener_info.hfilter[1], rui->wiener_info.hfilter[2]);
            break;
        case RESTORE_SGRPROJ:
            write_sgrproj_filter(&rui->sgrproj_info, sgrproj_info, w);
            //SVT_LOG("POC:%i plane:%i ep:%i xqd_0:%i  xqd_1:%i\n", piCSetPtr->picture_number, plane, rui->sgrproj_info.ep, rui->sgrproj_info.xqd[0], rui->sgrproj_info.xqd[1]);
            break;
        default:
            assert(unit_rtype == RESTORE_NONE); // SVT_LOG("POC:%i plane:%i OFF\n", piCSetPtr->picture_number, plane);
            break;
        }
    } else if (frame_rtype == RESTORE_WIENER) {
        aom_write_symbol(w,
                         unit_rtype != RESTORE_NONE,
                         /*xd->tile_ctx->*/ frame_context->wiener_restore_cdf,
                         2);
#if CONFIG_ENTROPY_STATS
        ++counts->wiener_restore[unit_rtype != RESTORE_NONE];
#endif
        if (unit_rtype != RESTORE_NONE) {
            write_wiener_filter(wiener_win, &rui->wiener_info, wiener_info, w);
            //SVT_LOG("POC:%i plane:%i v:%i %i %i  h:%i %i %i\n", piCSetPtr->picture_number, plane, rui->wiener_info.vfilter[0], rui->wiener_info.vfilter[1], rui->wiener_info.vfilter[2], rui->wiener_info.hfilter[0], rui->wiener_info.hfilter[1], rui->wiener_info.hfilter[2]);
        }
        //else
        //SVT_LOG("POC:%i plane:%i OFF\n", piCSetPtr->picture_number, plane);
    } else if (frame_rtype == RESTORE_SGRPROJ) {
        aom_write_symbol(w,
                         unit_rtype != RESTORE_NONE,
                         /*xd->tile_ctx->*/ frame_context->sgrproj_restore_cdf,
                         2);
#if CONFIG_ENTROPY_STATS
        ++counts->sgrproj_restore[unit_rtype != RESTORE_NONE];
#endif
        if (unit_rtype != RESTORE_NONE) {
            write_sgrproj_filter(&rui->sgrproj_info, sgrproj_info, w);
            //SVT_LOG("POC:%i plane:%i ep:%i xqd_0:%i  xqd_1:%i\n", piCSetPtr->picture_number, plane, rui->sgrproj_info.ep, rui->sgrproj_info.xqd[0], rui->sgrproj_info.xqd[1]);
        }
        //else
        //    SVT_LOG("POC:%i plane:%i OFF\n", piCSetPtr->picture_number, plane);
    }
}

static void ec_update_neighbors(PictureControlSet* pcs, EntropyCodingContext* ec_ctx, uint32_t blk_org_x,
                                uint32_t blk_org_y, uint16_t tile_idx, BlockSize bsize) {
    NeighborArrayUnit* partition_context_na        = pcs->partition_context_na[tile_idx];
    NeighborArrayUnit* luma_dc_sign_level_coeff_na = pcs->luma_dc_sign_level_coeff_na[tile_idx];
    NeighborArrayUnit* cr_dc_sign_level_coeff_na   = pcs->cr_dc_sign_level_coeff_na[tile_idx];
    NeighborArrayUnit* cb_dc_sign_level_coeff_na   = pcs->cb_dc_sign_level_coeff_na[tile_idx];
    MbModeInfo*        mbmi                        = get_mbmi(pcs, blk_org_x, blk_org_y);
    uint8_t            skip_coeff                  = mbmi->block_mi.skip;
    const int          bwidth                      = block_size_wide[bsize];
    const int          bheight                     = block_size_high[bsize];
    const bool         has_uv                      = is_chroma_reference(blk_org_y >> 2, blk_org_x >> 2, bsize, 1, 1);
    PartitionContext   partition;

    // Update the Leaf Depth Neighbor Array
    partition.above = partition_context_lookup[bsize].above;
    partition.left  = partition_context_lookup[bsize].left;

    svt_aom_neighbor_array_unit_mode_write(partition_context_na,
                                           (uint8_t*)&partition,
                                           blk_org_x,
                                           blk_org_y,
                                           bwidth,
                                           bheight,
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    if (skip_coeff) {
        uint8_t dc_sign_level_coeff = 0;

        svt_aom_neighbor_array_unit_mode_write(luma_dc_sign_level_coeff_na,
                                               (uint8_t*)&dc_sign_level_coeff,
                                               blk_org_x,
                                               blk_org_y,
                                               bwidth,
                                               bheight,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

        if (has_uv) {
            const BlockSize bsize_uv   = get_plane_block_size(bsize, 1, 1);
            const int       bwidth_uv  = block_size_wide[bsize_uv];
            const int       bheight_uv = block_size_high[bsize_uv];
            svt_aom_neighbor_array_unit_mode_write(cb_dc_sign_level_coeff_na,
                                                   &dc_sign_level_coeff,
                                                   ((blk_org_x >> 3) << 3) >> 1,
                                                   ((blk_org_y >> 3) << 3) >> 1,
                                                   bwidth_uv,
                                                   bheight_uv,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
            svt_aom_neighbor_array_unit_mode_write(cr_dc_sign_level_coeff_na,
                                                   &dc_sign_level_coeff,
                                                   ((blk_org_x >> 3) << 3) >> 1,
                                                   ((blk_org_y >> 3) << 3) >> 1,
                                                   bwidth_uv,
                                                   bheight_uv,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
            ec_ctx->coded_area_sb_uv += bwidth_uv * bheight_uv;
        }
        ec_ctx->coded_area_sb += bwidth * bheight;
    }
}

int svt_aom_allow_palette(int allow_screen_content_tools, BlockSize bsize) {
    return allow_screen_content_tools && block_size_wide[bsize] <= 64 && block_size_high[bsize] <= 64 &&
        bsize >= BLOCK_8X8;
}

int svt_aom_get_palette_bsize_ctx(BlockSize bsize) {
    return eb_num_pels_log2_lookup[bsize] - eb_num_pels_log2_lookup[BLOCK_8X8];
}

void svt_av1_tokenize_color_map(FRAME_CONTEXT* frame_context, EcBlkStruct* blk_ptr, int plane, TOKENEXTRA** t,
                                BlockSize bsize, TxSize tx_size, COLOR_MAP_TYPE type, int allow_update_cdf);
void svt_aom_get_block_dimensions(BlockSize bsize, int plane, const MacroBlockD* xd, int* width, int* height,
                                  int* rows_within_bounds, int* cols_within_bounds);
int  svt_get_palette_cache_y(const MacroBlockD* const xd, uint16_t* cache);
int  svt_av1_index_color_cache(const uint16_t* color_cache, int n_cache, const uint16_t* colors, int n_colors,
                               uint8_t* cache_color_found, int* out_cache_colors);

int svt_aom_get_palette_mode_ctx(const MacroBlockD* xd) {
    const MbModeInfo* const above_mi = xd->above_mbmi;
    const MbModeInfo* const left_mi  = xd->left_mbmi;
    int                     ctx      = 0;
    if (above_mi) {
        ctx += (above_mi->palette_mode_info.palette_size > 0);
    }
    if (left_mi) {
        ctx += (left_mi->palette_mode_info.palette_size > 0);
    }
    return ctx;
}

// Transmit color values with delta encoding. Write the first value as
// literal, and the deltas between each value and the previous one. "min_val" is
// the smallest possible value of the deltas.
static AOM_INLINE void delta_encode_palette_colors(const int* colors, int num, int bit_depth, int min_val,
                                                   AomWriter* w) {
    if (num <= 0) {
        return;
    }
    assert(colors[0] < (1 << bit_depth));
    aom_write_literal(w, colors[0], bit_depth);
    if (num == 1) {
        return;
    }
    int max_delta = 0;
    int deltas[PALETTE_MAX_SIZE];
    memset(deltas, 0, sizeof(deltas));
    for (int i = 1; i < num; ++i) {
        assert(colors[i] < (1 << bit_depth));
        const int delta = colors[i] - colors[i - 1];
        deltas[i - 1]   = delta;
        assert(delta >= min_val);
        if (delta > max_delta) {
            max_delta = delta;
        }
    }
    const int min_bits = bit_depth - 3;
    int       bits     = AOMMAX(av1_ceil_log2(max_delta + 1 - min_val), min_bits);
    assert(bits <= bit_depth);
    int range = (1 << bit_depth) - colors[0] - min_val;
    aom_write_literal(w, bits - min_bits, 2);
    for (int i = 0; i < num - 1; ++i) {
        aom_write_literal(w, deltas[i] - min_val, bits);
        range -= deltas[i];
        bits = AOMMIN(bits, av1_ceil_log2(range));
    }
}

static INLINE int get_unsigned_bits(unsigned int num_values) {
    return num_values > 0 ? get_msb(num_values) + 1 : 0;
}

static INLINE void write_uniform(AomWriter* w, int n, int v) {
    const int l = get_unsigned_bits(n);
    const int m = (1 << l) - n;
    if (l == 0) {
        return;
    }
    if (v < m) {
        aom_write_literal(w, v, l - 1);
    } else {
        aom_write_literal(w, m + ((v - m) >> 1), l - 1);
        aom_write_literal(w, (v - m) & 1, 1);
    }
}

int svt_aom_write_uniform_cost(int n, int v) {
    const int l = get_unsigned_bits(n);
    const int m = (1 << l) - n;
    if (l == 0) {
        return 0;
    }
    if (v < m) {
        return av1_cost_literal(l - 1);
    } else {
        return av1_cost_literal(l);
    }
}

// Transmit luma palette color values. First signal if each color in the color
// cache is used. Those colors that are not in the cache are transmitted with
// delta encoding.
static AOM_INLINE void write_palette_colors_y(const MacroBlockD* const xd, const PaletteModeInfo* const pmi,
                                              int bit_depth, AomWriter* w, const int palette_size) {
    const int n = palette_size;
    uint16_t  color_cache[2 * PALETTE_MAX_SIZE];
    const int n_cache = svt_get_palette_cache_y(xd, color_cache);
    int       out_cache_colors[PALETTE_MAX_SIZE];
    uint8_t   cache_color_found[2 * PALETTE_MAX_SIZE];
    const int n_out_cache = svt_av1_index_color_cache(
        color_cache, n_cache, pmi->palette_colors, n, cache_color_found, out_cache_colors);
    int n_in_cache = 0;
    for (int i = 0; i < n_cache && n_in_cache < n; ++i) {
        const int found = cache_color_found[i];
        aom_write_bit(w, found);
        n_in_cache += found;
    }
    assert(n_in_cache + n_out_cache == n);
    delta_encode_palette_colors(out_cache_colors, n_out_cache, bit_depth, 1, w);
}

static inline void pack_map_tokens(AomWriter* w, const TOKENEXTRA** tp, int n, int num) {
    const TOKENEXTRA* p = *tp;
    write_uniform(w, n, p->token); // The first color index.
    ++p;
    --num;
    for (int i = 0; i < num; ++i) {
        aom_write_symbol(w, p->token, p->color_map_cdf, n);
        ++p;
    }
    *tp = p;
}

static void write_palette_mode_info(PictureParentControlSet* ppcs, FRAME_CONTEXT* ec_ctx, MbModeInfo* mbmi,
                                    EcBlkStruct* blk_ptr, BlockSize bsize, int mi_row, int mi_col, AomWriter* w) {
    const uint32_t intra_luma_mode   = mbmi->block_mi.mode;
    uint32_t       intra_chroma_mode = mbmi->block_mi.uv_mode;

    const PaletteModeInfo* const pmi       = &blk_ptr->palette_info->pmi;
    const int                    bsize_ctx = svt_aom_get_palette_bsize_ctx(bsize);
    assert(bsize_ctx >= 0);
    if (intra_luma_mode == DC_PRED) {
        const int n                  = blk_ptr->palette_size[0];
        const int palette_y_mode_ctx = svt_aom_get_palette_mode_ctx(blk_ptr->av1xd);
        aom_write_symbol(w, n > 0, ec_ctx->palette_y_mode_cdf[bsize_ctx][palette_y_mode_ctx], 2);
        if (n > 0) {
            aom_write_symbol(w, n - PALETTE_MIN_SIZE, ec_ctx->palette_y_size_cdf[bsize_ctx], PALETTE_SIZES);
            write_palette_colors_y(blk_ptr->av1xd, pmi, ppcs->scs->static_config.encoder_bit_depth, w, n);
        }
    }

    const int uv_dc_pred = intra_chroma_mode == UV_DC_PRED && is_chroma_reference(mi_row, mi_col, bsize, 1, 1);
    if (uv_dc_pred) {
        assert(blk_ptr->palette_size[1] == 0); //remove when chroma is on
        const int palette_uv_mode_ctx = (blk_ptr->palette_size[0] > 0);
        aom_write_symbol(w, 0, ec_ctx->palette_uv_mode_cdf[palette_uv_mode_ctx], 2);
    }
}

void svt_av1_encode_dv(AomWriter* w, const Mv* mv, const Mv* ref, NmvContext* mvctx) {
    // DV and ref DV should not have sub-pel.
    assert((mv->x & 7) == 0);
    assert((mv->y & 7) == 0);
    assert((ref->x & 7) == 0);
    assert((ref->y & 7) == 0);
    // The y-component (row component) of the MV is coded first
    const Mv          diff = {{mv->x - ref->x, mv->y - ref->y}};
    const MvJointType j    = svt_av1_get_mv_joint(&diff);

    aom_write_symbol(w, j, mvctx->joints_cdf, MV_JOINTS);
    if (mv_joint_vertical(j)) {
        encode_mv_component(w, diff.y, &mvctx->comps[0], MV_SUBPEL_NONE);
    }

    if (mv_joint_horizontal(j)) {
        encode_mv_component(w, diff.x, &mvctx->comps[1], MV_SUBPEL_NONE);
    }
}

int svt_aom_allow_intrabc(const FrameHeader* frm_hdr, SliceType slice_type) {
    return (slice_type == I_SLICE && frm_hdr->allow_screen_content_tools && frm_hdr->allow_intrabc);
}

static void write_intrabc_info(FRAME_CONTEXT* ec_ctx, MbModeInfo* mbmi, EcBlkStruct* blk_ptr, AomWriter* w) {
    int use_intrabc = mbmi->block_mi.use_intrabc;
    aom_write_symbol(w, use_intrabc, ec_ctx->intrabc_cdf, 2);
    if (use_intrabc) {
        //assert(mbmi->mode == DC_PRED);
        //assert(mbmi->uv_mode == UV_DC_PRED);
        //assert(mbmi->motion_mode == SIMPLE_TRANSLATION);
        Mv dv_ref = blk_ptr->predmv[0];
        Mv mv     = mbmi->block_mi.mv[INTRA_FRAME];
        svt_av1_encode_dv(w, &mv, &dv_ref, &ec_ctx->ndvc);
    }
}

static INLINE int block_signals_txsize(BlockSize bsize) {
    return bsize > BLOCK_4X4;
}

static INLINE int get_vartx_max_txsize(/*const MbModeInfo *xd,*/ BlockSize bsize, int plane) {
    /* if (xd->lossless[xd->mi[0]->segment_id]) return TX_4X4;*/
    const TxSize max_txsize = blocksize_to_txsize[bsize];
    if (plane == 0) {
        return max_txsize; // luma
    }
    return av1_get_adjusted_tx_size(max_txsize); // chroma
}

static INLINE int max_block_wide(const MacroBlockD* xd, BlockSize bsize, int plane) {
    int max_blocks_wide = block_size_wide[bsize];

    if (xd->mb_to_right_edge < 0) {
        max_blocks_wide += gcc_right_shift(xd->mb_to_right_edge, 3 + !!plane);
    }

    // Scale the width in the transform block unit.
    return max_blocks_wide >> tx_size_wide_log2[0];
}

static INLINE int max_block_high(const MacroBlockD* xd, BlockSize bsize, int plane) {
    int max_blocks_high = block_size_high[bsize];

    if (xd->mb_to_bottom_edge < 0) {
        max_blocks_high += gcc_right_shift(xd->mb_to_bottom_edge, 3 + !!plane);
    }

    // Scale the height in the transform block unit.
    return max_blocks_high >> tx_size_high_log2[0];
}

static INLINE void txfm_partition_update(TXFM_CONTEXT* above_ctx, TXFM_CONTEXT* left_ctx, TxSize tx_size,
                                         TxSize txb_size) {
    BlockSize bsize = txsize_to_bsize[txb_size];
    assert(bsize < BLOCK_SIZES_ALL);
    int     bh  = mi_size_high[bsize];
    int     bw  = mi_size_wide[bsize];
    uint8_t txw = tx_size_wide[tx_size];
    uint8_t txh = tx_size_high[tx_size];
    int     i;
    for (i = 0; i < bh; ++i) {
        left_ctx[i] = txh;
    }
    for (i = 0; i < bw; ++i) {
        above_ctx[i] = txw;
    }
}

static INLINE TxSize get_sqr_tx_size(int tx_dim) {
    switch (tx_dim) {
    case 128:
    case 64:
        return TX_64X64;
        break;
    case 32:
        return TX_32X32;
        break;
    case 16:
        return TX_16X16;
        break;
    case 8:
        return TX_8X8;
        break;
    default:
        return TX_4X4;
    }
}

static INLINE int txfm_partition_context(TXFM_CONTEXT* above_ctx, TXFM_CONTEXT* left_ctx, BlockSize bsize,
                                         TxSize tx_size) {
    const uint8_t txw      = tx_size_wide[tx_size];
    const uint8_t txh      = tx_size_high[tx_size];
    const int     above    = *above_ctx < txw;
    const int     left     = *left_ctx < txh;
    int           category = TXFM_PARTITION_CONTEXTS;

    // dummy return, not used by others.
    if (tx_size == TX_4X4) {
        return 0;
    }

    TxSize max_tx_size = get_sqr_tx_size(AOMMAX(block_size_wide[bsize], block_size_high[bsize]));

    if (max_tx_size >= TX_8X8) {
        category = (txsize_sqr_up_map[tx_size] != max_tx_size && max_tx_size > TX_8X8) +
            (TX_SIZES - 1 - max_tx_size) * 2;
    }
    assert(category != TXFM_PARTITION_CONTEXTS);
    return category * 3 + above + left;
}

static void write_tx_size_vartx(MacroBlockD* xd, const MbModeInfo* mbmi, TxSize tx_size, int depth, int blk_row,
                                int blk_col, FRAME_CONTEXT* ec_ctx, AomWriter* w) {
    const int max_blocks_high = max_block_high(xd, mbmi->bsize, 0);
    const int max_blocks_wide = max_block_wide(xd, mbmi->bsize, 0);

    if (blk_row >= max_blocks_high || blk_col >= max_blocks_wide) {
        return;
    }

    if (depth == MAX_VARTX_DEPTH) {
        txfm_partition_update(xd->above_txfm_context + blk_col, xd->left_txfm_context + blk_row, tx_size, tx_size);
        return;
    }

    const int ctx = txfm_partition_context(
        xd->above_txfm_context + blk_col, xd->left_txfm_context + blk_row, mbmi->bsize, tx_size);
    const int write_txfm_partition = (tx_size == tx_depth_to_tx_size[mbmi->block_mi.tx_depth][mbmi->bsize]);

    if (write_txfm_partition) {
        aom_write_symbol(w, 0, ec_ctx->txfm_partition_cdf[ctx], 2);

        txfm_partition_update(xd->above_txfm_context + blk_col, xd->left_txfm_context + blk_row, tx_size, tx_size);
    } else {
        ASSERT(tx_size < TX_SIZES_ALL);
        const TxSize sub_txs = eb_sub_tx_size_map[tx_size];
        const int    bsw     = eb_tx_size_wide_unit[sub_txs];
        const int    bsh     = eb_tx_size_high_unit[sub_txs];

        aom_write_symbol(w, 1, ec_ctx->txfm_partition_cdf[ctx], 2);

        if (sub_txs == TX_4X4) {
            txfm_partition_update(xd->above_txfm_context + blk_col, xd->left_txfm_context + blk_row, sub_txs, tx_size);
            return;
        }

        assert(bsw > 0 && bsh > 0);
        for (int row = 0; row < eb_tx_size_high_unit[tx_size]; row += bsh) {
            for (int col = 0; col < eb_tx_size_wide_unit[tx_size]; col += bsw) {
                int offsetr = blk_row + row;
                int offsetc = blk_col + col;
                write_tx_size_vartx(xd, mbmi, sub_txs, depth + 1, offsetr, offsetc, ec_ctx, w);
            }
        }
    }
}

static INLINE void set_txfm_ctx(TXFM_CONTEXT* txfm_ctx, uint8_t txs, int len) {
    int i;
    for (i = 0; i < len; ++i) {
        txfm_ctx[i] = txs;
    }
}

static INLINE void set_txfm_ctxs(TxSize tx_size, int n8_w, int n8_h, int skip, const MacroBlockD* xd) {
    uint8_t bw = tx_size_wide[tx_size];
    uint8_t bh = tx_size_high[tx_size];

    if (skip) {
        bw = n8_w * MI_SIZE;
        bh = n8_h * MI_SIZE;
    }

    set_txfm_ctx(xd->above_txfm_context, bw, n8_w);
    set_txfm_ctx(xd->left_txfm_context, bh, n8_h);
}

static INLINE int tx_size_to_depth(TxSize tx_size, BlockSize bsize) {
    TxSize ctx_size = blocksize_to_txsize[bsize];
    int    depth    = 0;
    while (tx_size != ctx_size) {
        depth++;
        ctx_size = eb_sub_tx_size_map[ctx_size];
        assert(depth <= MAX_TX_DEPTH);
    }
    return depth;
}

// Returns a context number for the given MB prediction signal
// The mode info data structure has a one element border above and to the
// left of the entries corresponding to real blocks.
// The prediction flags in these dummy entries are initialized to 0.
static INLINE int get_tx_size_context(const MacroBlockD* xd) {
    const MbModeInfo*       mbmi        = xd->mi[0];
    const MbModeInfo* const above_mbmi  = xd->above_mbmi;
    const MbModeInfo* const left_mbmi   = xd->left_mbmi;
    const TxSize            max_tx_size = blocksize_to_txsize[mbmi->bsize];
    const int               max_tx_wide = tx_size_wide[max_tx_size];
    const int               max_tx_high = tx_size_high[max_tx_size];
    const int               has_above   = xd->up_available;
    const int               has_left    = xd->left_available;

    int above = xd->above_txfm_context[0] >= max_tx_wide;
    int left  = xd->left_txfm_context[0] >= max_tx_high;

    if (has_above) {
        if (is_inter_block(&above_mbmi->block_mi)) {
            above = block_size_wide[above_mbmi->bsize] >= max_tx_wide;
        }
    }

    if (has_left) {
        if (is_inter_block(&left_mbmi->block_mi)) {
            left = block_size_high[left_mbmi->bsize] >= max_tx_high;
        }
    }

    if (has_above && has_left) {
        return (above + left);
    } else if (has_above) {
        return above;
    } else if (has_left) {
        return left;
    } else {
        return 0;
    }
}

static void write_selected_tx_size(const MacroBlockD* xd, FRAME_CONTEXT* ec_ctx, AomWriter* w, TxSize tx_size) {
    const MbModeInfo* const mbmi  = xd->mi[0];
    const BlockSize         bsize = mbmi->bsize;

    if (block_signals_txsize(bsize)) {
        const int tx_size_ctx = get_tx_size_context(xd);
        assert(bsize < BLOCK_SIZES_ALL);
        const int     depth       = tx_size_to_depth(tx_size, bsize);
        const int     max_depths  = bsize_to_max_depth(bsize);
        const int32_t tx_size_cat = bsize_to_tx_size_cat(bsize);

        assert(depth >= 0 && depth <= max_depths);
        assert(!is_inter_block(&mbmi->block_mi));
        assert(IMPLIES(is_rect_tx(tx_size), is_rect_tx_allowed(/*xd,*/ mbmi)));

        aom_write_symbol(w, depth, ec_ctx->tx_size_cdf[tx_size_cat][tx_size_ctx], max_depths + 1);
    }
}

static EbErrorType av1_code_tx_size(PictureControlSet* pcs, int segment_id, FRAME_CONTEXT* ec_ctx, AomWriter* w,
                                    MacroBlockD* xd, const MbModeInfo* mbmi, TxSize tx_size, TxMode tx_mode,
                                    BlockSize bsize, uint8_t skip) {
    EbErrorType return_error = EB_ErrorNone;
    int         is_inter_tx  = is_inter_block(&mbmi->block_mi) || is_intrabc_block(&mbmi->block_mi);
    //int skip = mbmi->skip;
    //int segment_id = 0;// mbmi->segment_id;
    if (tx_mode == TX_MODE_SELECT && block_signals_txsize(bsize) && !(is_inter_tx && skip) &&
        !svt_av1_is_lossless_segment(pcs, segment_id)) {
        if (is_inter_tx) { // This implies skip flag is 0.
            const TxSize max_tx_size = get_vartx_max_txsize(/*xd,*/ bsize, 0);
            const int    txbh        = eb_tx_size_high_unit[max_tx_size];
            const int    txbw        = eb_tx_size_wide_unit[max_tx_size];
            const int    width       = block_size_wide[bsize] >> tx_size_wide_log2[0];
            const int    height      = block_size_high[bsize] >> tx_size_high_log2[0];
            int          idx, idy;
            for (idy = 0; idy < height; idy += txbh) {
                for (idx = 0; idx < width; idx += txbw) {
                    write_tx_size_vartx(xd, mbmi, max_tx_size, 0, idy, idx, ec_ctx, w);
                }
            }
        } else {
            write_selected_tx_size(xd, ec_ctx, w, tx_size);
            set_txfm_ctxs(tx_size, xd->n8_w, xd->n8_h, 0, xd);
        }
    } else {
        set_txfm_ctxs(tx_size, xd->n8_w, xd->n8_h, skip && is_inter_block(&mbmi->block_mi), xd);
    }

    return return_error;
}

void set_mi_row_col(PictureControlSet* pcs, MacroBlockD* xd, TileInfo* tile, int mi_row, int bh, int mi_col, int bw,
                    uint32_t mi_stride, int mi_rows, int mi_cols) {
    xd->mb_to_top_edge    = -((mi_row * MI_SIZE) * 8);
    xd->mb_to_bottom_edge = ((mi_rows - bh - mi_row) * MI_SIZE) * 8;
    xd->mb_to_left_edge   = -((mi_col * MI_SIZE) * 8);
    xd->mb_to_right_edge  = ((mi_cols - bw - mi_col) * MI_SIZE) * 8;

    xd->mi_stride = mi_stride;

    // Are edges available for intra prediction?
    xd->up_available     = (mi_row > tile->mi_row_start);
    xd->left_available   = (mi_col > tile->mi_col_start);
    const int32_t offset = mi_row * mi_stride + mi_col;
    xd->mi               = pcs->mi_grid_base + offset;

    if (xd->up_available) {
        xd->above_mbmi = xd->mi[-xd->mi_stride];
    } else {
        xd->above_mbmi = NULL;
    }

    if (xd->left_available) {
        xd->left_mbmi = xd->mi[-1];
    } else {
        xd->left_mbmi = NULL;
    }

    xd->n8_h        = bh;
    xd->n8_w        = bw;
    xd->is_sec_rect = 0;
    if (xd->n8_w < xd->n8_h) {
        // Only mark is_sec_rect as 1 for the last block.
        // For PARTITION_VERT_4, it would be (0, 0, 0, 1);
        // For other partitions, it would be (0, 1).
        if (!((mi_col + xd->n8_w) & (xd->n8_h - 1))) {
            xd->is_sec_rect = 1;
        }
    }

    if (xd->n8_w > xd->n8_h) {
        if (mi_row & (xd->n8_w - 1)) {
            xd->is_sec_rect = 1;
        }
    }
}

static INLINE int svt_aom_get_segment_id(Av1Common* cm, const uint8_t* segment_ids, BlockSize bsize, int mi_row,
                                         int mi_col) {
    const int mi_offset = mi_row * cm->mi_cols + mi_col;
    const int bw        = mi_size_wide[bsize];
    const int bh        = mi_size_high[bsize];
    const int xmis      = AOMMIN(cm->mi_cols - mi_col, bw);
    const int ymis      = AOMMIN(cm->mi_rows - mi_row, bh);
    int       x, y, segment_id = MAX_SEGMENTS;

    for (y = 0; y < ymis; ++y) {
        for (x = 0; x < xmis; ++x) {
            segment_id = AOMMIN(segment_id, segment_ids[mi_offset + y * cm->mi_cols + x]);
        }
    }

    assert(segment_id >= 0 && segment_id < MAX_SEGMENTS);
    return segment_id;
}

static void code_tx_size(PictureControlSet* pcs, uint32_t blk_org_x, uint32_t blk_org_y, EcBlkStruct* blk_ptr,
                         const BlockSize bsize, NeighborArrayUnit* txfm_context_array, FRAME_CONTEXT* ec_ctx,
                         AomWriter* w, uint8_t skip) {
    uint32_t      txfm_context_left_index  = get_neighbor_array_unit_left_index(txfm_context_array, blk_org_y);
    uint32_t      txfm_context_above_index = get_neighbor_array_unit_top_index(txfm_context_array, blk_org_x);
    TxMode        tx_mode                  = pcs->ppcs->frm_hdr.tx_mode;
    Av1Common*    cm                       = pcs->ppcs->av1_cm;
    MacroBlockD*  xd                       = blk_ptr->av1xd;
    TileInfo*     tile                     = &xd->tile;
    int32_t       mi_row                   = blk_org_y >> MI_SIZE_LOG2;
    int32_t       mi_col                   = blk_org_x >> MI_SIZE_LOG2;
    const int32_t bw                       = mi_size_wide[bsize];
    const int32_t bh                       = mi_size_high[bsize];
    uint32_t      mi_stride                = pcs->mi_stride;
    set_mi_row_col(pcs, xd, tile, mi_row, bh, mi_col, bw, mi_stride, cm->mi_rows, cm->mi_cols);

    const MbModeInfo* const mbmi              = xd->mi[0];
    xd->above_txfm_context                    = &txfm_context_array->top_array[txfm_context_above_index];
    xd->left_txfm_context                     = &txfm_context_array->left_array[txfm_context_left_index];
    const TxSize             tx_size          = tx_depth_to_tx_size[mbmi->block_mi.tx_depth][bsize];
    FrameHeader*             frm_hdr          = &pcs->ppcs->frm_hdr;
    SegmentationNeighborMap* segmentation_map = pcs->segmentation_neighbor_map;
    av1_code_tx_size(pcs,
                     frm_hdr->segmentation_params.segmentation_enabled
                         ? svt_aom_get_segment_id(cm, segmentation_map->data, BLOCK_4X4, mi_row, mi_col)
                         : 0,
                     ec_ctx,
                     w,
                     xd,
                     mbmi,
                     tx_size,
                     tx_mode,
                     bsize,
                     skip);
}

int svt_av1_get_spatial_seg_prediction(PictureControlSet* pcs, MacroBlockD* xd, uint32_t blk_org_x, uint32_t blk_org_y,
                                       int* cdf_index) {
    int prev_ul = -1; // top left segment_id
    int prev_l  = -1; // left segment_id
    int prev_u  = -1; // top segment_id

    uint32_t                 mi_col           = blk_org_x >> MI_SIZE_LOG2;
    uint32_t                 mi_row           = blk_org_y >> MI_SIZE_LOG2;
    bool                     left_available   = xd->left_available;
    bool                     up_available     = xd->up_available;
    Av1Common*               cm               = pcs->ppcs->av1_cm;
    SegmentationNeighborMap* segmentation_map = pcs->segmentation_neighbor_map;

    //    SVT_LOG("Left available = %d, Up Available = %d ", left_available, up_available);

    if ((up_available) && (left_available)) {
        prev_ul = svt_aom_get_segment_id(cm, segmentation_map->data, BLOCK_4X4, mi_row - 1, mi_col - 1);
    }

    if (up_available) {
        prev_u = svt_aom_get_segment_id(cm, segmentation_map->data, BLOCK_4X4, mi_row - 1, mi_col - 0);
    }

    if (left_available) {
        prev_l = svt_aom_get_segment_id(cm, segmentation_map->data, BLOCK_4X4, mi_row - 0, mi_col - 1);
    }

    // Pick CDF index based on number of matching/out-of-bounds segment IDs.
    if (prev_ul < 0 || prev_u < 0 || prev_l < 0) { /* Edge case */
        *cdf_index = 0;
    } else if ((prev_ul == prev_u) && (prev_ul == prev_l)) {
        *cdf_index = 2;
    } else if ((prev_ul == prev_u) || (prev_ul == prev_l) || (prev_u == prev_l)) {
        *cdf_index = 1;
    } else {
        *cdf_index = 0;
    }

    // If 2 or more are identical returns that as predictor, otherwise prev_l.
    if (prev_u == -1) { // edge case
        return prev_l == -1 ? 0 : prev_l;
    }
    if (prev_l == -1) { // edge case
        return prev_u;
    }
    return (prev_ul == prev_u) ? prev_u : prev_l;
}

int svt_av1_neg_interleave(int x, int ref, int max) {
    assert(x < max);
    const int diff = x - ref;
    if (!ref) {
        return x;
    }
    if (ref >= (max - 1)) {
        return -x + max - 1;
    }
    if (2 * ref < max) {
        if (abs(diff) <= ref) {
            return diff > 0 ? (diff << 1) - 1 : ((-diff) << 1);
        }
        return x;
    } else {
        if (abs(diff) < (max - ref)) {
            return diff > 0 ? (diff << 1) - 1 : ((-diff) << 1);
        }
        return (max - x) - 1;
    }
}

void svt_av1_update_segmentation_map(PictureControlSet* pcs, BlockSize bsize, uint32_t blk_org_x, uint32_t blk_org_y,
                                     uint8_t segment_id) {
    Av1Common* cm          = pcs->ppcs->av1_cm;
    uint8_t*   segment_ids = pcs->segmentation_neighbor_map->data;
    uint32_t   mi_col      = blk_org_x >> MI_SIZE_LOG2;
    uint32_t   mi_row      = blk_org_y >> MI_SIZE_LOG2;
    const int  mi_offset   = mi_row * cm->mi_cols + mi_col;
    const int  bw          = mi_size_wide[bsize];
    const int  bh          = mi_size_high[bsize];
    const int  xmis        = AOMMIN((int)(cm->mi_cols - mi_col), bw);
    const int  ymis        = AOMMIN((int)(cm->mi_rows - mi_row), bh);
    int        x, y;

    for (y = 0; y < ymis; ++y) {
        for (x = 0; x < xmis; ++x) {
            segment_ids[mi_offset + y * cm->mi_cols + x] = segment_id;
        }
    }
}

void write_segment_id(PictureControlSet* pcs, FRAME_CONTEXT* frame_context, AomWriter* ecWriter, BlockSize bsize,
                      uint32_t blk_org_x, uint32_t blk_org_y, EcBlkStruct* blk_ptr, bool skip_coeff) {
    SegmentationParams* segmentation_params = &pcs->ppcs->frm_hdr.segmentation_params;
    if (!segmentation_params->segmentation_enabled) {
        return;
    }
    MbModeInfo* mbmi = get_mbmi(pcs, blk_org_x, blk_org_y);
    int         cdf_num;
    const int   spatial_pred = svt_av1_get_spatial_seg_prediction(pcs, blk_ptr->av1xd, blk_org_x, blk_org_y, &cdf_num);
    if (skip_coeff) {
        svt_av1_update_segmentation_map(pcs, bsize, blk_org_x, blk_org_y, spatial_pred);
        mbmi->segment_id = spatial_pred;
        return;
    }
    const int coded_id = svt_av1_neg_interleave(
        mbmi->segment_id, spatial_pred, segmentation_params->last_active_seg_id + 1);
    struct segmentation_probs* segp     = &frame_context->seg;
    AomCdfProb*                pred_cdf = segp->spatial_pred_seg_cdf[cdf_num];
    aom_write_symbol(ecWriter, coded_id, pred_cdf, MAX_SEGMENTS);
    svt_av1_update_segmentation_map(pcs, bsize, blk_org_x, blk_org_y, mbmi->segment_id);
}

static void write_inter_segment_id(PictureControlSet* pcs, FRAME_CONTEXT* frame_context, AomWriter* ecWriter,
                                   const BlockSize bsize, uint32_t blk_org_x, uint32_t blk_org_y, EcBlkStruct* blk_ptr,
                                   bool skip, int pre_skip) {
    SegmentationParams* segmentation_params = &pcs->ppcs->frm_hdr.segmentation_params;
    if (!segmentation_params->segmentation_enabled) {
        return;
    }

    if (segmentation_params->segmentation_update_map) {
        if (pre_skip) {
            if (!segmentation_params->seg_id_pre_skip) {
                return;
            }
        } else {
            if (segmentation_params->seg_id_pre_skip) {
                return;
            }
            if (skip) {
                write_segment_id(pcs, frame_context, ecWriter, bsize, blk_org_x, blk_org_y, blk_ptr, 1);
                if (segmentation_params->segmentation_temporal_update) {
                    SVT_ERROR("Temporal update is not supported yet! \n");
                    assert(0);
                    //                    blk_ptr->seg_id_predicted = 0;
                }
                return;
            }
        }

        if (segmentation_params->segmentation_temporal_update) {
            SVT_ERROR("Temporal update is not supported yet! \n");
            assert(0);

        } else {
            write_segment_id(pcs, frame_context, ecWriter, bsize, blk_org_x, blk_org_y, blk_ptr, 0);
        }
    }
}

int svt_aom_is_interintra_allowed(const MbModeInfo* mbmi) {
    return svt_aom_is_interintra_allowed_bsize(mbmi->bsize) &&
        svt_aom_is_interintra_allowed_mode(mbmi->block_mi.mode) &&
        svt_aom_is_interintra_allowed_ref(mbmi->block_mi.ref_frame);
}

int svt_aom_is_interintra_wedge_used(BlockSize bsize);

static EbErrorType write_modes_b(PictureControlSet* pcs, EntropyCodingContext* ec_ctx, EntropyCoder* ec,
                                 SuperBlock* sb_ptr, EcBlkStruct* blk_ptr, uint16_t tile_idx,
                                 EbPictureBufferDesc* coeff_ptr, const int mi_row, const int mi_col) {
    EbErrorType         return_error  = EB_ErrorNone;
    FRAME_CONTEXT*      frame_context = ec->fc;
    AomWriter*          ec_writer     = &ec->ec_writer;
    SequenceControlSet* scs           = pcs->scs;
    FrameHeader*        frm_hdr       = &pcs->ppcs->frm_hdr;

    NeighborArrayUnit* luma_dc_sign_level_coeff_na = pcs->luma_dc_sign_level_coeff_na[tile_idx];
    NeighborArrayUnit* cr_dc_sign_level_coeff_na   = pcs->cr_dc_sign_level_coeff_na[tile_idx];
    NeighborArrayUnit* cb_dc_sign_level_coeff_na   = pcs->cb_dc_sign_level_coeff_na[tile_idx];
    NeighborArrayUnit* txfm_context_array          = pcs->txfm_context_array[tile_idx];
    const uint32_t     blk_org_x                   = mi_col << MI_SIZE_LOG2;
    const uint32_t     blk_org_y                   = mi_row << MI_SIZE_LOG2;
    MbModeInfo*        mbmi                        = get_mbmi(pcs, blk_org_x, blk_org_y);
    const BlockSize    bsize                       = mbmi->bsize;
    const int          bwidth                      = block_size_wide[bsize];
    const int          bheight                     = block_size_high[bsize];
    bool               skip_coeff                  = mbmi->block_mi.skip;
    const bool         has_uv                      = is_chroma_reference(blk_org_y >> 2, blk_org_x >> 2, bsize, 1, 1);
    ec_ctx->mbmi                                   = mbmi;

    const uint8_t skip_mode = mbmi->block_mi.skip_mode;

    assert(bsize < BLOCK_SIZES_ALL);
    int           mi_stride           = pcs->ppcs->av1_cm->mi_stride;
    const int32_t offset              = mi_row * mi_stride + mi_col;
    blk_ptr->av1xd->mi                = pcs->mi_grid_base + offset;
    blk_ptr->av1xd->tile.mi_col_start = sb_ptr->tile_info.mi_col_start;
    blk_ptr->av1xd->tile.mi_col_end   = sb_ptr->tile_info.mi_col_end;
    blk_ptr->av1xd->tile.mi_row_start = sb_ptr->tile_info.mi_row_start;
    blk_ptr->av1xd->tile.mi_row_end   = sb_ptr->tile_info.mi_row_end;
    blk_ptr->av1xd->up_available      = (mi_row > sb_ptr->tile_info.mi_row_start);
    blk_ptr->av1xd->left_available    = (mi_col > sb_ptr->tile_info.mi_col_start);
    if (blk_ptr->av1xd->up_available) {
        blk_ptr->av1xd->above_mbmi = blk_ptr->av1xd->mi[-mi_stride];
    } else {
        blk_ptr->av1xd->above_mbmi = NULL;
    }
    if (blk_ptr->av1xd->left_available) {
        blk_ptr->av1xd->left_mbmi = blk_ptr->av1xd->mi[-1];
    } else {
        blk_ptr->av1xd->left_mbmi = NULL;
    }
    blk_ptr->av1xd->tile_ctx = frame_context;

    const int32_t bw = mi_size_wide[bsize];
    const int32_t bh = mi_size_high[bsize];
    set_mi_row_col(pcs,
                   blk_ptr->av1xd,
                   &blk_ptr->av1xd->tile,
                   mi_row,
                   bh,
                   mi_col,
                   bw,
                   mi_stride,
                   pcs->ppcs->av1_cm->mi_rows,
                   pcs->ppcs->av1_cm->mi_cols);
    if (pcs->slice_type == I_SLICE) {
        //const int32_t skip = write_skip(cm, xd, mbmi->segment_id, mi, w)

        if (pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled &&
            pcs->ppcs->frm_hdr.segmentation_params.seg_id_pre_skip) {
            write_segment_id(pcs, frame_context, ec_writer, bsize, blk_org_x, blk_org_y, blk_ptr, skip_coeff);
        }

        encode_skip_coeff_av1(blk_ptr, frame_context, ec_writer, skip_coeff);

        if (pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled &&
            !pcs->ppcs->frm_hdr.segmentation_params.seg_id_pre_skip) {
            write_segment_id(pcs, frame_context, ec_writer, bsize, blk_org_x, blk_org_y, blk_ptr, skip_coeff);
        }

        write_cdef(scs, pcs, ec_ctx, ec_writer, skip_coeff, blk_org_x >> MI_SIZE_LOG2, blk_org_y >> MI_SIZE_LOG2);
        if (pcs->ppcs->frm_hdr.delta_q_params.delta_q_present) {
            int32_t current_q_index        = blk_ptr->qindex;
            int32_t super_block_upper_left = (((blk_org_y >> 2) & (scs->seq_header.sb_mi_size - 1)) == 0) &&
                (((blk_org_x >> 2) & (scs->seq_header.sb_mi_size - 1)) == 0);
            if ((bsize != scs->seq_header.sb_size || skip_coeff == 0) && super_block_upper_left) {
                assert(current_q_index > 0);
                int32_t reduced_delta_qindex = (current_q_index - pcs->ppcs->prev_qindex[tile_idx]) /
                    frm_hdr->delta_q_params.delta_q_res;

                //write_delta_qindex(xd, reduced_delta_qindex, w);
                av1_write_delta_q_index(frame_context, reduced_delta_qindex, ec_writer);
                /*if (pcs->picture_number == 0){
                SVT_LOG("%d\t%d\t%d\t%d\n",
                blk_org_x,
                blk_org_y,
                current_q_index,
                pcs->ppcs->prev_qindex);
                }*/
                pcs->ppcs->prev_qindex[tile_idx] = current_q_index;
            }
        }

        {
            const uint32_t intra_luma_mode   = mbmi->block_mi.mode;
            uint32_t       intra_chroma_mode = mbmi->block_mi.uv_mode;
            if (svt_aom_allow_intrabc(&pcs->ppcs->frm_hdr, pcs->ppcs->slice_type)) {
                write_intrabc_info(frame_context, mbmi, blk_ptr, ec_writer);
            }
            if (mbmi->block_mi.use_intrabc == 0) {
                encode_intra_luma_mode_kf_av1(frame_context, ec_writer, mbmi, blk_ptr, bsize, intra_luma_mode);
            }
            if (mbmi->block_mi.use_intrabc == 0) {
                if (has_uv) {
                    encode_intra_chroma_mode_av1(frame_context,
                                                 ec_writer,
                                                 mbmi,
                                                 bsize,
                                                 intra_luma_mode,
                                                 intra_chroma_mode,
                                                 bwidth <= 32 && bheight <= 32);
                }
            }
            if (mbmi->block_mi.use_intrabc == 0 && svt_aom_allow_palette(frm_hdr->allow_screen_content_tools, bsize)) {
                write_palette_mode_info(

                    pcs->ppcs,
                    frame_context,
                    mbmi,
                    blk_ptr,
                    bsize,
                    blk_org_y >> MI_SIZE_LOG2,
                    blk_org_x >> MI_SIZE_LOG2,
                    ec_writer);
            }
            if (mbmi->block_mi.use_intrabc == 0 &&
                svt_aom_filter_intra_allowed(
                    scs->seq_header.filter_intra_level, bsize, blk_ptr->palette_size[0], intra_luma_mode)) {
                aom_write_symbol(ec_writer,
                                 mbmi->block_mi.filter_intra_mode != FILTER_INTRA_MODES,
                                 frame_context->filter_intra_cdfs[bsize],
                                 2);
                if (mbmi->block_mi.filter_intra_mode != FILTER_INTRA_MODES) {
                    aom_write_symbol(ec_writer,
                                     mbmi->block_mi.filter_intra_mode,
                                     frame_context->filter_intra_mode_cdf,
                                     FILTER_INTRA_MODES);
                }
            }
            if (mbmi->block_mi.use_intrabc == 0) {
                assert(blk_ptr->palette_size[1] == 0);
                TOKENEXTRA* tok = ec_ctx->tok;
                for (int plane = 0; plane < 2; ++plane) {
                    const uint8_t palette_size_plane = blk_ptr->palette_size[plane];
                    if (palette_size_plane > 0) {
                        const TxSize tx_size = tx_depth_to_tx_size[mbmi->block_mi.tx_depth][mbmi->bsize];
                        svt_av1_tokenize_color_map(
                            frame_context,
                            blk_ptr,
                            plane,
                            &tok,
                            bsize,
                            tx_size,
                            PALETTE_MAP,
                            0); //NO CDF update in entropy, the update will take place in arithmetic encode
                        assert(mbmi->block_mi.use_intrabc == 0);
                        assert(svt_aom_allow_palette(pcs->ppcs->frm_hdr.allow_screen_content_tools, bsize));
                        int rows, cols;
                        svt_aom_get_block_dimensions(bsize, plane, blk_ptr->av1xd, NULL, NULL, &rows, &cols);
                        pack_map_tokens(ec_writer, (const TOKENEXTRA**)(&ec_ctx->tok), palette_size_plane, rows * cols);
                        // advance the pointer
                        ec_ctx->tok = tok;
                    }
                }
            }
            if (frm_hdr->tx_mode == TX_MODE_SELECT) {
                code_tx_size(pcs,
                             blk_org_x,
                             blk_org_y,
                             blk_ptr,
                             bsize,
                             txfm_context_array,
                             frame_context,
                             ec_writer,
                             skip_coeff);
            }
            if (!skip_coeff) {
                av1_encode_coeff_1d(pcs,
                                    ec_ctx,
                                    frame_context,
                                    ec_writer,
                                    blk_ptr,
                                    blk_org_x,
                                    blk_org_y,
                                    intra_luma_mode,
                                    bsize,
                                    coeff_ptr,
                                    luma_dc_sign_level_coeff_na,
                                    cr_dc_sign_level_coeff_na,
                                    cb_dc_sign_level_coeff_na);
            }
        }
    } else {
        write_inter_segment_id(pcs, frame_context, ec_writer, bsize, blk_org_x, blk_org_y, blk_ptr, 0, 1);
        if (frm_hdr->skip_mode_params.skip_mode_flag && is_comp_ref_allowed(bsize)) {
            encode_skip_mode_av1(blk_ptr, frame_context, ec_writer, skip_mode);
        }
        if (!frm_hdr->skip_mode_params.skip_mode_flag && skip_mode) {
            SVT_ERROR("SKIP not supported\n");
        }
        if (!skip_mode) {
            // const int32_t skip = write_skip(cm, xd, mbmi->segment_id, mi, w);
            encode_skip_coeff_av1(blk_ptr, frame_context, ec_writer, skip_coeff);
        }

        write_inter_segment_id(pcs, frame_context, ec_writer, bsize, blk_org_x, blk_org_y, blk_ptr, skip_coeff, 0);
        write_cdef(scs,
                   pcs, /*cm,*/
                   ec_ctx,
                   ec_writer,
                   skip_mode ? 1 : skip_coeff,
                   blk_org_x >> MI_SIZE_LOG2,
                   blk_org_y >> MI_SIZE_LOG2);
        if (pcs->ppcs->frm_hdr.delta_q_params.delta_q_present) {
            int32_t current_q_index        = blk_ptr->qindex;
            int32_t super_block_upper_left = (((blk_org_y >> 2) & (scs->seq_header.sb_mi_size - 1)) == 0) &&
                (((blk_org_x >> 2) & (scs->seq_header.sb_mi_size - 1)) == 0);
            if ((bsize != scs->seq_header.sb_size || skip_coeff == 0) && super_block_upper_left) {
                assert(current_q_index > 0);
                int32_t reduced_delta_qindex = (current_q_index - pcs->ppcs->prev_qindex[tile_idx]) /
                    frm_hdr->delta_q_params.delta_q_res;
                av1_write_delta_q_index(frame_context, reduced_delta_qindex, ec_writer);
                pcs->ppcs->prev_qindex[tile_idx] = current_q_index;
            }
        }
        if (frm_hdr->tx_mode == TX_MODE_SELECT) {
            if (skip_mode) {
                code_tx_size(
                    pcs, blk_org_x, blk_org_y, blk_ptr, bsize, txfm_context_array, frame_context, ec_writer, skip_mode);
            }
        }
        if (!skip_mode) {
            write_is_inter(blk_ptr, frame_context, ec_writer, (int32_t)is_inter_mode(ec_ctx->mbmi->block_mi.mode));
            if (is_intra_mode(ec_ctx->mbmi->block_mi.mode)) {
                uint32_t intra_luma_mode = mbmi->block_mi.mode;

                uint32_t intra_chroma_mode = mbmi->block_mi.uv_mode;

                encode_intra_luma_mode_nonkey_av1(frame_context, ec_writer, mbmi, bsize, intra_luma_mode);
                if (has_uv) {
                    encode_intra_chroma_mode_av1(frame_context,
                                                 ec_writer,
                                                 mbmi,
                                                 bsize,
                                                 intra_luma_mode,
                                                 intra_chroma_mode,
                                                 bwidth <= 32 && bheight <= 32);
                }
                if (svt_aom_allow_palette(pcs->ppcs->frm_hdr.allow_screen_content_tools, bsize)) {
                    write_palette_mode_info(pcs->ppcs,
                                            frame_context,
                                            mbmi,
                                            blk_ptr,
                                            bsize,
                                            blk_org_y >> MI_SIZE_LOG2,
                                            blk_org_x >> MI_SIZE_LOG2,
                                            ec_writer);
                }
                if (svt_aom_filter_intra_allowed(
                        scs->seq_header.filter_intra_level, bsize, blk_ptr->palette_size[0], intra_luma_mode)) {
                    aom_write_symbol(ec_writer,
                                     mbmi->block_mi.filter_intra_mode != FILTER_INTRA_MODES,
                                     frame_context->filter_intra_cdfs[bsize],
                                     2);
                    if (mbmi->block_mi.filter_intra_mode != FILTER_INTRA_MODES) {
                        aom_write_symbol(ec_writer,
                                         mbmi->block_mi.filter_intra_mode,
                                         frame_context->filter_intra_mode_cdf,
                                         FILTER_INTRA_MODES);
                    }
                }

            } else {
                svt_aom_collect_neighbors_ref_counts_new(blk_ptr->av1xd);

                write_ref_frames(pcs->ppcs, blk_ptr->av1xd, ec_writer);

                MvReferenceFrame* rf          = mbmi->block_mi.ref_frame;
                int16_t           mode_ctx    = svt_aom_mode_context_analyzer(blk_ptr->inter_mode_ctx, rf);
                PredictionMode    inter_mode  = mbmi->block_mi.mode;
                const int32_t     is_compound = is_inter_compound_mode(inter_mode);

                // If segment skip is not enabled code the mode.
                if (is_inter_compound_mode(inter_mode)) {
                    write_inter_compound_mode(frame_context, ec_writer, inter_mode, mode_ctx);
                } else if (is_inter_singleref_mode(inter_mode)) {
                    write_inter_mode(frame_context, ec_writer, inter_mode, mode_ctx, blk_org_x, blk_org_y);
                }

                if (inter_mode == NEWMV || inter_mode == NEW_NEWMV || have_nearmv_in_inter_mode(inter_mode)) {
                    write_drl_idx(frame_context, ec_writer, mbmi, blk_ptr);
                }

                if (inter_mode == NEWMV || inter_mode == NEW_NEWMV) {
                    Mv ref_mv;

                    for (uint8_t ref = 0; ref < 1 + is_compound; ++ref) {
                        NmvContext* nmvc = &frame_context->nmvc;
                        ref_mv           = blk_ptr->predmv[ref];

                        Mv mv = mbmi->block_mi.mv[ref];

                        svt_av1_encode_mv(pcs->ppcs, ec_writer, &mv, &ref_mv, nmvc, frm_hdr->allow_high_precision_mv);
                    }
                } else if (inter_mode == NEAREST_NEWMV || inter_mode == NEAR_NEWMV) {
                    NmvContext* nmvc   = &frame_context->nmvc;
                    Mv          ref_mv = blk_ptr->predmv[1];

                    Mv mv = mbmi->block_mi.mv[1];

                    svt_av1_encode_mv(pcs->ppcs, ec_writer, &mv, &ref_mv, nmvc, frm_hdr->allow_high_precision_mv);
                } else if (inter_mode == NEW_NEARESTMV || inter_mode == NEW_NEARMV) {
                    NmvContext* nmvc   = &frame_context->nmvc;
                    Mv          ref_mv = blk_ptr->predmv[0];

                    Mv mv = mbmi->block_mi.mv[0];

                    svt_av1_encode_mv(pcs->ppcs, ec_writer, &mv, &ref_mv, nmvc, frm_hdr->allow_high_precision_mv);
                }
                if (scs->seq_header.enable_interintra_compound && svt_aom_is_interintra_allowed(mbmi)) {
                    if (mbmi->block_mi.is_interintra_used) {
                        rf[1]                       = INTRA_FRAME;
                        mbmi->block_mi.ref_frame[1] = INTRA_FRAME;
                    }

                    const int interintra  = mbmi->block_mi.is_interintra_used;
                    const int bsize_group = eb_size_group_lookup[bsize];
                    aom_write_symbol(
                        ec_writer, mbmi->block_mi.is_interintra_used, frame_context->interintra_cdf[bsize_group], 2);
                    if (interintra) {
                        aom_write_symbol(ec_writer,
                                         mbmi->block_mi.interintra_mode,
                                         frame_context->interintra_mode_cdf[bsize_group],
                                         INTERINTRA_MODES);
                        if (svt_aom_is_interintra_wedge_used(bsize)) {
                            aom_write_symbol(ec_writer,
                                             mbmi->block_mi.use_wedge_interintra,
                                             frame_context->wedge_interintra_cdf[bsize],
                                             2);
                            if (mbmi->block_mi.use_wedge_interintra) {
                                aom_write_symbol(ec_writer,
                                                 mbmi->block_mi.interintra_wedge_index,
                                                 frame_context->wedge_idx_cdf[bsize],
                                                 16);
                            }
                        }
                    }
                }

                if (frm_hdr->is_motion_mode_switchable && rf[1] != INTRA_FRAME) {
                    write_motion_mode(
                        frame_context, ec_writer, bsize, mbmi, mbmi->block_mi.motion_mode, rf[0], rf[1], blk_ptr, pcs);
                }
                // First write idx to indicate current compound inter prediction mode group
                // Group A (0): dist_wtd_comp, compound_average
                // Group b (1): interintra, compound_diffwtd, wedge
                if (has_second_ref(&mbmi->block_mi)) {
                    const int masked_compound_used = is_any_masked_compound_used(bsize) &&
                        scs->seq_header.enable_masked_compound;

                    if (masked_compound_used) {
                        const int ctx_comp_group_idx = svt_aom_get_comp_group_idx_context_enc(blk_ptr->av1xd);
                        aom_write_symbol(ec_writer,
                                         mbmi->block_mi.comp_group_idx,
                                         frame_context->comp_group_idx_cdf[ctx_comp_group_idx],
                                         2);
                    } else {
                        assert(mbmi->block_mi.comp_group_idx == 0);
                    }

                    if (mbmi->block_mi.comp_group_idx == 0) {
                        assert(IMPLIES(mbmi->block_mi.compound_idx,
                                       mbmi->block_mi.interinter_comp.type == COMPOUND_AVERAGE));

                        if (scs->seq_header.order_hint_info.enable_jnt_comp) {
                            const int comp_index_ctx = svt_aom_get_comp_index_context_enc(
                                pcs->ppcs,
                                pcs->ppcs->cur_order_hint, // cur_frame_index,
                                pcs->ppcs->ref_order_hint[rf[0] - 1], // bck_frame_index,
                                pcs->ppcs->ref_order_hint[rf[1] - 1], // fwd_frame_index,
                                blk_ptr->av1xd);
                            aom_write_symbol(ec_writer,
                                             mbmi->block_mi.compound_idx,
                                             frame_context->compound_index_cdf[comp_index_ctx],
                                             2);
                        } else {
                            assert(mbmi->block_mi.compound_idx == 1);
                        }
                    } else {
                        assert(pcs->ppcs->frm_hdr.reference_mode != SINGLE_REFERENCE &&
                               is_inter_compound_mode(mbmi->block_mi.mode) &&
                               mbmi->block_mi.motion_mode == SIMPLE_TRANSLATION);
                        assert(masked_compound_used);
                        // compound_diffwtd, wedge
                        assert(mbmi->block_mi.interinter_comp.type == COMPOUND_WEDGE ||
                               mbmi->block_mi.interinter_comp.type == COMPOUND_DIFFWTD);

                        if (is_interinter_compound_used(COMPOUND_WEDGE, bsize)) {
                            aom_write_symbol(ec_writer,
                                             mbmi->block_mi.interinter_comp.type - COMPOUND_WEDGE,
                                             frame_context->compound_type_cdf[bsize],
                                             MASKED_COMPOUND_TYPES);
                        }

                        if (mbmi->block_mi.interinter_comp.type == COMPOUND_WEDGE) {
                            assert(is_interinter_compound_used(COMPOUND_WEDGE, bsize));
                            aom_write_symbol(ec_writer,
                                             mbmi->block_mi.interinter_comp.wedge_index,
                                             frame_context->wedge_idx_cdf[bsize],
                                             16);
                            aom_write_bit(ec_writer, mbmi->block_mi.interinter_comp.wedge_sign);
                        } else {
                            assert(mbmi->block_mi.interinter_comp.type == COMPOUND_DIFFWTD);
                            aom_write_literal(
                                ec_writer, mbmi->block_mi.interinter_comp.mask_type, MAX_DIFFWTD_MASK_BITS);
                        }
                    }
                }
                write_mb_interp_filter(bsize, rf[0], rf[1], pcs->ppcs, ec_writer, mbmi, blk_ptr, ec);
            }
            {
                assert(blk_ptr->palette_size[1] == 0);
                TOKENEXTRA* tok = ec_ctx->tok;
                for (int plane = 0; plane < 2; ++plane) {
                    const uint8_t palette_size_plane = blk_ptr->palette_size[plane];
                    if (palette_size_plane > 0) {
                        const TxSize tx_size = tx_depth_to_tx_size[mbmi->block_mi.tx_depth][mbmi->bsize];
                        svt_av1_tokenize_color_map(
                            frame_context,
                            blk_ptr,
                            plane,
                            &tok,
                            bsize,
                            tx_size,
                            PALETTE_MAP,
                            0); //NO CDF update in entropy, the update will take place in arithmetic encode
                        assert(mbmi->block_mi.use_intrabc == 0);
                        assert(svt_aom_allow_palette(pcs->ppcs->frm_hdr.allow_screen_content_tools, bsize));
                        int rows, cols;
                        svt_aom_get_block_dimensions(bsize, plane, blk_ptr->av1xd, NULL, NULL, &rows, &cols);
                        pack_map_tokens(ec_writer, (const TOKENEXTRA**)(&ec_ctx->tok), palette_size_plane, rows * cols);
                        // advance the pointer
                        ec_ctx->tok = tok;
                    }
                }
            }

            if (frm_hdr->tx_mode == TX_MODE_SELECT) {
                code_tx_size(pcs,
                             blk_org_x,
                             blk_org_y,
                             blk_ptr,
                             bsize,
                             txfm_context_array,
                             frame_context,
                             ec_writer,
                             skip_coeff);
            }
            if (!skip_coeff) {
                uint32_t intra_luma_mode = DC_PRED;
                if (is_intra_mode(ec_ctx->mbmi->block_mi.mode)) {
                    intra_luma_mode = mbmi->block_mi.mode;
                }

                {
                    av1_encode_coeff_1d(pcs,
                                        ec_ctx,
                                        frame_context,
                                        ec_writer,
                                        blk_ptr,
                                        blk_org_x,
                                        blk_org_y,
                                        intra_luma_mode,
                                        bsize,
                                        coeff_ptr,
                                        luma_dc_sign_level_coeff_na,
                                        cr_dc_sign_level_coeff_na,
                                        cb_dc_sign_level_coeff_na);
                }
            }
        }
    }
    svt_block_on_mutex(pcs->entropy_coding_pic_mutex);
    pcs->ppcs->tot_qindex += blk_ptr->qindex * bwidth * bheight;
    pcs->ppcs->valid_qindex_area += bwidth * bheight;
    svt_release_mutex(pcs->entropy_coding_pic_mutex);
    // Update the neighbors
    ec_update_neighbors(pcs, ec_ctx, blk_org_x, blk_org_y, tile_idx, bsize);

    if (svt_av1_allow_palette(pcs->ppcs->palette_level, bsize)) {
        // free ENCDEC palette info buffer
        assert(blk_ptr->palette_info->color_idx_map != NULL && "free palette:Null");
        EB_FREE(blk_ptr->palette_info->color_idx_map);
        blk_ptr->palette_info->color_idx_map = NULL;
        EB_FREE(blk_ptr->palette_info);
    }

    return return_error;
}

/**********************************************
 * Write sb
 **********************************************/
void svt_aom_write_modes_sb(EntropyCodingContext* ec_ctx, SuperBlock* sb_ptr, PictureControlSet* pcs, uint16_t tile_idx,
                            EntropyCoder* ec, EbPictureBufferDesc* coeff_ptr, PARTITION_TREE* ptree, int mi_row,
                            int mi_col) {
    if (mi_row >= pcs->ppcs->av1_cm->mi_rows || mi_col >= pcs->ppcs->av1_cm->mi_cols) {
        return;
    }
    FRAME_CONTEXT*     frame_context        = ec->fc;
    AomWriter*         ec_writer            = &ec->ec_writer;
    NeighborArrayUnit* partition_context_na = pcs->partition_context_na[tile_idx];

    const BlockSize bsize = ptree->bsize;
    assert(bsize < BLOCK_SIZES_ALL);
    const int           hbs          = mi_size_wide[bsize] >> 1;
    const int           quarter_step = mi_size_wide[bsize] >> 2;
    const PartitionType partition    = ptree->partition;
    Av1Common*          cm           = pcs->ppcs->av1_cm;

    if (bsize >= BLOCK_8X8) {
        for (int32_t plane = 0; plane < 3; ++plane) {
            int32_t rcol0, rcol1, rrow0, rrow1, tile_tl_idx;
            if (svt_av1_loop_restoration_corners_in_sb(cm,
                                                       &pcs->scs->seq_header,
                                                       plane,
                                                       mi_row,
                                                       mi_col,
                                                       bsize,
                                                       &rcol0,
                                                       &rcol1,
                                                       &rrow0,
                                                       &rrow1,
                                                       &tile_tl_idx)) {
                const int32_t rstride = pcs->rst_info[plane].horz_units_per_tile;
                for (int32_t rrow = rrow0; rrow < rrow1; ++rrow) {
                    for (int32_t rcol = rcol0; rcol < rcol1; ++rcol) {
                        const int32_t              runit_idx = tile_tl_idx + rcol + rrow * rstride;
                        const RestorationUnitInfo* rui       = &pcs->rst_info[plane].unit_info[runit_idx];
                        loop_restoration_write_sb_coeffs(pcs, frame_context, ec_ctx, rui, ec_writer, plane);
                    }
                }
            }
        }

        encode_partition_av1(pcs,
                             frame_context,
                             ec_writer,
                             bsize,
                             partition,
                             mi_col << MI_SIZE_LOG2,
                             mi_row << MI_SIZE_LOG2,
                             partition_context_na);
    }

    assert(IMPLIES(bsize == BLOCK_4X4, partition == PARTITION_NONE));
    assert(IMPLIES(partition != PARTITION_SPLIT, (mi_row + hbs < cm->mi_rows) || (mi_col + hbs < cm->mi_cols)));
    switch (partition) {
    case PARTITION_NONE:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        break;
    case PARTITION_HORZ:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        if (mi_row + hbs < cm->mi_rows) {
            write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[1], tile_idx, coeff_ptr, mi_row + hbs, mi_col);
        }
        break;
    case PARTITION_VERT:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        if (mi_col + hbs < cm->mi_cols) {
            write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[1], tile_idx, coeff_ptr, mi_row, mi_col + hbs);
        }
        break;
    case PARTITION_SPLIT:
        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            const int x_idx = (i & 1) * hbs;
            const int y_idx = (i >> 1) * hbs;
            if (mi_row + y_idx >= cm->mi_rows || mi_col + x_idx >= cm->mi_cols) {
                continue;
            }
            svt_aom_write_modes_sb(
                ec_ctx, sb_ptr, pcs, tile_idx, ec, coeff_ptr, ptree->sub_tree[i], mi_row + y_idx, mi_col + x_idx);
        }
        break;
    case PARTITION_HORZ_A:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[1], tile_idx, coeff_ptr, mi_row, mi_col + hbs);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[2], tile_idx, coeff_ptr, mi_row + hbs, mi_col);
        break;
    case PARTITION_HORZ_B:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[1], tile_idx, coeff_ptr, mi_row + hbs, mi_col);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[2], tile_idx, coeff_ptr, mi_row + hbs, mi_col + hbs);
        break;
    case PARTITION_VERT_A:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[1], tile_idx, coeff_ptr, mi_row + hbs, mi_col);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[2], tile_idx, coeff_ptr, mi_row, mi_col + hbs);
        break;
    case PARTITION_VERT_B:
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[0], tile_idx, coeff_ptr, mi_row, mi_col);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[1], tile_idx, coeff_ptr, mi_row, mi_col + hbs);
        write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[2], tile_idx, coeff_ptr, mi_row + hbs, mi_col + hbs);
        break;
    case PARTITION_HORZ_4:
        for (int i = 0; i < SUB_PARTITIONS_PART4; ++i) {
            int this_mi_row = mi_row + i * quarter_step;
            if (i > 0 && this_mi_row >= cm->mi_rows) {
                // Only the last block is able to be outside the picture boundary. If one of the first
                // 3 blocks is outside the boundary, H4 is not a valid partition (see AV1 spec 5.11.4)
                assert(i == 3);
                break;
            }
            write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[i], tile_idx, coeff_ptr, this_mi_row, mi_col);
        }
        break;
    case PARTITION_VERT_4:
        for (int i = 0; i < SUB_PARTITIONS_PART4; ++i) {
            int this_mi_col = mi_col + i * quarter_step;
            if (i > 0 && this_mi_col >= cm->mi_cols) {
                // Only the last block is able to be outside the picture boundary. If one of the first
                // 3 blocks is outside the boundary, H4 is not a valid partition (see AV1 spec 5.11.4)
                assert(i == 3);
                break;
            }
            write_modes_b(pcs, ec_ctx, ec, sb_ptr, ptree->blk_data[i], tile_idx, coeff_ptr, mi_row, this_mi_col);
        }
        break;
    default:
        assert(0);
    }
}
