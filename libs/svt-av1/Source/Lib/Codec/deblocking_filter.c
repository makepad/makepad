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

#include <string.h>

#include "deblocking_filter.h"
#include "definitions.h"
#include "utility.h"
#include "pcs.h"
#include "coding_unit.h"
#include "sequence_control_set.h"
#include "reference_object.h"
#include "common_utils.h"
#include "ac_bias.h"
#include "inv_transforms.h"

#define DLF_MAX_LVL 4
static const int32_t  inter_frame_multiplier[INPUT_SIZE_COUNT]      = {6017, 6017, 6017, 12034, 12034, 12034, 12034};
static const uint32_t disable_dlf_th[DLF_MAX_LVL][INPUT_SIZE_COUNT] = {{0, 0, 0, 0, 0, 0, 0},
                                                                       {100, 200, 500, 800, 1000, 1000, 1000},
                                                                       {900, 1000, 2000, 3000, 4000, 4000, 4000},
                                                                       {6000, 7000, 8000, 9000, 10000, 10000, 10000}};

static const TxSize txsize_horz_map[TX_SIZES_ALL] = {
    TX_4X4, // TX_4X4
    TX_8X8, // TX_8X8
    TX_16X16, // TX_16X16
    TX_32X32, // TX_32X32
    TX_64X64, // TX_64X64
    TX_4X4, // TX_4X8
    TX_8X8, // TX_8X4
    TX_8X8, // TX_8X16
    TX_16X16, // TX_16X8
    TX_16X16, // TX_16X32
    TX_32X32, // TX_32X16
    TX_32X32, // TX_32X64
    TX_64X64, // TX_64X32
    TX_4X4, // TX_4X16
    TX_16X16, // TX_16X4
    TX_8X8, // TX_8X32
    TX_32X32, // TX_32X8
    TX_16X16, // TX_16X64
    TX_64X64, // TX_64X16
};

static const TxSize txsize_vert_map[TX_SIZES_ALL] = {
    TX_4X4, // TX_4X4
    TX_8X8, // TX_8X8
    TX_16X16, // TX_16X16
    TX_32X32, // TX_32X32
    TX_64X64, // TX_64X64
    TX_8X8, // TX_4X8
    TX_4X4, // TX_8X4
    TX_16X16, // TX_8X16
    TX_8X8, // TX_16X8
    TX_32X32, // TX_16X32
    TX_16X16, // TX_32X16
    TX_64X64, // TX_32X64
    TX_32X32, // TX_64X32
    TX_16X16, // TX_4X16
    TX_4X4, // TX_16X4
    TX_32X32, // TX_8X32
    TX_8X8, // TX_32X8
    TX_64X64, // TX_16X64
    TX_16X16, // TX_64X16
};

/*************************************************************************************************
 * svt_av1_loop_filter_init
 * Initialize the loop filter limits and thresholds
 *************************************************************************************************/
void svt_av1_loop_filter_init(PictureControlSet* pcs) {
    //assert(MB_MODE_COUNT == n_elements(mode_lf_lut));
    LoopFilterInfoN* lfi = &pcs->ppcs->lf_info;
    LoopFilter*      lf  = &pcs->ppcs->frm_hdr.loop_filter_params;
    int32_t          lvl;

    lf->combine_vert_horz_lf = 1;

    // init limits for given sharpness
    svt_aom_update_sharpness(lfi, lf->sharpness_level);

    // init hev threshold const vectors
    for (lvl = 0; lvl <= MAX_LOOP_FILTER; lvl++) {
        memset(lfi->lfthr[lvl].hev_thr, (lvl >> 4), SIMD_WIDTH);
    }
}

//***************************************************************************************************//

static INLINE int32_t scaled_buffer_offset(int32_t x_offset, int32_t y_offset, int32_t stride/*,
    const struct scale_factors *sf*/) {
    const int32_t x =
        /*sf ? sf->scale_value_x(x_offset, sf) >> SCALE_EXTRA_BITS :*/ x_offset;
    const int32_t y =
        /*sf ? sf->scale_value_y(y_offset, sf) >> SCALE_EXTRA_BITS :*/ y_offset;
    return y * stride + x;
}

static INLINE void setup_pred_plane(Buf2D* dst, BlockSize bsize, uint8_t* src, int32_t width, int32_t height,
                                    int32_t stride, int32_t mi_row, int32_t mi_col,
                                    /*const struct scale_factors *scale,*/
                                    int32_t subsampling_x, int32_t subsampling_y, int32_t is_16bit) {
    // Offset the buffer pointer
    if (subsampling_y && (mi_row & 0x01) && (mi_size_high[bsize] == 1)) {
        mi_row -= 1;
    }
    if (subsampling_x && (mi_col & 0x01) && (mi_size_wide[bsize] == 1)) {
        mi_col -= 1;
    }

    const int32_t x = (MI_SIZE * mi_col) >> subsampling_x;
    const int32_t y = (MI_SIZE * mi_row) >> subsampling_y;
    dst->buf        = src + (scaled_buffer_offset(x, y, stride /*, scale*/) << is_16bit);
    dst->buf0       = src;
    dst->width      = width;
    dst->height     = height;
    dst->stride     = stride;
}

void svt_av1_setup_dst_planes(PictureControlSet* pcs, MacroblockdPlane* planes, BlockSize bsize,
                              //const Yv12BufferConfig *src,
                              const EbPictureBufferDesc* src, int32_t mi_row, int32_t mi_col, const int32_t plane_start,
                              const int32_t plane_end) {
    // We use AOMMIN(num_planes, MAX_MB_PLANE) instead of num_planes to quiet
    // the static analysis warnings.
    //for (int32_t i = plane_start; i < AOMMIN(plane_end, MAX_MB_PLANE); ++i) {
    //    MacroblockdPlane *const pd = &planes[i];
    //    const int32_t is_uv = i > 0;
    //    setup_pred_plane(&pd->dst, bsize, src->buffers[i], src->crop_widths[is_uv],
    //        src->crop_heights[is_uv], src->strides[is_uv], mi_row,
    //        mi_col, NULL, pd->subsampling_x, pd->subsampling_y);
    //}
    SequenceControlSet* scs = pcs->scs;
    for (int32_t i = plane_start; i < AOMMIN(plane_end, 3); ++i) {
        if (i == 0) {
            MacroblockdPlane* const pd = &planes[0];
            setup_pred_plane(
                &pd->dst,
                bsize,
                &src->buffer_y[(src->org_x + src->org_y * src->stride_y) << pd->is_16bit],
                (scs->max_input_luma_width -
                 scs->max_input_pad_right), // The width/height should be the unpadded width/height (see AV1 spec 7.14.2 Edge Loop Filter Process)
                (scs->max_input_luma_height - scs->max_input_pad_bottom),
                src->stride_y,
                mi_row,
                mi_col,
                /*NULL,*/ pd->subsampling_x,
                pd->subsampling_y,
                pd->is_16bit); //AMIR: Updated to point to the right location
        } else if (i == 1) {
            MacroblockdPlane* const pd = &planes[1];
            setup_pred_plane(
                &pd->dst,
                bsize,
                &src->buffer_cb[((src->org_x + src->org_y * src->stride_cb) << pd->is_16bit) / 2],
                (scs->max_input_luma_width - scs->max_input_pad_right) >>
                    1, // The width/height should be the unpadded width/height (see AV1 spec 7.14.2 Edge Loop Filter Process)
                (scs->max_input_luma_height - scs->max_input_pad_bottom) >> 1,
                src->stride_cb,
                mi_row,
                mi_col,
                /*NULL,*/ pd->subsampling_x,
                pd->subsampling_y,
                pd->is_16bit);
        } else if (i == 2) {
            MacroblockdPlane* const pd = &planes[2];
            setup_pred_plane(
                &pd->dst,
                bsize,
                &src->buffer_cr[((src->org_x + src->org_y * src->stride_cr) << pd->is_16bit) / 2],
                (scs->max_input_luma_width - scs->max_input_pad_right) >>
                    1, // The width/height should be the unpadded width/height (see AV1 spec 7.14.2 Edge Loop Filter Process)
                (scs->max_input_luma_height - scs->max_input_pad_bottom) >> 1,
                src->stride_cr,
                mi_row,
                mi_col,
                /* NULL,*/ pd->subsampling_x,
                pd->subsampling_y,
                pd->is_16bit);
        }
    }
}

