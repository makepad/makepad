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
#include "enc_intra_prediction.h"
#include "md_process.h"
#include "common_dsp_rtcd.h"
#include "transforms.h"

static int get_filt_type(const MacroBlockD* xd, int plane) {
    int ab_sm, le_sm;

    if (plane == 0) {
        const MbModeInfo* ab = xd->above_mbmi;
        const MbModeInfo* le = xd->left_mbmi;
        ab_sm                = ab ? svt_aom_is_smooth(&ab->block_mi, plane) : 0;
        le_sm                = le ? svt_aom_is_smooth(&le->block_mi, plane) : 0;
    } else {
        const MbModeInfo* ab = xd->chroma_above_mbmi;
        const MbModeInfo* le = xd->chroma_left_mbmi;
        ab_sm                = ab ? svt_aom_is_smooth(&ab->block_mi, plane) : 0;
        le_sm                = le ? svt_aom_is_smooth(&le->block_mi, plane) : 0;
    }

    return (ab_sm || le_sm) ? 1 : 0;
}

////////////#####################...........Recurssive intra prediction ending...........#####################////////////

static void build_intra_predictors(const MacroBlockD* xd, uint8_t* top_neigh_array, uint8_t* left_neigh_array,
                                   // const uint8_t *ref,    int32_t ref_stride,
                                   uint8_t* dst, int32_t dst_stride, PredictionMode mode, int32_t angle_delta,
                                   FilterIntraMode filter_intra_mode, TxSize tx_size, int32_t disable_edge_filter,
                                   int32_t n_top_px, int32_t n_topright_px, int32_t n_left_px, int32_t n_bottomleft_px,
                                   int32_t plane) {
    int32_t i;

    int32_t        ref_stride = 1;
    const uint8_t* above_ref  = top_neigh_array; //CHKN ref - ref_stride;
    const uint8_t* left_ref   = left_neigh_array; //CHKN ref - 1;
    DECLARE_ALIGNED(32, uint8_t, left_data[MAX_TX_SIZE * 2 + 48]);
    DECLARE_ALIGNED(32, uint8_t, above_data[MAX_TX_SIZE * 2 + 48]);
    memset(left_data, 0x80, sizeof(left_data));
    memset(above_data, 0x80, sizeof(above_data));
    uint8_t* const above_row = above_data + 32;
    uint8_t* const left_col  = left_data + 32;

    const int32_t txwpx            = tx_size_wide[tx_size];
    const int32_t txhpx            = tx_size_high[tx_size];
    int32_t       need_left        = extend_modes[mode] & NEED_LEFT;
    int32_t       need_above       = extend_modes[mode] & NEED_ABOVE;
    int32_t       need_above_left  = extend_modes[mode] & NEED_ABOVELEFT;
    int32_t       p_angle          = 0;
    const int32_t is_dr_mode       = av1_is_directional_mode(mode);
    const int32_t use_filter_intra = filter_intra_mode != FILTER_INTRA_MODES;

    if (is_dr_mode) {
        p_angle = mode_to_angle_map[mode] + angle_delta * ANGLE_STEP;
        if (p_angle <= 90) {
            need_above = 1, need_left = 0, need_above_left = 1;
        } else if (p_angle < 180) {
            need_above = 1, need_left = 1, need_above_left = 1;
        } else {
            need_above = 0, need_left = 1, need_above_left = 1;
        }
    }
    if (use_filter_intra) {
        need_left = need_above = need_above_left = 1;
    }

    assert(n_top_px >= 0);
    assert(n_topright_px >= 0);
    assert(n_left_px >= 0);
    assert(n_bottomleft_px >= 0);

    if ((!need_above && n_left_px == 0) || (!need_left && n_top_px == 0)) {
        int32_t val;
        if (need_left) {
            val = (n_top_px > 0) ? above_ref[0] : 129;
        } else {
            val = (n_left_px > 0) ? left_ref[0] : 127;
        }
        for (i = 0; i < txhpx; ++i) {
            memset(dst, val, txwpx);
            dst += dst_stride;
        }
        return;
    }

    // NEED_LEFT
    if (need_left) {
        int32_t need_bottom = !!(extend_modes[mode] & NEED_BOTTOMLEFT);
        if (use_filter_intra) {
            need_bottom = 0;
        }
        if (is_dr_mode) {
            need_bottom = p_angle > 180;
        }
        const int32_t num_left_pixels_needed = txhpx + (need_bottom ? txwpx : 0);
        i                                    = 0;
        if (n_left_px > 0) {
            for (; i < n_left_px; i++) {
                left_col[i] = left_ref[i * ref_stride];
            }
            if (need_bottom && n_bottomleft_px > 0) {
                assert(i == txhpx);
                for (; i < txhpx + n_bottomleft_px; i++) {
                    left_col[i] = left_ref[i * ref_stride];
                }
            }
            if (i < num_left_pixels_needed) {
                memset(&left_col[i], left_col[i - 1], num_left_pixels_needed - i);
            }
        } else {
            if (n_top_px > 0) {
                memset(left_col, above_ref[0], num_left_pixels_needed);
            } else {
                memset(left_col, 129, num_left_pixels_needed);
            }
        }
    }

    // NEED_ABOVE
    if (need_above) {
        int32_t need_right = !!(extend_modes[mode] & NEED_ABOVERIGHT);
        if (use_filter_intra) {
            need_right = 0;
        }
        if (is_dr_mode) {
            need_right = p_angle < 90;
        }
        const int32_t num_top_pixels_needed = txwpx + (need_right ? txhpx : 0);
        if (n_top_px > 0) {
            svt_memcpy(above_row, above_ref, n_top_px);
            i = n_top_px;
            if (need_right && n_topright_px > 0) {
                assert(n_top_px == txwpx);
                svt_memcpy(above_row + txwpx, above_ref + txwpx, n_topright_px);
                i += n_topright_px;
            }
            if (i < num_top_pixels_needed) {
                memset(&above_row[i], above_row[i - 1], num_top_pixels_needed - i);
            }
        } else {
            if (n_left_px > 0) {
                memset(above_row, left_ref[0], num_top_pixels_needed);
            } else {
                memset(above_row, 127, num_top_pixels_needed);
            }
        }
    }

    if (need_above_left) {
        if (n_top_px > 0 && n_left_px > 0) {
            above_row[-1] = above_ref[-1];
        } else if (n_top_px > 0) {
            above_row[-1] = above_ref[0];
        } else if (n_left_px > 0) {
            above_row[-1] = left_ref[0];
        } else {
            above_row[-1] = 128;
        }
        left_col[-1] = above_row[-1];
    }
    if (use_filter_intra) {
        svt_av1_filter_intra_predictor(dst, dst_stride, tx_size, above_row, left_col, filter_intra_mode);
        return;
    }

    if (is_dr_mode) {
        int32_t upsample_above = 0;
        int32_t upsample_left  = 0;
        if (!disable_edge_filter) {
            const int32_t need_right  = p_angle < 90;
            const int32_t need_bottom = p_angle > 180;
            const int32_t filt_type   = get_filt_type(xd, plane);

            if (p_angle != 90 && p_angle != 180) {
                const int32_t ab_le = need_above_left ? 1 : 0;
                if (need_above && need_left && (txwpx + txhpx >= 24)) {
                    filter_intra_edge_corner(above_row, left_col);
                }
                if (need_above && n_top_px > 0) {
                    const int32_t strength = svt_aom_intra_edge_filter_strength(txwpx, txhpx, p_angle - 90, filt_type);
                    const int32_t n_px     = n_top_px + ab_le + (need_right ? txhpx : 0);
                    svt_av1_filter_intra_edge(above_row - ab_le, n_px, strength);
                }
                if (need_left && n_left_px > 0) {
                    const int32_t strength = svt_aom_intra_edge_filter_strength(txhpx, txwpx, p_angle - 180, filt_type);
                    const int32_t n_px     = n_left_px + ab_le + (need_bottom ? txwpx : 0);
                    svt_av1_filter_intra_edge(left_col - ab_le, n_px, strength);
                }
            }
            upsample_above = svt_aom_use_intra_edge_upsample(txwpx, txhpx, p_angle - 90, filt_type);
            if (need_above && upsample_above) {
                const int32_t n_px = txwpx + (need_right ? txhpx : 0);
                svt_av1_upsample_intra_edge(above_row, n_px);
            }
            upsample_left = svt_aom_use_intra_edge_upsample(txhpx, txwpx, p_angle - 180, filt_type);
            if (need_left && upsample_left) {
                const int32_t n_px = txhpx + (need_bottom ? txwpx : 0);
                svt_av1_upsample_intra_edge(left_col, n_px);
            }
        }
        svt_aom_dr_predictor(dst, dst_stride, tx_size, above_row, left_col, upsample_above, upsample_left, p_angle);
        return;
    }

    // predict
    if (mode == DC_PRED) {
        svt_aom_dc_pred[n_left_px > 0][n_top_px > 0][tx_size](dst, dst_stride, above_row, left_col);
    } else {
        svt_aom_eb_pred[mode][tx_size](dst, dst_stride, above_row, left_col);
    }
}
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static void build_intra_predictors_high(const MacroBlockD* xd,
                                        uint16_t*          top_neigh_array, // int8_t
                                        uint16_t*          left_neigh_array, // int8_t
                                        //const uint8_t *ref8, int32_t ref_stride,
                                        uint16_t* dst, //uint8_t *dst8
                                        int32_t dst_stride, PredictionMode mode, int32_t angle_delta,
                                        FilterIntraMode filter_intra_mode, TxSize tx_size, int32_t disable_edge_filter,
                                        int32_t n_top_px, int32_t n_topright_px, int32_t n_left_px,
                                        int32_t n_bottomleft_px, int32_t plane, int32_t bd) {
    (void)xd;
    int32_t i;
    //uint16_t *dst = CONVERT_TO_SHORTPTR(dst8);
    //uint16_t *ref = CONVERT_TO_SHORTPTR(ref8);

    DECLARE_ALIGNED(16, uint16_t, left_data[MAX_TX_SIZE * 2 + 32]);
    DECLARE_ALIGNED(16, uint16_t, above_data[MAX_TX_SIZE * 2 + 32]);
    memset(left_data, 0x80, sizeof(left_data));
    memset(above_data, 0x80, sizeof(above_data));
    uint16_t* const above_row       = above_data + 16;
    uint16_t* const left_col        = left_data + 16;
    const int32_t   txwpx           = tx_size_wide[tx_size];
    const int32_t   txhpx           = tx_size_high[tx_size];
    int32_t         need_left       = extend_modes[mode] & NEED_LEFT;
    int32_t         need_above      = extend_modes[mode] & NEED_ABOVE;
    int32_t         need_above_left = extend_modes[mode] & NEED_ABOVELEFT;

    int32_t         ref_stride = 1;
    const uint16_t* above_ref  = top_neigh_array;
    const uint16_t* left_ref   = left_neigh_array;
    //const uint16_t *above_ref = ref - ref_stride;
    //const uint16_t *left_ref = ref - 1;
    int32_t       p_angle          = 0;
    const int32_t is_dr_mode       = av1_is_directional_mode(mode);
    const int32_t use_filter_intra = filter_intra_mode != FILTER_INTRA_MODES;
    int32_t       base             = 128 << (bd - 8);

    // The default values if ref pixels are not available:
    // base-1 base-1 base-1 .. base-1 base-1 base-1 base-1 base-1 base-1
    // base+1   A      b  ..     Y      Z
    // base+1   C      D  ..     W      X
    // base+1   E      F  ..     U      V
    // base+1   G      H  ..     S      T      T      T      T      T

    if (is_dr_mode) {
        p_angle = mode_to_angle_map[mode] + angle_delta * ANGLE_STEP;
        if (p_angle <= 90) {
            need_above = 1, need_left = 0, need_above_left = 1;
        } else if (p_angle < 180) {
            need_above = 1, need_left = 1, need_above_left = 1;
        } else {
            need_above = 0, need_left = 1, need_above_left = 1;
        }
    }
    if (use_filter_intra) {
        need_left = need_above = need_above_left = 1;
    }

    assert(n_top_px >= 0);
    assert(n_topright_px >= 0);
    assert(n_left_px >= 0);
    assert(n_bottomleft_px >= 0);

    if ((!need_above && n_left_px == 0) || (!need_left && n_top_px == 0)) {
        int32_t val;
        if (need_left) {
            val = (n_top_px > 0) ? above_ref[0] : base + 1;
        } else {
            val = (n_left_px > 0) ? left_ref[0] : base - 1;
        }
        for (i = 0; i < txhpx; ++i) {
            svt_aom_memset16(dst, val, txwpx);
            dst += dst_stride;
        }
        return;
    }

    // NEED_LEFT
    if (need_left) {
        int32_t need_bottom = !!(extend_modes[mode] & NEED_BOTTOMLEFT);
        if (use_filter_intra) {
            need_bottom = 0;
        }
        if (is_dr_mode) {
            need_bottom = p_angle > 180;
        }
        const int32_t num_left_pixels_needed = txhpx + (need_bottom ? txwpx : 0);
        i                                    = 0;
        if (n_left_px > 0) {
            for (; i < n_left_px; i++) {
                left_col[i] = left_ref[i * ref_stride];
            }
            if (need_bottom && n_bottomleft_px > 0) {
                assert(i == txhpx);
                for (; i < txhpx + n_bottomleft_px; i++) {
                    left_col[i] = left_ref[i * ref_stride];
                }
            }
            if (i < num_left_pixels_needed) {
                svt_aom_memset16(&left_col[i], left_col[i - 1], num_left_pixels_needed - i);
            }
        } else {
            if (n_top_px > 0) {
                svt_aom_memset16(left_col, above_ref[0], num_left_pixels_needed);
            } else {
                svt_aom_memset16(left_col, base + 1, num_left_pixels_needed);
            }
        }
    }

    // NEED_ABOVE
    if (need_above) {
        int32_t need_right = !!(extend_modes[mode] & NEED_ABOVERIGHT);
        if (use_filter_intra) {
            need_right = 0;
        }
        if (is_dr_mode) {
            need_right = p_angle < 90;
        }
        const int32_t num_top_pixels_needed = txwpx + (need_right ? txhpx : 0);
        if (n_top_px > 0) {
            svt_memcpy(above_row, above_ref, n_top_px * sizeof(above_ref[0]));
            i = n_top_px;
            if (need_right && n_topright_px > 0) {
                assert(n_top_px == txwpx);
                svt_memcpy(above_row + txwpx, above_ref + txwpx, n_topright_px * sizeof(above_ref[0]));
                i += n_topright_px;
            }
            if (i < num_top_pixels_needed) {
                svt_aom_memset16(&above_row[i], above_row[i - 1], num_top_pixels_needed - i);
            }
        } else {
            if (n_left_px > 0) {
                svt_aom_memset16(above_row, left_ref[0], num_top_pixels_needed);
            } else {
                svt_aom_memset16(above_row, base - 1, num_top_pixels_needed);
            }
        }
    }

    if (need_above_left) {
        if (n_top_px > 0 && n_left_px > 0) {
            above_row[-1] = above_ref[-1];
        } else if (n_top_px > 0) {
            above_row[-1] = above_ref[0];
        } else if (n_left_px > 0) {
            above_row[-1] = left_ref[0];
        } else {
            above_row[-1] = (uint16_t)base;
        }
        left_col[-1] = above_row[-1];
    }
    if (use_filter_intra) {
        svt_aom_highbd_filter_intra_predictor(dst, dst_stride, tx_size, above_row, left_col, filter_intra_mode, bd);
        return;
    }
    if (is_dr_mode) {
        int32_t upsample_above = 0;
        int32_t upsample_left  = 0;
        if (!disable_edge_filter) {
            const int32_t need_right  = p_angle < 90;
            const int32_t need_bottom = p_angle > 180;
            //const int32_t filt_type = get_filt_type(xd, plane);
            const int32_t filt_type = get_filt_type(xd, plane);
            if (p_angle != 90 && p_angle != 180) {
                const int32_t ab_le = need_above_left ? 1 : 0;
                if (need_above && need_left && (txwpx + txhpx >= 24)) {
                    filter_intra_edge_corner_high(above_row, left_col);
                }
                if (need_above && n_top_px > 0) {
                    const int32_t strength = svt_aom_intra_edge_filter_strength(txwpx, txhpx, p_angle - 90, filt_type);
                    const int32_t n_px     = n_top_px + ab_le + (need_right ? txhpx : 0);
                    svt_av1_filter_intra_edge_high(above_row - ab_le, n_px, strength);
                }
                if (need_left && n_left_px > 0) {
                    const int32_t strength = svt_aom_intra_edge_filter_strength(txhpx, txwpx, p_angle - 180, filt_type);
                    const int32_t n_px     = n_left_px + ab_le + (need_bottom ? txwpx : 0);

                    svt_av1_filter_intra_edge_high(left_col - ab_le, n_px, strength);
                }
            }
            upsample_above = svt_aom_use_intra_edge_upsample(txwpx, txhpx, p_angle - 90, filt_type);
            if (need_above && upsample_above) {
                const int32_t n_px = txwpx + (need_right ? txhpx : 0);
                //av1_upsample_intra_edge_high(above_row, n_px, bd);// AMIR : to be replaced by optimized code
                svt_av1_upsample_intra_edge_high_c(above_row, n_px, bd);
            }
            upsample_left = svt_aom_use_intra_edge_upsample(txhpx, txwpx, p_angle - 180, filt_type);
            if (need_left && upsample_left) {
                const int32_t n_px = txhpx + (need_bottom ? txwpx : 0);
                //av1_upsample_intra_edge_high(left_col, n_px, bd);// AMIR: to be replaced by optimized code
                svt_av1_upsample_intra_edge_high_c(left_col, n_px, bd);
            }
        }
        svt_aom_highbd_dr_predictor(
            dst, dst_stride, tx_size, above_row, left_col, upsample_above, upsample_left, p_angle, bd);
        return;
    }

    // predict
    if (mode == DC_PRED) {
        svt_aom_dc_pred_high[n_left_px > 0][n_top_px > 0][tx_size](dst, dst_stride, above_row, left_col, bd);
    } else {
        svt_aom_pred_high[mode][tx_size](dst, dst_stride, above_row, left_col, bd);
    }
}
#endif

// bsize is luma bsize.
// tx_size should be the proper size for the plane (chroma size for UV plane, luma size for Y plane).
void svt_av1_predict_intra_block(MacroBlockD* xd, BlockSize bsize, TxSize tx_size, PredictionMode mode,
                                 int32_t angle_delta, int32_t use_palette, PaletteInfo* palette_info,
                                 FilterIntraMode filter_intra_mode, uint8_t* top_neigh_array, uint8_t* left_neigh_array,
                                 EbPictureBufferDesc* recon_buffer, int32_t col_off, int32_t row_off, int32_t plane,
                                 Part shape, uint32_t dst_offset_x, uint32_t dst_offset_y, SeqHeader* seq_header_ptr,
                                 EbBitDepth bit_depth) {
    const int       ss_x        = plane ? 1 : 0;
    const int       ss_y        = plane ? 1 : 0;
    const BlockSize plane_bsize = get_plane_block_size(bsize, ss_x, ss_y);
    const int       wpx         = block_size_wide[plane_bsize];
    const int       hpx         = block_size_high[plane_bsize];

    const int is_16bit = (bit_depth == EB_EIGHT_BIT) ? 0 : 1;

    uint8_t* dst;
    int32_t  dst_stride;
    if (plane == 0) {
        dst = recon_buffer->buffer_y +
            ((dst_offset_x + recon_buffer->org_x + (dst_offset_y + recon_buffer->org_y) * recon_buffer->stride_y)
             << is_16bit);
        dst_stride = recon_buffer->stride_y;
    } else if (plane == 1) {
        dst = recon_buffer->buffer_cb +
            (((dst_offset_x + recon_buffer->org_x / 2 +
               (dst_offset_y + recon_buffer->org_y / 2) * recon_buffer->stride_cb))
             << is_16bit);
        dst_stride = recon_buffer->stride_cb;
    } else {
        dst = recon_buffer->buffer_cr +
            (((dst_offset_x + recon_buffer->org_x / 2 +
               (dst_offset_y + recon_buffer->org_y / 2) * recon_buffer->stride_cr))
             << is_16bit);
        dst_stride = recon_buffer->stride_cr;
    }

    const int32_t txwpx = tx_size_wide[tx_size];
    const int32_t txhpx = tx_size_high[tx_size];
    const int32_t x     = col_off << MI_SIZE_LOG2;
    const int32_t y     = row_off << MI_SIZE_LOG2;
    if (use_palette) {
        const uint8_t* const  map     = palette_info->color_idx_map;
        const uint16_t* const palette = palette_info->pmi.palette_colors + plane * PALETTE_MAX_SIZE;
        if (is_16bit) {
            uint16_t* dst16 = (uint16_t*)dst;
            for (int r = 0; r < txhpx; ++r) {
                for (int c = 0; c < txwpx; ++c) {
                    dst16[r * dst_stride + c] = palette[map[(r + y) * wpx + c + x]];
                }
            }
        } else {
            for (int r = 0; r < txhpx; ++r) {
                for (int c = 0; c < txwpx; ++c) {
                    dst[r * dst_stride + c] = (uint8_t)palette[map[(r + y) * wpx + c + x]];
                }
            }
        }
        return;
    }
    const int32_t txw           = eb_tx_size_wide_unit[tx_size];
    const int32_t txh           = eb_tx_size_high_unit[tx_size];
    const int32_t have_top      = row_off || (ss_y ? xd->chroma_up_available : xd->up_available);
    const int32_t have_left     = col_off || (ss_x ? xd->chroma_left_available : xd->left_available);
    const int32_t mi_row        = -xd->mb_to_top_edge >> (3 + MI_SIZE_LOG2);
    const int32_t mi_col        = -xd->mb_to_left_edge >> (3 + MI_SIZE_LOG2);
    const int32_t xr_chr_offset = 0;
    const int32_t yd_chr_offset = 0;

    // Distance between the right edge of this prediction block to
    // the frame right edge
    const int32_t xr = (xd->mb_to_right_edge >> (3 + ss_x)) + (wpx - x - txwpx) - xr_chr_offset;
    // Distance between the bottom edge of this prediction block to
    // the frame bottom edge
    const int32_t yd               = (xd->mb_to_bottom_edge >> (3 + ss_y)) + (hpx - y - txhpx) - yd_chr_offset;
    const int32_t right_available  = mi_col + ((col_off + txw) << ss_x) < xd->tile.mi_col_end;
    const int32_t bottom_available = (yd > 0) && (mi_row + ((row_off + txh) << ss_y) < xd->tile.mi_row_end);

    const PartitionType partition = from_shape_to_part[shape];

    // force 4x4 chroma component block size.
    bsize = svt_aom_scale_chroma_bsize(bsize, ss_x, ss_y);

    const int32_t have_top_right   = svt_aom_intra_has_top_right(seq_header_ptr->sb_size,
                                                               bsize,
                                                               mi_row,
                                                               mi_col,
                                                               have_top,
                                                               right_available,
                                                               partition,
                                                               tx_size,
                                                               row_off,
                                                               col_off,
                                                               ss_x,
                                                               ss_y);
    const int32_t have_bottom_left = svt_aom_intra_has_bottom_left(seq_header_ptr->sb_size,
                                                                   bsize,
                                                                   mi_row,
                                                                   mi_col,
                                                                   bottom_available,
                                                                   have_left,
                                                                   partition,
                                                                   tx_size,
                                                                   row_off,
                                                                   col_off,
                                                                   ss_x,
                                                                   ss_y);

    const int32_t disable_edge_filter = !(seq_header_ptr->enable_intra_edge_filter);

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    if (is_16bit) {
        build_intra_predictors_high(xd,
                                    (uint16_t*)top_neigh_array,
                                    (uint16_t*)left_neigh_array,
                                    (uint16_t*)dst,
                                    dst_stride,
                                    mode,
                                    angle_delta,
                                    filter_intra_mode,
                                    tx_size,
                                    disable_edge_filter,
                                    have_top ? AOMMIN(txwpx, xr + txwpx) : 0,
                                    have_top_right ? AOMMIN(txwpx, xr) : 0,
                                    have_left ? AOMMIN(txhpx, yd + txhpx) : 0,
                                    have_bottom_left ? AOMMIN(txhpx, yd) : 0,
                                    plane,
                                    bit_depth);
        return;
    }
#endif
    build_intra_predictors(xd,
                           top_neigh_array,
                           left_neigh_array,
                           dst,
                           dst_stride,
                           mode,
                           angle_delta,
                           filter_intra_mode,
                           tx_size,
                           disable_edge_filter,
                           have_top ? AOMMIN(txwpx, xr + txwpx) : 0,
                           have_top_right ? AOMMIN(txwpx, xr) : 0,
                           have_left ? AOMMIN(txhpx, yd + txhpx) : 0,
                           have_bottom_left ? AOMMIN(txhpx, yd) : 0,
                           plane);
}