//***************************************************************************************************//
static INLINE TxSize get_transform_size(const MbModeInfo* const mbmi, const EdgeDir edge_dir, const int32_t plane,
                                        const MacroblockdPlane* plane_ptr, const bool is_skip) {
    assert(mbmi != NULL);

    TxSize tx_size = (plane == COMPONENT_LUMA)
        ? (is_skip ? tx_depth_to_tx_size[0][mbmi->bsize]
                   : tx_depth_to_tx_size[mbmi->block_mi.tx_depth][mbmi->bsize]) // use max_tx_size
        : av1_get_max_uv_txsize(mbmi->bsize, plane_ptr->subsampling_x, plane_ptr->subsampling_y);
    assert(tx_size < TX_SIZES_ALL);

    // since in case of chrominance or non-square transorm need to convert
    // transform size into transform size in particular direction.
    // for vertical edge, filter direction is horizontal, for horizontal
    // edge, filter direction is vertical.
    return (VERT_EDGE == edge_dir) ? txsize_horz_map[tx_size] : txsize_vert_map[tx_size];
}

// Return TxSize from get_transform_size(), so it is plane and direction
// awared
static TxSize set_lpf_parameters(Av1DeblockingParameters* const params, const uint64_t mode_step,
                                 const PictureControlSet* const pcs, const EdgeDir edge_dir, const uint32_t x,
                                 const uint32_t y, const int32_t plane, const MacroblockdPlane* const plane_ptr) {
    FrameHeader*           frm_hdr = &pcs->ppcs->frm_hdr;
    const LoopFilterInfoN* lfi_n   = &pcs->ppcs->lf_info;

    // reset to initial values
    params->filter_length = 0;

    // no deblocking is required
    const uint32_t width  = plane_ptr->dst.width;
    const uint32_t height = plane_ptr->dst.height;
    if ((width <= x) || (height <= y)) {
        // just return the smallest transform unit size
        return TX_4X4;
    }

    const uint32_t scale_horz = plane_ptr->subsampling_x;
    const uint32_t scale_vert = plane_ptr->subsampling_y;
    // for sub8x8 block, chroma prediction mode is obtained from the bottom/right
    // mi structure of the co-located 8x8 luma block. so for chroma plane, mi_row
    // and mi_col should map to the bottom/right mi structure, i.e, both mi_row
    // and mi_col should be odd number for chroma plane.

    const int32_t     mi_row    = scale_vert | ((y << scale_vert) >> MI_SIZE_LOG2);
    const int32_t     mi_col    = scale_horz | ((x << scale_horz) >> MI_SIZE_LOG2);
    uint32_t          mi_stride = pcs->mi_stride;
    const int32_t     offset    = mi_row * mi_stride + mi_col;
    MbModeInfo**      mi        = pcs->mi_grid_base + offset;
    const MbModeInfo* mbmi      = mi[0];

    // If current mbmi is not correctly setup, return an invalid value to stop
    // filtering. One example is that if this tile is not coded, then its mbmi
    // it not set up.
    if (mbmi == NULL) {
        return TX_INVALID;
    }
    const uint8_t segment_id   = mbmi->segment_id;
    const int32_t curr_skipped = mbmi->block_mi.skip && is_inter_block_no_intrabc(mbmi->block_mi.ref_frame[0]);
    const TxSize  ts           = get_transform_size(mbmi, edge_dir, plane, plane_ptr, curr_skipped);
    assert(ts < TX_SIZES_ALL);

    {
        const uint32_t coord           = (VERT_EDGE == edge_dir) ? (x) : (y);
        const uint32_t transform_masks = edge_dir == VERT_EDGE ? tx_size_wide[ts] - 1 : tx_size_high[ts] - 1;
        const int32_t  txb_edge        = (coord & transform_masks) ? (0) : (1);

        if (!txb_edge) {
            return ts;
        }

        // prepare outer edge parameters. deblock the edge if it's an edge of a TU
        {
            uint32_t       curr_level; // Added to address 4x4 problem
            PredictionMode mode = mbmi->block_mi.mode;
            if (frm_hdr->delta_lf_params.delta_lf_present) {
                curr_level = svt_aom_get_filter_level_delta_lf(
                    frm_hdr, edge_dir, plane, pcs->ppcs->curr_delta_lf, segment_id, mode, mbmi->block_mi.ref_frame[0]);
            } else {
                assert(mode < 25);
                curr_level = lfi_n->lvl[plane][segment_id][edge_dir][mbmi->block_mi.ref_frame[0]][mode_lf_lut[mode]];
            }

            uint32_t level = curr_level;
            if (coord) {
                const MbModeInfo* const mi_prev = *(mi - mode_step);
                if (mi_prev == NULL) {
                    return TX_INVALID;
                }
                const int32_t pv_skip = mi_prev->block_mi.skip &&
                    is_inter_block_no_intrabc(mi_prev->block_mi.ref_frame[0]);
                const TxSize pv_ts = get_transform_size(mi_prev, edge_dir, plane, plane_ptr, pv_skip);
                uint32_t     pv_lvl;
                mode = mi_prev->block_mi.mode;
                if (frm_hdr->delta_lf_params.delta_lf_present) {
                    pv_lvl = svt_aom_get_filter_level_delta_lf(frm_hdr,
                                                               edge_dir,
                                                               plane,
                                                               pcs->ppcs->curr_delta_lf,
                                                               mi_prev->segment_id,
                                                               mi_prev->block_mi.mode,
                                                               mi_prev->block_mi.ref_frame[0]);
                } else {
                    assert(mode < MB_MODE_COUNT);
                    pv_lvl = lfi_n->lvl[plane][mi_prev->segment_id][edge_dir][mi_prev->block_mi.ref_frame[0]]
                                       [mode_lf_lut[mode]];
                }

                const BlockSize bsize = get_plane_block_size(
                    mbmi->bsize, plane_ptr->subsampling_x, plane_ptr->subsampling_y);
                assert(bsize < BLOCK_SIZES_ALL);
                const int32_t prediction_masks = (edge_dir == VERT_EDGE) ? block_size_wide[bsize] - 1
                                                                         : block_size_high[bsize] - 1;
                const int32_t pu_edge          = !(coord & prediction_masks);
                // if the current and the previous blocks are skipped,
                // deblock the edge if the edge belongs to a PU's edge only.
                if ((curr_level || pv_lvl) && (!pv_skip || !curr_skipped || pu_edge)) {
                    const TxSize min_ts = AOMMIN(ts, pv_ts);
                    if (TX_4X4 >= min_ts) {
                        params->filter_length = 4;
                    } else {
                        params->filter_length = (plane != 0) ? 6 : (TX_8X8 == min_ts) ? 8 : 14;
                    }
                    // update the level if the current block is skipped,
                    // but the previous one is not
                    level = (curr_level) ? (curr_level) : (pv_lvl);
                }
            }
            // prepare common parameters
            if (params->filter_length) {
                const LoopFilterThresh* const limits = pcs->ppcs->lf_info.lfthr + level;
                params->lim                          = limits->lim;
                params->mblim                        = limits->mblim;
                params->hev_thr                      = limits->hev_thr;
            }
        }
    }

    return ts;
}