/** IntraPrediction()
is the main function to compute intra prediction for a PU
*/
EbErrorType svt_av1_intra_prediction(uint8_t hbd_md, ModeDecisionContext* ctx, PictureControlSet* pcs,
                                     ModeDecisionCandidateBuffer* cand_bf) {
    EbErrorType    return_error   = EB_ErrorNone;
    const TxSize   tx_size        = tx_depth_to_tx_size[cand_bf->cand->block_mi.tx_depth][ctx->blk_geom->bsize];
    const TxSize   tx_size_chroma = av1_get_max_uv_txsize(ctx->blk_geom->bsize, 1, 1);
    const uint32_t sb_size_luma   = pcs->ppcs->scs->sb_size;
    const uint32_t sb_size_chroma = pcs->ppcs->scs->sb_size / 2;
    const bool     is_16bit       = !!hbd_md;

    uint8_t        top_neigh_array[(64 * 2 + 1) << 1];
    uint8_t        left_neigh_array[(64 * 2 + 1) << 1];
    PredictionMode mode;
    // Hsan: plane should be derived @ an earlier stage (e.g. @ the call of perform_fast_loop())
    int32_t start_plane = (ctx->uv_intra_comp_only) ? 1 : 0;
    int32_t end_plane   = ctx->mds_do_chroma ? MAX_MB_PLANE : 1;
    for (int32_t plane = start_plane; plane < end_plane; ++plane) {
        if (plane) {
            mode = (cand_bf->cand->block_mi.uv_mode == UV_CFL_PRED) ? (PredictionMode)UV_DC_PRED
                                                                    : (PredictionMode)cand_bf->cand->block_mi.uv_mode;
        } else {
            mode = cand_bf->cand->block_mi.mode;
        }
        assert(mode < INTRA_MODES);
        int                ang         = plane ? cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_UV]
                                               : cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_Y];
        const IntraSize    intra_size  = ang == 0 ? svt_aom_intra_unit[mode] : (IntraSize){2, 2};
        const int          bwidth      = plane ? ctx->blk_geom->bwidth_uv : ctx->blk_geom->bwidth;
        const int          bheight     = plane ? ctx->blk_geom->bheight_uv : ctx->blk_geom->bheight;
        const int          blk_org_x   = plane ? ctx->round_origin_x >> 1 : ctx->blk_org_x;
        const int          blk_org_y   = plane ? ctx->round_origin_y >> 1 : ctx->blk_org_y;
        const int          sb_size     = plane ? sb_size_chroma : sb_size_luma;
        NeighborArrayUnit* recon_neigh = plane == 0 ? (is_16bit ? ctx->luma_recon_na_16bit : ctx->recon_neigh_y)
            : plane == 1                            ? (is_16bit ? ctx->cb_recon_na_16bit : ctx->recon_neigh_cb)
                                                    : (is_16bit ? ctx->cr_recon_na_16bit : ctx->recon_neigh_cr);

        // Copy neighbour arrays
        if (blk_org_y != 0) {
            svt_memcpy(top_neigh_array + ((uint64_t)1 << is_16bit),
                       recon_neigh->top_array + (blk_org_x << is_16bit),
                       (bwidth * intra_size.top) << is_16bit);
        }

        if (blk_org_x != 0) {
            const uint16_t multipler = (blk_org_y % sb_size + bheight * intra_size.left) > sb_size ? 1
                                                                                                   : intra_size.left;
            svt_memcpy(left_neigh_array + ((uint64_t)1 << is_16bit),
                       recon_neigh->left_array + (blk_org_y << is_16bit),
                       (bheight * multipler) << is_16bit);
        }

        if (blk_org_y != 0 && blk_org_x != 0) {
            if (is_16bit) {
                uint16_t* top_hbd  = (uint16_t*)top_neigh_array;
                uint16_t* left_hbd = (uint16_t*)left_neigh_array;
                top_hbd[0] = left_hbd[0] = ((uint16_t*)(recon_neigh->top_left_array) + recon_neigh->max_pic_h +
                                            blk_org_x - blk_org_y)[0];

            } else {
                top_neigh_array[0] = left_neigh_array[0] =
                    recon_neigh->top_left_array[recon_neigh->max_pic_h + blk_org_x - blk_org_y];
            }
        }

        svt_av1_predict_intra_block(
            ctx->blk_ptr->av1xd,
            ctx->blk_geom->bsize,
            plane ? tx_size_chroma : tx_size,
            mode,
            plane ? cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_UV]
                  : cand_bf->cand->block_mi.angle_delta[PLANE_TYPE_Y],
            plane == 0 ? (cand_bf->cand->palette_info ? cand_bf->cand->palette_size[0] > 0 : 0) : 0,
            plane == 0 ? cand_bf->cand->palette_info : NULL,
            plane ? FILTER_INTRA_MODES : cand_bf->cand->block_mi.filter_intra_mode,
            top_neigh_array + ((uint64_t)1 << is_16bit),
            left_neigh_array + ((uint64_t)1 << is_16bit),
            cand_bf->pred,
            0,
            0,
            plane,
            ctx->shape,
            0,
            0,
            &pcs->scs->seq_header,
            hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT);
    }

    return return_error;
}

static void intra_luma_prediction_for_interintra(ModeDecisionContext* ctx, PictureControlSet* pcs,
                                                 InterIntraMode interintra_mode, EbPictureBufferDesc* prediction_ptr) {
    const uint8_t        is_inter     = 0; // set to 0 b/c this is an intra path
    const TxSize         tx_size      = tx_depth_to_tx_size[0][ctx->blk_geom->bsize];
    const PredictionMode mode         = interintra_to_intra_mode[interintra_mode];
    const uint32_t       sb_size_luma = pcs->ppcs->scs->sb_size;

    const bool is_16bit = !!ctx->hbd_md;
    // No angular modes for interintra
    const IntraSize intra_size = svt_aom_intra_unit[mode];
    uint8_t         top_neigh_array[(64 * 2 + 1) << 1];
    uint8_t         left_neigh_array[(64 * 2 + 1) << 1];

    NeighborArrayUnit* recon_neigh = is_16bit ? ctx->luma_recon_na_16bit : ctx->recon_neigh_y;
    if (ctx->blk_org_y != 0) {
        svt_memcpy(top_neigh_array + ((uint64_t)1 << is_16bit),
                   recon_neigh->top_array + (ctx->blk_org_x << is_16bit),
                   (ctx->blk_geom->bwidth * intra_size.top) << is_16bit);
    }

    if (ctx->blk_org_x != 0) {
        uint16_t multipler = (ctx->blk_org_y % sb_size_luma + ctx->blk_geom->bheight * intra_size.left) > sb_size_luma
            ? 1
            : intra_size.left;
        svt_memcpy(left_neigh_array + ((uint64_t)1 << is_16bit),
                   recon_neigh->left_array + (ctx->blk_org_y << is_16bit),
                   (ctx->blk_geom->bheight * multipler) << is_16bit);
    }

    if (ctx->blk_org_y != 0 && ctx->blk_org_x != 0) {
        if (is_16bit) {
            uint16_t* top_hbd  = (uint16_t*)top_neigh_array;
            uint16_t* left_hbd = (uint16_t*)left_neigh_array;
            top_hbd[0] = left_hbd[0] = ((uint16_t*)(recon_neigh->top_left_array) + recon_neigh->max_pic_h +
                                        ctx->blk_org_x - ctx->blk_org_y)[0];
        } else {
            top_neigh_array[0] = left_neigh_array[0] =
                recon_neigh->top_left_array[recon_neigh->max_pic_h + ctx->blk_org_x - ctx->blk_org_y];
        }
    }
    svt_av1_predict_intra_block(ctx->blk_ptr->av1xd,
                                ctx->blk_geom->bsize,
                                tx_size,
                                mode,
                                0,
                                0,
                                NULL,
                                FILTER_INTRA_MODES,
                                top_neigh_array + ((uint64_t)1 << is_16bit),
                                left_neigh_array + ((uint64_t)1 << is_16bit),
                                prediction_ptr,
                                (tx_org[ctx->blk_geom->bsize][is_inter][0][0].x) >> 2,
                                (tx_org[ctx->blk_geom->bsize][is_inter][0][0].y) >> 2,
                                AOM_PLANE_Y,
                                ctx->shape,
                                0,
                                0,
                                &pcs->scs->seq_header,
                                ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT);
}