/*************************************************************************************************
* svt_av1_filter_block_plane_vert
* Filter all the vertical edges in the same superblock
*************************************************************************************************/
void svt_av1_filter_block_plane_vert(const PictureControlSet* const pcs, const int32_t plane,
                                     const MacroblockdPlane* const plane_ptr, const uint32_t mi_row,
                                     const uint32_t mi_col) {
    SequenceControlSet* scs = pcs->scs;
    // TODO
    // when loop_filter_mode = 1, dblk is processed in encdec
    // 16 bit dblk for loop_filter_mode = 1 needs to enabled after 16bit encdec is done
    bool           is_16bit   = scs->is_16bit_pipeline;
    const int32_t  row_step   = MI_SIZE >> MI_SIZE_LOG2;
    const uint32_t scale_horz = plane_ptr->subsampling_x;
    const uint32_t scale_vert = plane_ptr->subsampling_y;
    uint8_t* const dst_ptr    = plane_ptr->dst.buf;
    const int32_t  dst_stride = plane_ptr->dst.stride;
    int32_t        y_range    = scs->seq_header.sb_size == BLOCK_128X128 ? (MAX_MIB_SIZE >> scale_vert)
                                                                         : (SB64_MIB_SIZE >> scale_vert);
    int32_t        x_range    = scs->seq_header.sb_size == BLOCK_128X128 ? (MAX_MIB_SIZE >> scale_horz)
                                                                         : (SB64_MIB_SIZE >> scale_horz);

    if (pcs->ppcs->frame_superres_enabled || pcs->ppcs->frame_resize_enabled) {
        // the boundary of last column should use the actual width for frame might be downscaled in
        // super resolution
        const uint32_t       sb_size = (scs->seq_header.sb_size == BLOCK_128X128) ? 128 : 64;
        EbPictureBufferDesc* pic_ptr = pcs->ppcs->enhanced_pic;
        if (mi_col == (pic_ptr->width / sb_size * sb_size) >> MI_SIZE_LOG2) {
            x_range = (((pic_ptr->width) % sb_size) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            if (plane) {
                x_range = ((((pic_ptr->width) % sb_size + scale_horz) >> scale_horz) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            }
        }
        if (mi_row == (pic_ptr->height / sb_size * sb_size) >> MI_SIZE_LOG2) {
            y_range = (((pic_ptr->height) % sb_size) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            if (plane) {
                y_range = ((((pic_ptr->height) % sb_size + scale_vert) >> scale_vert) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            }
        }
    }
    for (int32_t y = 0; y < y_range; y += row_step) {
        uint8_t* p = dst_ptr + ((y * MI_SIZE * dst_stride) << plane_ptr->is_16bit);
        for (int32_t x = 0; x < x_range;) {
            // inner loop always filter vertical edges in a MI block. If MI size
            // is 8x8, it will filter the vertical edge aligned with a 8x8 block.
            // If 4x4 trasnform is used, it will then filter the internal edge
            //  aligned with a 4x4 block
            const uint32_t          curr_x = ((mi_col * MI_SIZE) >> scale_horz) + x * MI_SIZE;
            const uint32_t          curr_y = ((mi_row * MI_SIZE) >> scale_vert) + y * MI_SIZE;
            uint32_t                advance_units;
            TxSize                  tx_size;
            Av1DeblockingParameters params;
            memset(&params, 0, sizeof(params));

            tx_size = set_lpf_parameters(
                &params, ((uint64_t)1 << scale_horz), pcs, VERT_EDGE, curr_x, curr_y, plane, plane_ptr);
            if (tx_size == TX_INVALID) {
                params.filter_length = 0;
                tx_size              = TX_4X4;
            }

            switch (params.filter_length) {
                // apply 4-tap filtering
            case 4:
                if (is_16bit) {
                    svt_aom_highbd_lpf_vertical_4((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                  dst_stride,
                                                  params.mblim,
                                                  params.lim,
                                                  params.hev_thr,
                                                  scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_vertical_4(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
            case 6: // apply 6-tap filter for chroma plane only
                assert(plane != 0);
                if (is_16bit) {
                    svt_aom_highbd_lpf_vertical_6((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                  dst_stride,
                                                  params.mblim,
                                                  params.lim,
                                                  params.hev_thr,
                                                  scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_vertical_6(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // apply 8-tap filtering
            case 8:
                if (is_16bit) {
                    svt_aom_highbd_lpf_vertical_8((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                  dst_stride,
                                                  params.mblim,
                                                  params.lim,
                                                  params.hev_thr,
                                                  scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_vertical_8(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // apply 14-tap filtering
            case 14:
                if (is_16bit) {
                    svt_aom_highbd_lpf_vertical_14((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                   dst_stride,
                                                   params.mblim,
                                                   params.lim,
                                                   params.hev_thr,
                                                   scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_vertical_14(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // no filtering
            default:
                break;
            }
            // advance the destination pointer
            assert(tx_size < TX_SIZES_ALL);
            advance_units = eb_tx_size_wide_unit[tx_size];
            x += advance_units;
            p += ((advance_units * MI_SIZE) << plane_ptr->is_16bit);
        }
    }
}

/*************************************************************************************************
* svt_av1_filter_block_plane_horz
* Filter all the horizontal edges in the same superblock
*************************************************************************************************/
void svt_av1_filter_block_plane_horz(const PictureControlSet* const pcs, const int32_t plane,
                                     const MacroblockdPlane* const plane_ptr, const uint32_t mi_row,
                                     const uint32_t mi_col) {
    SequenceControlSet* scs = pcs->scs;
    // when loop_filter_mode = 1, dblk is processed in encdec
    // 16 bit dblk for loop_filter_mode = 1 needs to enabled after 16bit encdec is done
    bool           is_16bit   = scs->is_16bit_pipeline;
    const int32_t  col_step   = MI_SIZE >> MI_SIZE_LOG2;
    const uint32_t scale_horz = plane_ptr->subsampling_x;
    const uint32_t scale_vert = plane_ptr->subsampling_y;
    uint8_t* const dst_ptr    = plane_ptr->dst.buf;
    const int32_t  dst_stride = plane_ptr->dst.stride;
    int32_t        y_range    = scs->seq_header.sb_size == BLOCK_128X128 ? (MAX_MIB_SIZE >> scale_vert)
                                                                         : (SB64_MIB_SIZE >> scale_vert);
    int32_t        x_range    = scs->seq_header.sb_size == BLOCK_128X128 ? (MAX_MIB_SIZE >> scale_horz)
                                                                         : (SB64_MIB_SIZE >> scale_horz);

    uint32_t mi_stride = pcs->mi_stride;

    if (pcs->ppcs->frame_superres_enabled || pcs->ppcs->frame_resize_enabled) {
        // the boundary of last column should use the actual width for frames might be downscaled in
        // super resolution
        const uint32_t       sb_size = (scs->seq_header.sb_size == BLOCK_128X128) ? 128 : 64;
        EbPictureBufferDesc* pic_ptr = pcs->ppcs->enhanced_pic;
        if (mi_col == (pic_ptr->width / sb_size * sb_size) >> MI_SIZE_LOG2) {
            x_range = (((pic_ptr->width) % sb_size) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            if (plane) {
                x_range = ((((pic_ptr->width) % sb_size + scale_horz) >> scale_horz) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            }
        }
        if (mi_row == (pic_ptr->height / sb_size * sb_size) >> MI_SIZE_LOG2) {
            y_range = (((pic_ptr->height) % sb_size) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            if (plane) {
                y_range = ((((pic_ptr->height) % sb_size + scale_vert) >> scale_vert) + MI_SIZE - 1) >> MI_SIZE_LOG2;
            }
        }
    }
    for (int32_t x = 0; x < x_range; x += col_step) {
        uint8_t* p = dst_ptr + ((x * MI_SIZE) << plane_ptr->is_16bit);
        for (int32_t y = 0; y < y_range;) {
            // inner loop always filter vertical edges in a MI block. If MI size
            // is 8x8, it will first filter the vertical edge aligned with a 8x8
            // block. If 4x4 trasnform is used, it will then filter the internal
            // edge aligned with a 4x4 block
            const uint32_t          curr_x = ((mi_col * MI_SIZE) >> scale_horz) + x * MI_SIZE;
            const uint32_t          curr_y = ((mi_row * MI_SIZE) >> scale_vert) + y * MI_SIZE;
            uint32_t                advance_units;
            TxSize                  tx_size;
            Av1DeblockingParameters params;
            memset(&params, 0, sizeof(params));

            tx_size = set_lpf_parameters(&params,
                                         //(pcs->ppcs->av1_cm->mi_stride << scale_vert),
                                         (mi_stride << scale_vert),
                                         pcs,
                                         HORZ_EDGE,
                                         curr_x,
                                         curr_y,
                                         plane,
                                         plane_ptr);
            if (tx_size == TX_INVALID) {
                params.filter_length = 0;
                tx_size              = TX_4X4;
            }

            switch (params.filter_length) {
                // apply 4-tap filtering
            case 4:
                if (is_16bit) {
                    svt_aom_highbd_lpf_horizontal_4((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                    dst_stride,
                                                    params.mblim,
                                                    params.lim,
                                                    params.hev_thr,
                                                    scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_horizontal_4(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // apply 6-tap filtering
            case 6:
                assert(plane != 0);
                if (is_16bit) {
                    svt_aom_highbd_lpf_horizontal_6((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                    dst_stride,
                                                    params.mblim,
                                                    params.lim,
                                                    params.hev_thr,
                                                    scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_horizontal_6(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // apply 8-tap filtering
            case 8:
                if (is_16bit) {
                    svt_aom_highbd_lpf_horizontal_8((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                    dst_stride,
                                                    params.mblim,
                                                    params.lim,
                                                    params.hev_thr,
                                                    scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_horizontal_8(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // apply 14-tap filtering
            case 14:
                if (is_16bit) {
                    svt_aom_highbd_lpf_horizontal_14((uint16_t*)(p), //CONVERT_TO_SHORTPTR(p),
                                                     dst_stride,
                                                     params.mblim,
                                                     params.lim,
                                                     params.hev_thr,
                                                     scs->static_config.encoder_bit_depth);
                } else {
                    svt_aom_lpf_horizontal_14(p, dst_stride, params.mblim, params.lim, params.hev_thr);
                }
                break;
                // no filtering
            default:
                break;
            }

            // advance the destination pointer
            assert(tx_size < TX_SIZES_ALL);
            advance_units = eb_tx_size_high_unit[tx_size];
            y += advance_units;
            p += ((advance_units * dst_stride * MI_SIZE) << plane_ptr->is_16bit);
        }
    }
}

/*************************************************************************************************
* svt_aom_loop_filter_sb
* Loop over all superblocks in the picture and filter each superblock
*************************************************************************************************/
void svt_aom_loop_filter_sb(EbPictureBufferDesc* frame_buffer, //reconpicture,
                            //Yv12BufferConfig *frame_buffer,
                            PictureControlSet* pcs, int32_t mi_row, int32_t mi_col, int32_t plane_start,
                            int32_t plane_end, uint8_t last_col) {
    FrameHeader*     frm_hdr = &pcs->ppcs->frm_hdr;
    MacroblockdPlane pd[3];
    int32_t          plane;

    pd[0].subsampling_x = 0;
    pd[0].subsampling_y = 0;
    pd[0].plane_type    = PLANE_TYPE_Y;
    pd[0].is_16bit      = frame_buffer->bit_depth > 8;
    pd[1].subsampling_x = 1;
    pd[1].subsampling_y = 1;
    pd[1].plane_type    = PLANE_TYPE_UV;
    pd[1].is_16bit      = frame_buffer->bit_depth > 8;
    pd[2].subsampling_x = 1;
    pd[2].subsampling_y = 1;
    pd[2].plane_type    = PLANE_TYPE_UV;
    pd[2].is_16bit      = frame_buffer->bit_depth > 8;

    if (pcs->ppcs->scs->is_16bit_pipeline) {
        pd[0].is_16bit = pd[1].is_16bit = pd[2].is_16bit = true;
    }

    for (plane = plane_start; plane < plane_end; plane++) {
        if (plane == 0 && !(frm_hdr->loop_filter_params.filter_level[0]) &&
            !(frm_hdr->loop_filter_params.filter_level[1])) {
            break;
        } else if (plane == 1 && !(frm_hdr->loop_filter_params.filter_level_u)) {
            continue;
        } else if (plane == 2 && !(frm_hdr->loop_filter_params.filter_level_v)) {
            continue;
        }

        if (frm_hdr->loop_filter_params.combine_vert_horz_lf) {
            // filter all vertical and horizontal edges in every super block
            // filter vertical edges
            svt_av1_setup_dst_planes(
                pcs, pd, pcs->ppcs->scs->seq_header.sb_size, frame_buffer, mi_row, mi_col, plane, plane + 1);
            svt_av1_filter_block_plane_vert(pcs, plane, &pd[plane], mi_row, mi_col);
            // filter horizontal edges
            int32_t max_mib_size = pcs->ppcs->scs->seq_header.sb_size == BLOCK_128X128 ? MAX_MIB_SIZE : SB64_MIB_SIZE;

            if (mi_col - max_mib_size >= 0) {
                svt_av1_setup_dst_planes(pcs,
                                         pd,
                                         pcs->ppcs->scs->seq_header.sb_size,
                                         frame_buffer,
                                         mi_row,
                                         mi_col - max_mib_size,
                                         plane,
                                         plane + 1);
                svt_av1_filter_block_plane_horz(pcs, plane, &pd[plane], mi_row, mi_col - max_mib_size);
            }
            // Filter the horizontal edges of the last sb in each row
            if (last_col) {
                svt_av1_setup_dst_planes(
                    pcs, pd, pcs->ppcs->scs->seq_header.sb_size, frame_buffer, mi_row, mi_col, plane, plane + 1);
                svt_av1_filter_block_plane_horz(pcs, plane, &pd[plane], mi_row, mi_col);
            }
        } else {
            // filter all vertical edges in every 64x64 super block
            svt_av1_setup_dst_planes(
                pcs, pd, pcs->ppcs->scs->seq_header.sb_size, frame_buffer, mi_row, mi_col, plane, plane + 1);

            svt_av1_filter_block_plane_vert(pcs, plane, &pd[plane], mi_row, mi_col);

            // filter all horizontal edges in every 64x64 super block
            svt_av1_setup_dst_planes(
                pcs, pd, pcs->ppcs->scs->seq_header.sb_size, frame_buffer, mi_row, mi_col, plane, plane + 1);
            svt_av1_filter_block_plane_horz(pcs, plane, &pd[plane], mi_row, mi_col);
        }
    }
}

/*************************************************************************************************
* svt_av1_loop_filter_frame
* Apply loop filtering to the frame based on the selected loop filter parameters
*************************************************************************************************/
void svt_av1_loop_filter_frame(EbPictureBufferDesc* frame_buffer, PictureControlSet* pcs, int32_t plane_start,
                               int32_t plane_end) {
    SequenceControlSet* scs = pcs->scs;
#if !OPT_DLF
    //SuperBlock                     *sb_ptr;
    //uint16_t                                   sb_index;
    uint8_t  sb_size_log2 = (uint8_t)svt_log2f(scs->sb_size);
    uint32_t x_sb_index;
    uint32_t y_sb_index;
    uint32_t sb_origin_x;
    uint32_t sb_origin_y;
    bool     end_of_row_flag;
#endif
    uint32_t pic_width_in_sb      = (pcs->ppcs->aligned_width + scs->sb_size - 1) / scs->sb_size;
    uint32_t picture_height_in_sb = (pcs->ppcs->aligned_height + scs->sb_size - 1) / scs->sb_size;

    svt_av1_loop_filter_frame_init(&pcs->ppcs->frm_hdr, &pcs->ppcs->lf_info, plane_start, plane_end);
#if OPT_DLF
#if OPT_Q_CDEF
    if ((pcs->ppcs->cdef_search_ctrls.enabled && !pcs->ppcs->cdef_search_ctrls.use_qp_strength &&
         !pcs->ppcs->cdef_search_ctrls.use_reference_cdef_fs) ||
        pcs->ppcs->enable_restoration || pcs->ppcs->is_ref ||
#else
    if (pcs->ppcs->cdef_search_ctrls.enabled || pcs->ppcs->enable_restoration || pcs->ppcs->is_ref ||
#endif
        scs->static_config.recon_enabled) {
        uint8_t sb_size_log2 = (uint8_t)svt_log2f(scs->sb_size);
        bool    end_of_row_flag;
        for (uint32_t y_sb_index = 0; y_sb_index < picture_height_in_sb; ++y_sb_index) {
            for (uint32_t x_sb_index = 0; x_sb_index < pic_width_in_sb; ++x_sb_index) {
                uint32_t sb_origin_x = x_sb_index << sb_size_log2;
                uint32_t sb_origin_y = y_sb_index << sb_size_log2;

                end_of_row_flag = (x_sb_index == pic_width_in_sb - 1) ? true : false;

                svt_aom_loop_filter_sb(
                    frame_buffer, pcs, sb_origin_y >> 2, sb_origin_x >> 2, plane_start, plane_end, end_of_row_flag);
            }
        }
    }
#else
    for (y_sb_index = 0; y_sb_index < picture_height_in_sb; ++y_sb_index) {
        for (x_sb_index = 0; x_sb_index < pic_width_in_sb; ++x_sb_index) {
            //sb_index        = (uint16_t)(y_sb_index * pic_width_in_sb + x_sb_index);
            //sb_ptr          = pcs->sb_ptr_array[sb_index];
            sb_origin_x     = x_sb_index << sb_size_log2;
            sb_origin_y     = y_sb_index << sb_size_log2;
            end_of_row_flag = (x_sb_index == pic_width_in_sb - 1) ? true : false;

            svt_aom_loop_filter_sb(
                frame_buffer, pcs, sb_origin_y >> 2, sb_origin_x >> 2, plane_start, plane_end, end_of_row_flag);
        }
    }
#endif
}

void svt_copy_buffer(EbPictureBufferDesc* srcBuffer, EbPictureBufferDesc* dstBuffer, PictureControlSet* pcs,
                     uint8_t plane) {
    bool is_16bit           = pcs->ppcs->scs->is_16bit_pipeline;
    dstBuffer->org_x        = srcBuffer->org_x;
    dstBuffer->org_y        = srcBuffer->org_y;
    dstBuffer->origin_bot_y = srcBuffer->origin_bot_y;
    dstBuffer->width        = srcBuffer->width;
    dstBuffer->height       = srcBuffer->height;
    dstBuffer->max_width    = srcBuffer->max_width;
    dstBuffer->max_height   = srcBuffer->max_height;
    dstBuffer->bit_depth    = srcBuffer->bit_depth;
    dstBuffer->color_format = srcBuffer->color_format;
    dstBuffer->luma_size    = srcBuffer->luma_size;
    dstBuffer->chroma_size  = srcBuffer->chroma_size;
    dstBuffer->packed_flag  = srcBuffer->packed_flag;

    uint32_t luma_buffer_offset = (srcBuffer->org_x + srcBuffer->org_y * srcBuffer->stride_y) << is_16bit;
    uint16_t luma_width         = ALIGN_POWER_OF_TWO(srcBuffer->width, 3) << is_16bit;
    uint16_t luma_height        = ALIGN_POWER_OF_TWO(srcBuffer->height, 3);

    uint16_t chroma_width = (luma_width >> 1);
    if (plane == 0) {
        uint16_t stride_y = srcBuffer->stride_y << is_16bit;

        dstBuffer->stride_y         = srcBuffer->stride_y;
        dstBuffer->stride_bit_inc_y = srcBuffer->stride_bit_inc_y;

        for (int32_t input_row_index = 0; input_row_index < luma_height; input_row_index++) {
            svt_memcpy((dstBuffer->buffer_y + luma_buffer_offset + stride_y * input_row_index),
                       (srcBuffer->buffer_y + luma_buffer_offset + stride_y * input_row_index),
                       luma_width);
        }
    } else if (plane == 1) {
        uint16_t stride_cb           = srcBuffer->stride_cb << is_16bit;
        dstBuffer->stride_cb         = srcBuffer->stride_cb;
        dstBuffer->stride_bit_inc_cb = srcBuffer->stride_bit_inc_cb;

        uint32_t chroma_buffer_offset = (srcBuffer->org_x / 2 + srcBuffer->org_y / 2 * srcBuffer->stride_cb)
            << is_16bit;

        for (int32_t input_row_index = 0; input_row_index < luma_height / 2; input_row_index++) {
            svt_memcpy((dstBuffer->buffer_cb + chroma_buffer_offset + stride_cb * input_row_index),
                       (srcBuffer->buffer_cb + chroma_buffer_offset + stride_cb * input_row_index),
                       chroma_width);
        }
    } else if (plane == 2) {
        uint16_t stride_cr = srcBuffer->stride_cr << is_16bit;

        dstBuffer->stride_cr         = srcBuffer->stride_cr;
        dstBuffer->stride_bit_inc_cr = srcBuffer->stride_bit_inc_cr;

        uint32_t chroma_buffer_offset = (srcBuffer->org_x / 2 + srcBuffer->org_y / 2 * srcBuffer->stride_cr)
            << is_16bit;

        for (int32_t input_row_index = 0; input_row_index < luma_height / 2; input_row_index++) {
            svt_memcpy((dstBuffer->buffer_cr + chroma_buffer_offset + stride_cr * input_row_index),
                       (srcBuffer->buffer_cr + chroma_buffer_offset + stride_cr * input_row_index),
                       chroma_width);
        }
    }
}

uint64_t picture_sse_calculations(PictureControlSet* pcs, EbPictureBufferDesc* recon_ptr, int32_t plane) {
    SequenceControlSet* scs      = pcs->ppcs->scs;
    bool                is_16bit = scs->is_16bit_pipeline;

    // svt_spatial_full_distortion_kernel note:
    // intrinsic optimization require width and height in 4 pixel aligned.
    // when scaling is enabled the width and height might be not aligned.
    // here uses aligned_width and aligned_height to avoid wrong sse results.
    // if encoding in non-scaled frame, aligned_width and aligned_height equals
    // frame width and height, it has no effect to original resolution
    const uint16_t input_align_width  = pcs->ppcs->aligned_width;
    const uint16_t input_align_height = pcs->ppcs->aligned_height;
    const uint32_t ss_x               = scs->subsampling_x;
    const uint32_t ss_y               = scs->subsampling_y;

    uint8_t* input_buffer;
    uint8_t* recon_coeff_buffer;

    if (!is_16bit) {
        EbPictureBufferDesc* input_pic = (EbPictureBufferDesc*)pcs->ppcs->enhanced_pic;

        if (plane == 0) {
            recon_coeff_buffer = (uint8_t*)&(
                (recon_ptr->buffer_y)[recon_ptr->org_x + recon_ptr->org_y * recon_ptr->stride_y]);
            input_buffer = (uint8_t*)&(
                (input_pic->buffer_y)[input_pic->org_x + input_pic->org_y * input_pic->stride_y]);

            return svt_spatial_full_distortion_kernel(input_buffer,
                                                      0,
                                                      input_pic->stride_y,
                                                      recon_coeff_buffer,
                                                      0,
                                                      recon_ptr->stride_y,
                                                      input_align_width,
                                                      input_align_height);
        } else if (plane == 1) {
            recon_coeff_buffer = (uint8_t*)&(
                (recon_ptr->buffer_cb)[recon_ptr->org_x / 2 + recon_ptr->org_y / 2 * recon_ptr->stride_cb]);
            input_buffer = (uint8_t*)&(
                (input_pic->buffer_cb)[input_pic->org_x / 2 + input_pic->org_y / 2 * input_pic->stride_cb]);

            return svt_spatial_full_distortion_kernel(input_buffer,
                                                      0,
                                                      input_pic->stride_cb,
                                                      recon_coeff_buffer,
                                                      0,
                                                      recon_ptr->stride_cb,
                                                      input_align_width >> ss_x,
                                                      input_align_height >> ss_y);
        } else if (plane == 2) {
            recon_coeff_buffer = (uint8_t*)&(
                (recon_ptr->buffer_cr)[recon_ptr->org_x / 2 + recon_ptr->org_y / 2 * recon_ptr->stride_cr]);
            input_buffer = (uint8_t*)&(
                (input_pic->buffer_cr)[input_pic->org_x / 2 + input_pic->org_y / 2 * input_pic->stride_cr]);

            return svt_spatial_full_distortion_kernel(input_buffer,
                                                      0,
                                                      input_pic->stride_cr,
                                                      recon_coeff_buffer,
                                                      0,
                                                      recon_ptr->stride_cr,
                                                      input_align_width >> ss_x,
                                                      input_align_height >> ss_y);
        }
        return 0;
    } else {
        EbPictureBufferDesc* input_pic = (EbPictureBufferDesc*)pcs->input_frame16bit;

        if (plane == 0) {
            recon_coeff_buffer = (uint8_t*)&(
                (recon_ptr->buffer_y)[(recon_ptr->org_x + recon_ptr->org_y * recon_ptr->stride_y) << is_16bit]);
            input_buffer = (uint8_t*)&(
                (input_pic->buffer_y)[(input_pic->org_x + input_pic->org_y * input_pic->stride_y) << is_16bit]);

            return svt_full_distortion_kernel16_bits(input_buffer,
                                                     0,
                                                     input_pic->stride_y,
                                                     recon_coeff_buffer,
                                                     0,
                                                     recon_ptr->stride_y,
                                                     input_align_width,
                                                     input_align_height);
        } else if (plane == 1) {
            recon_coeff_buffer = (uint8_t*)&(
                (recon_ptr
                     ->buffer_cb)[(recon_ptr->org_x / 2 + recon_ptr->org_y / 2 * recon_ptr->stride_cb) << is_16bit]);
            input_buffer = (uint8_t*)&(
                (input_pic
                     ->buffer_cb)[(input_pic->org_x / 2 + input_pic->org_y / 2 * input_pic->stride_cb) << is_16bit]);

            return svt_full_distortion_kernel16_bits(input_buffer,
                                                     0,
                                                     input_pic->stride_cb,
                                                     recon_coeff_buffer,
                                                     0,
                                                     recon_ptr->stride_cb,
                                                     (input_align_width + ss_x) >> ss_x,
                                                     (input_align_height + ss_y) >> ss_y);
        } else if (plane == 2) {
            recon_coeff_buffer = (uint8_t*)&(
                (recon_ptr
                     ->buffer_cr)[(recon_ptr->org_x / 2 + recon_ptr->org_y / 2 * recon_ptr->stride_cr) << is_16bit]);
            input_buffer = (uint8_t*)&(
                (input_pic
                     ->buffer_cr)[(input_pic->org_x / 2 + input_pic->org_y / 2 * input_pic->stride_cr) << is_16bit]);

            return svt_full_distortion_kernel16_bits(input_buffer,
                                                     0,
                                                     input_pic->stride_cr,
                                                     recon_coeff_buffer,
                                                     0,
                                                     recon_ptr->stride_cr,
                                                     (input_align_width + ss_x) >> ss_x,
                                                     (input_align_height + ss_y) >> ss_y);
        }
        return 0;
    }
}

/*************************************************************************************************
* try_filter_frame
* Sett the filter levels, compute the filtering sse, and resett the recon buffer.
* Returns the filtering SSE
*************************************************************************************************/
static int64_t try_filter_frame(const EbPictureBufferDesc* sd, EbPictureBufferDesc* temp_lf_recon_buffer,
                                PictureControlSet* pcs, int32_t filt_level, int32_t partial_frame, int32_t plane,
                                int32_t dir) {
    (void)sd;
    (void)partial_frame;
    (void)sd;
    int64_t      filt_err;
    FrameHeader* frm_hdr = &pcs->ppcs->frm_hdr;
    assert(plane >= 0 && plane <= 2);
    int32_t filter_level[2] = {filt_level, filt_level};
    if (plane == 0 && dir == 0) {
        filter_level[1] = frm_hdr->loop_filter_params.filter_level[1];
    }
    if (plane == 0 && dir == 1) {
        filter_level[0] = frm_hdr->loop_filter_params.filter_level[0];
    }

    bool                 is_16bit = pcs->ppcs->scs->is_16bit_pipeline;
    EbPictureBufferDesc* recon_buffer;
    svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);

    // set base filters for use of get_filter_level when in DELTA_Q_LF mode
    switch (plane) {
    case 0:
        frm_hdr->loop_filter_params.filter_level[0] = filter_level[0];
        frm_hdr->loop_filter_params.filter_level[1] = filter_level[1];
        break;
    case 1:
        frm_hdr->loop_filter_params.filter_level_u = filter_level[0];
        break;
    case 2:
        frm_hdr->loop_filter_params.filter_level_v = filter_level[0];
        break;
    }

    svt_av1_loop_filter_frame(recon_buffer, pcs, plane, plane + 1);

    filt_err = picture_sse_calculations(pcs, recon_buffer, plane);

    // Re-instate the unfiltered frame; if both filters are off, no need to copy as there was no change to the pic
    if (filter_level[0] || filter_level[1]) {
        svt_copy_buffer(
            temp_lf_recon_buffer /*cpi->last_frame_uf*/, recon_buffer /*cm->frame_to_show*/, pcs, (uint8_t)plane);
    }

    return filt_err;
}

/*************************************************************************************************
* search_filter_level
* Perform a search for the best filter level for the picture data plane
*************************************************************************************************/
static int32_t search_filter_level(EbPictureBufferDesc* sd, // source
                                   EbPictureBufferDesc* temp_lf_recon_buffer, PictureControlSet* pcs,
                                   int32_t partial_frame, const int32_t* last_frame_filter_level, double* best_cost_ret,
                                   int32_t plane, int32_t dir) {
    const int32_t min_filter_level = 0;
    const int32_t max_filter_level = MAX_LOOP_FILTER; // av1_get_max_filter_level(cpi);
    int32_t       filt_direction   = 0;
    int64_t       best_err;
    int32_t       filt_best;
    FrameHeader*  frm_hdr = &pcs->ppcs->frm_hdr;
    //Macroblock *x = &cpi->td.mb;

    // Start the search at the previous frame filter level unless it is now out of
    // range.
    int32_t lvl;
    switch (plane) {
    case 0:
        if (dir > 1) {
            lvl = (last_frame_filter_level[0] + last_frame_filter_level[1] + 1) >> 1;
        } else {
            lvl = last_frame_filter_level[dir];
        }
        break;
    case 1:
        lvl = last_frame_filter_level[2];
        break;
    case 2:
        lvl = last_frame_filter_level[3];
        break;
    default:
        assert(plane >= 0 && plane <= 2);
        return 0;
    }
    int32_t filt_mid    = clamp(lvl, min_filter_level, max_filter_level);
    int32_t filter_step = filt_mid < 16 ? 4 : filt_mid / 4;

    bool                 is_16bit = pcs->ppcs->scs->is_16bit_pipeline;
    EbPictureBufferDesc* recon_buffer;
    svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);
    // Sum squared error at each filter level
    int64_t ss_err[MAX_LOOP_FILTER + 1];

    // Set each entry to -1
    memset(ss_err, 0xFF, sizeof(ss_err));
    // make a copy of recon_buffer
    svt_copy_buffer(
        recon_buffer /*cm->frame_to_show*/, temp_lf_recon_buffer /*&cpi->last_frame_uf*/, pcs, (uint8_t)plane);

    best_err                = try_filter_frame(sd, temp_lf_recon_buffer, pcs, filt_mid, partial_frame, plane, dir);
    filt_best               = filt_mid;
    ss_err[filt_mid]        = best_err;
    int32_t tot_convergence = 0;
    while (filter_step > 0) {
        const int32_t filt_high = AOMMIN(filt_mid + filter_step, max_filter_level);
        const int32_t filt_low  = AOMMAX(filt_mid - filter_step, min_filter_level);

        // Bias against raising loop filter in favor of lowering it.
        int64_t bias = (best_err >> (15 - (filt_mid / 8))) * filter_step;

        // yx, bias less for large block size
        if (frm_hdr->tx_mode != ONLY_4X4) {
            bias >>= 1;
        }

        if (filt_direction <= 0 && filt_low != filt_mid) {
            // Get Low filter error score
            if (ss_err[filt_low] < 0) {
                ss_err[filt_low] = try_filter_frame(sd, temp_lf_recon_buffer, pcs, filt_low, partial_frame, plane, dir);
            }
            // If value is close to the best so far then bias towards a lower loop
            // filter value.
            if (ss_err[filt_low] < (best_err + bias)) {
                // Was it actually better than the previous best?
                if (ss_err[filt_low] < best_err) {
                    best_err = ss_err[filt_low];
                }
                filt_best = filt_low;
            }
        }

        // Now look at filt_high
        if (filt_direction >= 0 && filt_high != filt_mid) {
            if (ss_err[filt_high] < 0) {
                ss_err[filt_high] = try_filter_frame(
                    sd, temp_lf_recon_buffer, pcs, filt_high, partial_frame, plane, dir);
            }
            // If value is significantly better than previous best, bias added against
            // raising filter value
            if (ss_err[filt_high] < (best_err - bias)) {
                best_err  = ss_err[filt_high];
                filt_best = filt_high;
            }
        }

        // Half the step distance if the best filter value was the same as last time
        if (filt_best == filt_mid) {
            tot_convergence++;
            if (tot_convergence == pcs->ppcs->dlf_ctrls.early_exit_convergence) {
                filter_step = 0;
            } else {
                filter_step /= 2;
            }
            filt_direction = 0;
        } else {
            filt_direction = (filt_best < filt_mid) ? -1 : 1;
            filt_mid       = filt_best;
        }
    }
    // Update best error
    best_err = ss_err[filt_best];

    if (plane == 0) {
        if (ss_err[0] >= 0) {
            pcs->zero_filt_sse = ss_err[0];
        }

        if (ss_err[filt_best] >= 0) {
            pcs->best_filt_sse = best_err;
        }
    }

    if (best_cost_ret) {
        *best_cost_ret = (double)best_err; //RDCOST_DBL(x->rdmult, 0, best_err);
    }
    return filt_best;
}

static void me_based_dlf_skip(PictureControlSet* pcs, uint16_t prev_dlf_dist_th, bool* do_y, bool* do_uv) {
    *do_y  = true;
    *do_uv = true;
    if (pcs->slice_type == I_SLICE) {
        return;
    }

    const uint8_t  in_res               = pcs->ppcs->input_resolution;
    const uint32_t use_zero_strength_th = disable_dlf_th[pcs->ppcs->dlf_ctrls.zero_filter_strength_lvl][in_res] *
        (pcs->temporal_layer_index + 1);
    if (!use_zero_strength_th) {
        return;
    }

    uint32_t total_me_sad = 0;
    for (uint16_t b64_index = 0; b64_index < pcs->b64_total_count; ++b64_index) {
        total_me_sad += pcs->ppcs->rc_me_distortion[b64_index];
    }
    uint32_t average_me_sad = total_me_sad / pcs->b64_total_count;

    int32_t prev_dlf_dist = 0;
    if (prev_dlf_dist_th) {
        int32_t tot_refs = 0;
        for (uint32_t ref_it = 0; ref_it < pcs->ppcs->tot_ref_frame_types; ++ref_it) {
            MvReferenceFrame ref_pair = pcs->ppcs->ref_frame_type_arr[ref_it];
            MvReferenceFrame rf[2];
            av1_set_ref_frame(rf, ref_pair);

            if (rf[1] == NONE_FRAME) {
                uint8_t            list_idx = get_list_idx(rf[0]);
                uint8_t            ref_idx  = get_ref_frame_idx(rf[0]);
                EbReferenceObject* ref_obj  = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;

                if (ref_obj->dlf_dist_dev >= 0 && ref_obj->tmp_layer_idx <= pcs->temporal_layer_index) {
                    prev_dlf_dist += ref_obj->dlf_dist_dev;
                    tot_refs++;
                }
            }
        }
        if (tot_refs) {
            prev_dlf_dist /= tot_refs;
        }
    }

    if (!prev_dlf_dist_th || (prev_dlf_dist < prev_dlf_dist_th * (pcs->temporal_layer_index + 1))) {
        if (average_me_sad < use_zero_strength_th) {
            *do_y = false;
        }
        if (average_me_sad < (use_zero_strength_th * 2)) {
            *do_uv = false;
        }
    }
}

/*************************************************************************************************
* svt_av1_pick_filter_level_by_q
* Choose the optimal loop filter levels by qindex
*************************************************************************************************/
void svt_av1_pick_filter_level_by_q(PictureControlSet* pcs, uint8_t qindex, int32_t* filter_level) {
    SequenceControlSet* scs     = pcs->scs;
    FrameHeader*        frm_hdr = &pcs->ppcs->frm_hdr;

    const uint8_t in_res           = pcs->ppcs->input_resolution;
    const int32_t inter_frame_mult = inter_frame_multiplier[in_res];

    int32_t min_ref_filter_level[2] = {MAX_LOOP_FILTER, MAX_LOOP_FILTER};
    int32_t min_ref_filter_level_u  = MAX_LOOP_FILTER;
    int32_t min_ref_filter_level_v  = MAX_LOOP_FILTER;

    for (uint32_t ref_it = 0; ref_it < pcs->ppcs->tot_ref_frame_types; ++ref_it) {
        MvReferenceFrame ref_pair = pcs->ppcs->ref_frame_type_arr[ref_it];
        MvReferenceFrame rf[2];
        av1_set_ref_frame(rf, ref_pair);

        if (rf[1] == NONE_FRAME) {
            uint8_t            list_idx = get_list_idx(rf[0]);
            uint8_t            ref_idx  = get_ref_frame_idx(rf[0]);
            EbReferenceObject* ref_obj  = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;

            min_ref_filter_level[0] = MIN(min_ref_filter_level[0], ref_obj->filter_level[0]);
            min_ref_filter_level[1] = MIN(min_ref_filter_level[1], ref_obj->filter_level[1]);
            min_ref_filter_level_u  = MIN(min_ref_filter_level_u, ref_obj->filter_level_u);
            min_ref_filter_level_v  = MIN(min_ref_filter_level_v, ref_obj->filter_level_v);
        }
    }

    const int32_t min_filter_level = 0;
    const int32_t max_filter_level = MAX_LOOP_FILTER; // av1_get_max_filter_level(cpi);
    const int32_t q                = svt_aom_ac_quant_qtx(qindex, 0, (EbBitDepth)scs->static_config.encoder_bit_depth);
    // These values were determined by linear fitting the result of the
    // searched level for 8 bit depth:
    // Keyframes: filt_guess = q * 0.06699 - 1.60817
    // Other frames: filt_guess = q * 0.02295 + 2.48225
    //
    // And high bit depth separately:
    // filt_guess = q * 0.316206 + 3.87252
    int32_t filt_guess;
    switch (scs->static_config.encoder_bit_depth) {
    case EB_EIGHT_BIT:
        filt_guess = (frm_hdr->frame_type == KEY_FRAME) ? ROUND_POWER_OF_TWO(q * 17563 - 421574, 18)
                                                        : ROUND_POWER_OF_TWO(q * inter_frame_mult + 650707, 18);
        break;
    case EB_TEN_BIT:
        filt_guess = ROUND_POWER_OF_TWO(q * 20723 + 4060632, 20);
        break;
    case EB_TWELVE_BIT:
        filt_guess = ROUND_POWER_OF_TWO(q * 20723 + 16242526, 22);
        break;
    default:
        assert(0 &&
               "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT "
               "or EB_TWELVE_BIT");
        return;
    }
    if (scs->static_config.encoder_bit_depth != EB_EIGHT_BIT && frm_hdr->frame_type == KEY_FRAME) {
        filt_guess -= 4;
    }

    int32_t filt_guess_chroma = filt_guess / 2;
    bool    do_y = true, do_uv = true;
    // Don't use prev_dlf_dist_th b/c deriving the filter strength from QP is mainly used for SB-based DLF and we do not compute the
    // SSE in that path.
    me_based_dlf_skip(pcs, 0, &do_y, &do_uv);
    if (!do_y) {
        filt_guess = 0;
    }
    if (!do_uv) {
        filt_guess_chroma = 0;
    }
    // Force filter_level to 0 if loop-filter is shut for 1 (or many) of the sub-layer reference frame(s)
    filter_level[0] = min_ref_filter_level[0] || !pcs->ppcs->temporal_layer_index
        ? clamp(filt_guess, min_filter_level, max_filter_level)
        : 0;

    filter_level[1] = min_ref_filter_level[1] || !pcs->ppcs->temporal_layer_index
        ? clamp(filt_guess, min_filter_level, max_filter_level)
        : 0;

    filter_level[2] = min_ref_filter_level_u || !pcs->ppcs->temporal_layer_index
        ? clamp(filt_guess_chroma, min_filter_level, max_filter_level)
        : 0;

    filter_level[3] = min_ref_filter_level_v || !pcs->ppcs->temporal_layer_index
        ? clamp(filt_guess_chroma, min_filter_level, max_filter_level)
        : 0;
}

/*************************************************************************************************
* svt_av1_pick_filter_level
* Choose the optimal loop filter levels
*************************************************************************************************/
EbErrorType svt_av1_pick_filter_level(EbPictureBufferDesc* srcBuffer, // source input
                                      PictureControlSet* pcs, LpfPickMethod method) {
    SequenceControlSet* scs     = pcs->scs;
    FrameHeader*        frm_hdr = &pcs->ppcs->frm_hdr;
    (void)srcBuffer;
    struct LoopFilter* const lf            = &frm_hdr->loop_filter_params;
    const int32_t            sharpness_val = CLIP3(0, 7, pcs->scs->static_config.sharpness);
    lf->sharpness_level                    = sharpness_val;
    if (frm_hdr->frame_type == KEY_FRAME && pcs->scs->static_config.tune == TUNE_VQ) {
        lf->sharpness_level = MIN(7, sharpness_val + 2);
    } else if (pcs->scs->static_config.tune == TUNE_IQ || pcs->scs->static_config.tune == TUNE_MS_SSIM) {
        // Loop filter sharpness levels are highly nonlinear. Visually, lf sharpness 1 is closer to 7 than
        // it is to 0, so in practice let's choose between levels 0, 1 and 7 to keep it simple
        int32_t max_lf_sharpness;

        if (frm_hdr->quantization_params.base_q_idx <= 112) {
            max_lf_sharpness = 7;
        } else if (frm_hdr->quantization_params.base_q_idx <= 160) {
            max_lf_sharpness = 1;
        } else {
            max_lf_sharpness = 0;
        }

        lf->sharpness_level = MIN(lf->sharpness_level, max_lf_sharpness);
    }
    if (method == LPF_PICK_MINIMAL_LPF) {
        lf->filter_level[0] = lf->filter_level[1] = 0;
    } else if (method >= LPF_PICK_FROM_Q) {
        int32_t filter_level[4];
        svt_av1_pick_filter_level_by_q(pcs, frm_hdr->quantization_params.base_q_idx, filter_level);
        lf->filter_level[0] = filter_level[0];
        lf->filter_level[1] = filter_level[1];
        lf->filter_level_u  = filter_level[2];
        lf->filter_level_v  = filter_level[3];
    } else {
        uint16_t padding = scs->super_block_size + 32;
        if (scs->static_config.superres_mode > SUPERRES_NONE || scs->static_config.resize_mode > RESIZE_NONE) {
            padding += scs->super_block_size;
        }
        EbPictureBufferDescInitData temp_lf_recon_desc_init_data;
        temp_lf_recon_desc_init_data.max_width          = (uint16_t)scs->max_input_luma_width;
        temp_lf_recon_desc_init_data.max_height         = (uint16_t)scs->max_input_luma_height;
        temp_lf_recon_desc_init_data.buffer_enable_mask = PICTURE_BUFFER_DESC_FULL_MASK;

        temp_lf_recon_desc_init_data.left_padding  = padding;
        temp_lf_recon_desc_init_data.right_padding = padding;
        temp_lf_recon_desc_init_data.top_padding   = padding;
        temp_lf_recon_desc_init_data.bot_padding   = padding;
        temp_lf_recon_desc_init_data.split_mode    = false;
        temp_lf_recon_desc_init_data.color_format  = scs->static_config.encoder_color_format;
        bool is_16bit                              = scs->static_config.encoder_bit_depth > 8 ? true : false;
        if (scs->is_16bit_pipeline || is_16bit) {
            temp_lf_recon_desc_init_data.bit_depth = EB_SIXTEEN_BIT;
            EB_NEW(
                pcs->temp_lf_recon_pic_16bit, svt_recon_picture_buffer_desc_ctor, (EbPtr)&temp_lf_recon_desc_init_data);
            if (!is_16bit) {
                pcs->temp_lf_recon_pic_16bit->bit_depth = EB_EIGHT_BIT;
            }
        } else {
            temp_lf_recon_desc_init_data.bit_depth = EB_EIGHT_BIT;
            EB_NEW(pcs->temp_lf_recon_pic, svt_recon_picture_buffer_desc_ctor, (EbPtr)&temp_lf_recon_desc_init_data);
        }

        if (pcs->ppcs->dlf_ctrls.dlf_avg && pcs->ppcs->tot_ref_frame_types > 0) {
            int32_t tot_ref_filter_level[2] = {0, 0};
            int32_t tot_ref_filter_level_u  = 0;
            int32_t tot_ref_filter_level_v  = 0;

            int32_t tot_refs = 0;

            for (uint32_t ref_it = 0; ref_it < pcs->ppcs->tot_ref_frame_types; ++ref_it) {
                MvReferenceFrame ref_pair = pcs->ppcs->ref_frame_type_arr[ref_it];
                MvReferenceFrame rf[2];
                av1_set_ref_frame(rf, ref_pair);

                if (rf[1] == NONE_FRAME) {
                    uint8_t            list_idx = get_list_idx(rf[0]);
                    uint8_t            ref_idx  = get_ref_frame_idx(rf[0]);
                    EbReferenceObject* ref_obj  = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;

                    tot_ref_filter_level[0] += ref_obj->filter_level[0];
                    tot_ref_filter_level[1] += ref_obj->filter_level[1];
                    tot_ref_filter_level_u += ref_obj->filter_level_u;
                    tot_ref_filter_level_v += ref_obj->filter_level_v;

                    tot_refs++;
                }
            }

            lf->filter_level[0] = tot_ref_filter_level[0] / tot_refs;
            lf->filter_level[1] = tot_ref_filter_level[1] / tot_refs;
            lf->filter_level_u  = tot_ref_filter_level_u / tot_refs;
            lf->filter_level_v  = tot_ref_filter_level_v / tot_refs;
        }
        bool do_y = true, do_uv = true;
        me_based_dlf_skip(pcs, pcs->ppcs->dlf_ctrls.prev_dlf_dist_th, &do_y, &do_uv);

        const int32_t last_frame_filter_level[4] = {
            lf->filter_level[0], lf->filter_level[1], lf->filter_level_u, lf->filter_level_v};
        EbPictureBufferDesc* temp_lf_recon_buffer = scs->is_16bit_pipeline ? pcs->temp_lf_recon_pic_16bit
                                                                           : pcs->temp_lf_recon_pic;

        if (!do_y) {
            lf->filter_level[0] = lf->filter_level[1] = 0;
        } else if (!pcs->temporal_layer_index || !pcs->ppcs->dlf_ctrls.use_ref_avg_y ||
                   pcs->ppcs->tot_ref_frame_types == 0) {
            lf->filter_level[0] = lf->filter_level[1] = search_filter_level(srcBuffer,
                                                                            temp_lf_recon_buffer,
                                                                            pcs,
                                                                            method == LPF_PICK_FROM_SUBIMAGE,
                                                                            last_frame_filter_level,
                                                                            NULL,
                                                                            0,
                                                                            2);
        } else if (last_frame_filter_level[0] || last_frame_filter_level[1]) {
            int32_t prev_dlf_dist = 0;
            int32_t tot_refs      = 0;
            for (uint32_t ref_it = 0; ref_it < pcs->ppcs->tot_ref_frame_types; ++ref_it) {
                MvReferenceFrame ref_pair = pcs->ppcs->ref_frame_type_arr[ref_it];
                MvReferenceFrame rf[2];
                av1_set_ref_frame(rf, ref_pair);

                if (rf[1] == NONE_FRAME) {
                    uint8_t            list_idx = get_list_idx(rf[0]);
                    uint8_t            ref_idx  = get_ref_frame_idx(rf[0]);
                    EbReferenceObject* ref_obj  = pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;

                    if (ref_obj->dlf_dist_dev >= 0) {
                        prev_dlf_dist += ref_obj->dlf_dist_dev;
                        tot_refs++;
                    }
                }
            }
            if (tot_refs) {
                prev_dlf_dist /= tot_refs;
            }

            // If improvement from DLF of ref frames is small, disable DLF for the current frame
            if (tot_refs && prev_dlf_dist < 5) {
                lf->filter_level[0] = lf->filter_level[1] = 0;
            }
        }

        if (!do_uv || (lf->filter_level[0] == 0 && lf->filter_level[1] == 0)) {
            // chroma filtering not allowed if luma filters off
            lf->filter_level_u = 0;
            lf->filter_level_v = 0;
        } else if (pcs->temporal_layer_index && pcs->ppcs->dlf_ctrls.use_ref_avg_uv &&
                   pcs->ppcs->tot_ref_frame_types > 0) {
            //use avg-ref for chroma
            lf->filter_level_u = last_frame_filter_level[2];
            lf->filter_level_v = last_frame_filter_level[3];
        } else {
            lf->filter_level_u = search_filter_level(srcBuffer,
                                                     temp_lf_recon_buffer,
                                                     pcs,
                                                     method == LPF_PICK_FROM_SUBIMAGE,
                                                     last_frame_filter_level,
                                                     NULL,
                                                     1,
                                                     0);
            lf->filter_level_v = search_filter_level(srcBuffer,
                                                     temp_lf_recon_buffer,
                                                     pcs,
                                                     method == LPF_PICK_FROM_SUBIMAGE,
                                                     last_frame_filter_level,
                                                     NULL,
                                                     2,
                                                     0);
        }
        EB_DELETE(pcs->temp_lf_recon_pic);
        EB_DELETE(pcs->temp_lf_recon_pic_16bit);
    }

    return EB_ErrorNone;
}