// For every block, perform DC/V/H/S intra prediction to be used later in inter-intra search
void svt_aom_precompute_intra_pred_for_inter_intra(PictureControlSet* pcs, ModeDecisionContext* ctx) {
    uint32_t            j;
    EbPictureBufferDesc pred_desc;
    pred_desc.org_x = pred_desc.org_y = 0;
    pred_desc.stride_y                = ctx->blk_geom->bwidth;

    for (j = 0; j < INTERINTRA_MODES; ++j) {
        InterIntraMode interintra_mode = (InterIntraMode)j;
        pred_desc.buffer_y             = ctx->intrapred_buf[j];
        intra_luma_prediction_for_interintra(ctx, pcs, interintra_mode, &pred_desc);
    }
}

EbErrorType svt_aom_update_neighbor_samples_array_open_loop_mb(uint8_t use_top_righ_bottom_left,
                                                               uint8_t update_top_neighbor, uint8_t* above_ref,
                                                               uint8_t* left_ref, EbPictureBufferDesc* input_ptr,
                                                               uint32_t stride, uint32_t src_origin_x,
                                                               uint32_t src_origin_y, uint8_t bwidth, uint8_t bheight) {
    EbErrorType return_error = EB_ErrorNone;

    uint32_t idx;
    uint8_t* src_ptr;
    uint8_t* read_ptr;
    uint32_t count;

    uint32_t width              = input_ptr->width;
    uint32_t height             = input_ptr->height;
    uint32_t block_width_neigh  = use_top_righ_bottom_left ? bwidth << 1 : bwidth;
    uint32_t block_height_neigh = use_top_righ_bottom_left ? bheight << 1 : bheight;
    // Adjust the Source ptr to start at the origin of the block being updated
    src_ptr = input_ptr->buffer_y + (((src_origin_y + input_ptr->org_y) * stride) + (src_origin_x + input_ptr->org_x));

    //Initialise the Luma Intra Reference Array to the mid range value 128 (for CUs at the picture boundaries)
    EB_MEMSET(above_ref, 127, block_width_neigh + 1);
    EB_MEMSET(left_ref, 129, block_height_neigh + 1);

    // Get the upper left sample
    if (src_origin_x != 0 && src_origin_y != 0) {
        read_ptr   = src_ptr - stride - 1;
        *above_ref = *read_ptr;
        *left_ref  = *read_ptr;
        left_ref++;
        above_ref++;
    } else {
        *above_ref = *left_ref = 128;
        left_ref++;
        above_ref++;
    }
    // Get the left-column
    count = block_width_neigh;
    if (src_origin_x != 0) {
        read_ptr = src_ptr - 1;
        if (src_origin_y == 0) {
            *(left_ref - 1) = *read_ptr;
        }
        count = ((src_origin_y + count) > height) ? count - ((src_origin_y + count) - height) : count;
        for (idx = 0; idx < count; ++idx) {
            *left_ref = *read_ptr;
            read_ptr += stride;
            left_ref++;
        }
        left_ref += (block_width_neigh - count);
        if (use_top_righ_bottom_left) {
            // pading unknown left bottom pixels with value at(-1, -15)
            for (idx = 0; idx < bheight; idx++) {
                *(left_ref - bheight + idx) = *(left_ref - bheight - 1);
            }
        }
    } else if (src_origin_y != 0) {
        count = ((src_origin_y + count) > height) ? count - ((src_origin_y + count) - height) : count;
        EB_MEMSET(left_ref - 1, *(src_ptr - stride), count + 1);
        *(above_ref - 1) = *(src_ptr - stride);
    } else {
        left_ref += count;
    }

    // Get the top-row
    count = block_width_neigh;
    if (src_origin_y != 0) {
        if (update_top_neighbor) {
            read_ptr = src_ptr - stride;
            count    = ((src_origin_x + count) > width) ? count - ((src_origin_x + count) - width) : count;
            svt_memcpy(above_ref, read_ptr, count);
        }
        // pading unknown top right pixels with value at(15, -1)
        if (use_top_righ_bottom_left) {
            if (src_origin_x != 0) {
                for (idx = 0; idx < bwidth; idx++) {
                    *(above_ref + bwidth + idx) = *(above_ref + bwidth - 1);
                }
            }
        }
    } else if (src_origin_x != 0) {
        count = ((src_origin_x + count) > width) ? count - ((src_origin_x + count) - width) : count;
        EB_MEMSET(above_ref - 1, *(left_ref - count), count + 1);
    }

    return return_error;
}

EbErrorType svt_aom_update_neighbor_samples_array_open_loop_mb_recon(uint8_t use_top_righ_bottom_left,
                                                                     uint8_t update_top_neighbor, uint8_t* above_ref,
                                                                     uint8_t* left_ref, uint8_t* recon_ptr,
                                                                     uint32_t stride, uint32_t src_origin_x,
                                                                     uint32_t src_origin_y, uint8_t bwidth,
                                                                     uint8_t bheight, uint32_t width, uint32_t height) {
    EbErrorType return_error = EB_ErrorNone;

    uint32_t idx;
    uint8_t* src_ptr;
    uint8_t* read_ptr;
    uint32_t count;
    uint32_t block_width_neigh  = use_top_righ_bottom_left ? bwidth << 1 : bwidth;
    uint32_t block_height_neigh = use_top_righ_bottom_left ? bheight << 1 : bheight;
    // Adjust the Source ptr to start at the origin of the block being updated
    src_ptr = recon_ptr + (src_origin_y * stride + src_origin_x);

    //Initialise the Luma Intra Reference Array to the mid range value 128 (for CUs at the picture boundaries)
    EB_MEMSET(above_ref, 127, block_width_neigh + 1);
    EB_MEMSET(left_ref, 129, block_height_neigh + 1);
    // Get the upper left sample
    if (src_origin_x != 0 && src_origin_y != 0) {
        read_ptr   = src_ptr - stride - 1;
        *above_ref = *read_ptr;
        *left_ref  = *read_ptr;
        left_ref++;
        above_ref++;
    } else {
        *above_ref = *left_ref = 128;
        left_ref++;
        above_ref++;
    }
    // Get the left-column
    count = block_width_neigh;
    if (src_origin_x != 0) {
        read_ptr = src_ptr - 1;
        if (src_origin_y == 0) {
            *(left_ref - 1) = *read_ptr;
        }
        count = ((src_origin_y + count) > height) ? count - ((src_origin_y + count) - height) : count;
        for (idx = 0; idx < count; ++idx) {
            *left_ref = *read_ptr;
            read_ptr += stride;
            left_ref++;
        }
        left_ref += (block_width_neigh - count);
        // pading unknown left bottom pixels with value at(-1, -15)
        if (use_top_righ_bottom_left) {
            for (idx = 0; idx < bheight; idx++) {
                *(left_ref - bheight + idx) = *(left_ref - bheight - 1);
            }
        }
    } else if (src_origin_y != 0) {
        count = ((src_origin_y + count) > height) ? count - ((src_origin_y + count) - height) : count;
        EB_MEMSET(left_ref - 1, *(src_ptr - stride), count + 1);
        *(above_ref - 1) = *(src_ptr - stride);
    } else {
        left_ref += count;
    }

    // Get the top-row
    count = block_width_neigh;
    if (src_origin_y != 0) {
        if (update_top_neighbor) {
            read_ptr = src_ptr - stride;
            count    = ((src_origin_x + count) > width) ? count - ((src_origin_x + count) - width) : count;
            svt_memcpy(above_ref, read_ptr, count);
        }
        // pading unknown top right pixels with value at(15, -1)
        if (use_top_righ_bottom_left) {
            if (src_origin_x != 0) {
                for (idx = 0; idx < bwidth; idx++) {
                    *(above_ref + bwidth + idx) = *(above_ref + bwidth - 1);
                }
            }
        }
    } else if (src_origin_x != 0) {
        count = ((src_origin_x + count) > width) ? count - ((src_origin_x + count) - width) : count;
        EB_MEMSET(above_ref - 1, *(left_ref - count), count + 1);
    }

    return return_error;
}
