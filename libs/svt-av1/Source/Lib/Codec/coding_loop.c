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
#include <string.h>

#include "coding_loop.h"
#include "utility.h"
#include "rd_cost.h"
#include "deblocking_filter.h"
#include "pic_operators.h"
#include "segmentation.h"
#include "enc_dec_process.h"
#include "EbSvtAv1ErrorCodes.h"
#include "transforms.h"
#include "inv_transforms.h"
#include "md_config_process.h"
#include "enc_intra_prediction.h"
#include "aom_dsp_rtcd.h"
#include "md_rate_estimation.h"
#include "full_loop.h"
#include "pack_unpack_c.h"
#include "enc_inter_prediction.h"

void aom_av1_set_ssim_rdmult(ModeDecisionContext* ctx, PictureControlSet* pcs, const int mi_row, const int mi_col);

static EbErrorType ec_rtime_alloc_palette_info(EcBlkStruct* md_blk_arr_nsq) {
    EB_MALLOC_ARRAY(md_blk_arr_nsq->palette_info, 1);
    EB_MALLOC_ARRAY(md_blk_arr_nsq->palette_info->color_idx_map, MAX_PALETTE_SQUARE);

    return EB_ErrorNone;
}

/*******************************************
* set Penalize Skip Flag
*
* Summary: Set the penalize_skipflag to true
* When there is luminance/chrominance change
* or in noisy clip with low motion at meduim
* varince area
*
*******************************************/

typedef void (*EbAv1EncodeLoopFuncPtr)(PictureControlSet* pcs, EncDecContext* ed_ctx, SuperBlock* sb_ptr,
                                       uint32_t org_x, uint32_t org_y,
                                       EbPictureBufferDesc* pred_samples, // no basis/offset
                                       EbPictureBufferDesc* coeff_samples_sb, // sb based
                                       EbPictureBufferDesc* residual16bit, // no basis/offset
                                       EbPictureBufferDesc* transform16bit, // no basis/offset
                                       EbPictureBufferDesc* inverse_quant_buffer, uint32_t component_mask,
                                       uint16_t* eob);

typedef void (*EbAv1GenerateReconFuncPtr)(EncDecContext* ed_ctx, uint32_t org_x, uint32_t org_y,
                                          EbPictureBufferDesc* pred_samples, // no basis/offset
                                          EbPictureBufferDesc* residual16bit, // no basis/offset
                                          uint32_t component_mask, uint16_t* eob);

/*******************************************
* Residual Kernel 8-16bit
    Computes the residual data
*******************************************/
void svt_aom_residual_kernel(uint8_t* input, uint32_t input_offset, uint32_t input_stride, uint8_t* pred,
                             uint32_t pred_offset, uint32_t pred_stride, int16_t* residual, uint32_t residual_offset,
                             uint32_t residual_stride, bool hbd, uint32_t area_width, uint32_t area_height) {
    if (hbd) {
        svt_residual_kernel16bit(((uint16_t*)input) + input_offset,
                                 input_stride,
                                 ((uint16_t*)pred) + pred_offset,
                                 pred_stride,
                                 residual + residual_offset,
                                 residual_stride,
                                 area_width,
                                 area_height);
    } else {
        svt_residual_kernel8bit(&(input[input_offset]),
                                input_stride,
                                &(pred[pred_offset]),
                                pred_stride,
                                residual + residual_offset,
                                residual_stride,
                                area_width,
                                area_height);
    }
}

/***************************************************
* Update Recon Samples Neighbor Arrays
***************************************************/
static void encode_pass_update_recon_sample_neighbour_arrays(
    NeighborArrayUnit* lumaReconSampleNeighborArray, NeighborArrayUnit* cbReconSampleNeighborArray,
    NeighborArrayUnit* crReconSampleNeighborArray, EbPictureBufferDesc* recon_buffer, uint32_t org_x, uint32_t org_y,
    uint32_t width, uint32_t height, uint32_t bwidth_uv, uint32_t bheight_uv, uint32_t component_mask, bool is_16bit) {
    uint32_t round_origin_x = (org_x >> 3) << 3; // for Chroma blocks with size of 4
    uint32_t round_origin_y = (org_y >> 3) << 3; // for Chroma blocks with size of 4

    if (is_16bit == true) {
        if (component_mask & PICTURE_BUFFER_DESC_LUMA_MASK) {
            // Recon Samples - Luma
            svt_aom_neighbor_array_unit16bit_sample_write(lumaReconSampleNeighborArray,
                                                          (uint16_t*)(recon_buffer->buffer_y),
                                                          recon_buffer->stride_y,
                                                          recon_buffer->org_x + org_x,
                                                          recon_buffer->org_y + org_y,
                                                          org_x,
                                                          org_y,
                                                          width,
                                                          height,
                                                          NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }

        if (component_mask & PICTURE_BUFFER_DESC_CHROMA_MASK) {
            // Recon Samples - Cb
            svt_aom_neighbor_array_unit16bit_sample_write(cbReconSampleNeighborArray,
                                                          (uint16_t*)(recon_buffer->buffer_cb),
                                                          recon_buffer->stride_cb,
                                                          (recon_buffer->org_x + round_origin_x) >> 1,
                                                          (recon_buffer->org_y + round_origin_y) >> 1,
                                                          round_origin_x >> 1,
                                                          round_origin_y >> 1,
                                                          bwidth_uv,
                                                          bheight_uv,
                                                          NEIGHBOR_ARRAY_UNIT_FULL_MASK);

            // Recon Samples - Cr
            svt_aom_neighbor_array_unit16bit_sample_write(crReconSampleNeighborArray,
                                                          (uint16_t*)(recon_buffer->buffer_cr),
                                                          recon_buffer->stride_cr,
                                                          (recon_buffer->org_x + round_origin_x) >> 1,
                                                          (recon_buffer->org_y + round_origin_y) >> 1,
                                                          round_origin_x >> 1,
                                                          round_origin_y >> 1,
                                                          bwidth_uv,
                                                          bheight_uv,
                                                          NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
    } else {
        if (component_mask & PICTURE_BUFFER_DESC_LUMA_MASK) {
            // Recon Samples - Luma
            svt_aom_neighbor_array_unit_sample_write(lumaReconSampleNeighborArray,
                                                     recon_buffer->buffer_y,
                                                     recon_buffer->stride_y,
                                                     recon_buffer->org_x + org_x,
                                                     recon_buffer->org_y + org_y,
                                                     org_x,
                                                     org_y,
                                                     width,
                                                     height,
                                                     NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }

        if (component_mask & PICTURE_BUFFER_DESC_CHROMA_MASK) {
            // Recon Samples - Cb
            svt_aom_neighbor_array_unit_sample_write(cbReconSampleNeighborArray,
                                                     recon_buffer->buffer_cb,
                                                     recon_buffer->stride_cb,
                                                     (recon_buffer->org_x + round_origin_x) >> 1,
                                                     (recon_buffer->org_y + round_origin_y) >> 1,
                                                     round_origin_x >> 1,
                                                     round_origin_y >> 1,
                                                     bwidth_uv,
                                                     bheight_uv,
                                                     NEIGHBOR_ARRAY_UNIT_FULL_MASK);

            // Recon Samples - Cr
            svt_aom_neighbor_array_unit_sample_write(crReconSampleNeighborArray,
                                                     recon_buffer->buffer_cr,
                                                     recon_buffer->stride_cr,
                                                     (recon_buffer->org_x + round_origin_x) >> 1,
                                                     (recon_buffer->org_y + round_origin_y) >> 1,
                                                     round_origin_x >> 1,
                                                     round_origin_y >> 1,
                                                     bwidth_uv,
                                                     bheight_uv,
                                                     NEIGHBOR_ARRAY_UNIT_FULL_MASK);
        }
    }

    return;
}

/**********************************************************
* Encode Loop
*
* Summary: Performs an AV1 conformant CfL prediction based on
* recon luma samples in pred_samples
*
* Inputs:
*   pred_samples - recon luma samples on which CfL prediction is based
*
* Outputs:
*   pred_samples - predicted chroma samples for cb and cr
*
**********************************************************/
static void av1_encode_generate_cfl_prediction(EbPictureBufferDesc* pred_samples, EncDecContext* ed_ctx,
                                               uint32_t pred_cb_offset, uint32_t pred_cr_offset,
                                               uint32_t round_origin_x, uint32_t round_origin_y) {
    bool             is_16bit = ed_ctx->is_16bit;
    const BlockGeom* blk_geom = ed_ctx->blk_geom;
    BlkStruct*       blk_ptr  = ed_ctx->blk_ptr;

    EbPictureBufferDesc* recon_samples = pred_samples;

    uint32_t recon_luma_offset = (recon_samples->org_y + round_origin_y) * recon_samples->stride_y +
        (recon_samples->org_x + round_origin_x);

    // Down sample Luma
    if (is_16bit) {
        svt_cfl_luma_subsampling_420_hbd(
            ((uint16_t*)recon_samples->buffer_y) + recon_luma_offset,
            recon_samples->stride_y,
            ed_ctx->md_ctx->pred_buf_q3,
            blk_geom->bwidth_uv == blk_geom->bwidth ? (blk_geom->bwidth_uv << 1) : blk_geom->bwidth,
            blk_geom->bheight_uv == blk_geom->bheight ? (blk_geom->bheight_uv << 1) : blk_geom->bheight);
    } else {
        svt_cfl_luma_subsampling_420_lbd(
            recon_samples->buffer_y + recon_luma_offset,
            recon_samples->stride_y,
            ed_ctx->md_ctx->pred_buf_q3,
            blk_geom->bwidth_uv == blk_geom->bwidth ? (blk_geom->bwidth_uv << 1) : blk_geom->bwidth,
            blk_geom->bheight_uv == blk_geom->bheight ? (blk_geom->bheight_uv << 1) : blk_geom->bheight);
    }

    const TxSize tx_size_uv   = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
    const int    tx_width_uv  = tx_size_wide[tx_size_uv];
    const int    tx_height_uv = tx_size_high[tx_size_uv];

    int32_t round_offset = (tx_width_uv * tx_height_uv) / 2;

    svt_subtract_average(ed_ctx->md_ctx->pred_buf_q3,
                         tx_width_uv,
                         tx_height_uv,
                         round_offset,
                         svt_log2f(tx_width_uv) + svt_log2f(tx_height_uv));

    int32_t alpha_q3_cb = cfl_idx_to_alpha(blk_ptr->block_mi.cfl_alpha_idx,
                                           blk_ptr->block_mi.cfl_alpha_signs,
                                           CFL_PRED_U); // once for U, once for V
    int32_t alpha_q3_cr = cfl_idx_to_alpha(blk_ptr->block_mi.cfl_alpha_idx,
                                           blk_ptr->block_mi.cfl_alpha_signs,
                                           CFL_PRED_V); // once for U, once for V

    if (is_16bit) {
        svt_cfl_predict_hbd(ed_ctx->md_ctx->pred_buf_q3,
                            ((uint16_t*)pred_samples->buffer_cb) + pred_cb_offset,
                            pred_samples->stride_cb,
                            ((uint16_t*)pred_samples->buffer_cb) + pred_cb_offset,
                            pred_samples->stride_cb,
                            alpha_q3_cb,
                            ed_ctx->bit_depth,
                            tx_width_uv,
                            tx_height_uv);

        svt_cfl_predict_hbd(ed_ctx->md_ctx->pred_buf_q3,
                            ((uint16_t*)pred_samples->buffer_cr) + pred_cr_offset,
                            pred_samples->stride_cr,
                            ((uint16_t*)pred_samples->buffer_cr) + pred_cr_offset,
                            pred_samples->stride_cr,
                            alpha_q3_cr,
                            ed_ctx->bit_depth,
                            tx_width_uv,
                            tx_height_uv);
    } else {
        svt_cfl_predict_lbd(ed_ctx->md_ctx->pred_buf_q3,
                            pred_samples->buffer_cb + pred_cb_offset,
                            pred_samples->stride_cb,
                            pred_samples->buffer_cb + pred_cb_offset,
                            pred_samples->stride_cb,
                            alpha_q3_cb,
                            8,
                            tx_width_uv,
                            tx_height_uv);

        svt_cfl_predict_lbd(ed_ctx->md_ctx->pred_buf_q3,
                            pred_samples->buffer_cr + pred_cr_offset,
                            pred_samples->stride_cr,
                            pred_samples->buffer_cr + pred_cr_offset,
                            pred_samples->stride_cr,
                            alpha_q3_cr,
                            8,
                            tx_width_uv,
                            tx_height_uv);
    }
}

/**********************************************************
* Encode Loop
*
* Summary: Performs an AV1 conformant
*   Transform, Quantization  and Inverse Quantization of a TU.
*
* Inputs:
*   org_x
*   org_y
*   txb_size
*   sb_sz
*   input - input samples (position sensitive)
*   pred - prediction samples (position independent)
*
* Outputs:
*   Inverse quantized coeff - quantization indices (position sensitive)
*
**********************************************************/
static void av1_encode_loop(PictureControlSet* pcs, EncDecContext* ed_ctx, uint32_t org_x, uint32_t org_y,
                            EbPictureBufferDesc* pred_samples, // no basis/offset
                            EbPictureBufferDesc* coeff_samples_sb, // sb based
                            EbPictureBufferDesc* residual16bit, // no basis/offset
                            EbPictureBufferDesc* transform16bit, // no basis/offset
                            EbPictureBufferDesc* inverse_quant_buffer, uint32_t component_mask, uint16_t* eob)

{
    ModeDecisionContext* md_ctx        = ed_ctx->md_ctx;
    const BlockGeom*     blk_geom      = ed_ctx->blk_geom;
    BlkStruct*           blk_ptr       = ed_ctx->blk_ptr;
    const uint32_t       qindex        = blk_ptr->qindex;
    const bool           is_16bit      = ed_ctx->is_16bit;
    const uint32_t       bit_depth     = ed_ctx->bit_depth;
    EbPictureBufferDesc* input_samples = is_16bit ? ed_ctx->input_sample16bit_buffer : ed_ctx->input_samples;

    const bool     is_inter       = is_inter_block(&blk_ptr->block_mi);
    const uint32_t round_origin_x = ROUND_UV(org_x); // for Chroma blocks with size of 4
    const uint32_t round_origin_y = ROUND_UV(org_y); // for Chroma blocks with size of 4
    const uint8_t  tx_depth       = blk_ptr->block_mi.tx_depth;
    // Get the tx origin coordinates within the SB (not frame)
    const uint16_t tx_org_x = org_x - md_ctx->sb_origin_x;
    const uint16_t tx_org_y = org_y - md_ctx->sb_origin_y;
    const int32_t  seg_qp   = pcs->ppcs->frm_hdr.segmentation_params.segmentation_enabled
           ? pcs->ppcs->frm_hdr.segmentation_params.feature_data[ed_ctx->blk_ptr->segment_id][SEG_LVL_ALT_Q]
           : 0;

    uint32_t input_luma_offset, input_cb_offset, input_cr_offset;
    uint32_t pred_luma_offset, pred_cb_offset, pred_cr_offset;
    uint32_t scratch_luma_offset, scratch_cb_offset, scratch_cr_offset;
    if (is_16bit) {
        input_luma_offset = tx_org_x + tx_org_y * input_samples->stride_y;
        input_cb_offset   = ROUND_UV(tx_org_x) / 2 + ROUND_UV(tx_org_y) / 2 * input_samples->stride_cb;
        input_cr_offset   = ROUND_UV(tx_org_x) / 2 + ROUND_UV(tx_org_y) / 2 * input_samples->stride_cr;
        pred_luma_offset  = ((pred_samples->org_y + org_y) * pred_samples->stride_y) + (pred_samples->org_x + org_x);
        pred_cb_offset    = ((pred_samples->org_x + round_origin_x) >> 1) +
            (((pred_samples->org_y + round_origin_y) >> 1) * pred_samples->stride_cb);
        pred_cr_offset = ((pred_samples->org_x + round_origin_x) >> 1) +
            (((pred_samples->org_y + round_origin_y) >> 1) * pred_samples->stride_cr);
    } else {
        input_luma_offset = ((org_y + input_samples->org_y) * input_samples->stride_y) + (org_x + input_samples->org_x);
        input_cb_offset   = (((round_origin_y + input_samples->org_y) >> 1) * input_samples->stride_cb) +
            ((round_origin_x + input_samples->org_x) >> 1);
        input_cr_offset = (((round_origin_y + input_samples->org_y) >> 1) * input_samples->stride_cr) +
            ((round_origin_x + input_samples->org_x) >> 1);
        pred_luma_offset = (pred_samples->org_x + org_x) + ((pred_samples->org_y + org_y) * pred_samples->stride_y);
        pred_cb_offset   = ((pred_samples->org_x + round_origin_x) >> 1) +
            (((pred_samples->org_y + round_origin_y) >> 1) * pred_samples->stride_cb);
        pred_cr_offset = ((pred_samples->org_x + round_origin_x) >> 1) +
            (((pred_samples->org_y + round_origin_y) >> 1) * pred_samples->stride_cr);
    }

    if (bit_depth != EB_EIGHT_BIT) {
        // Get the block origin coordinates within the SB (not frame)
        const uint16_t blk_org_x_in_sb = md_ctx->blk_org_x - md_ctx->sb_origin_x;
        const uint16_t blk_org_y_in_sb = md_ctx->blk_org_y - md_ctx->sb_origin_y;
        scratch_luma_offset            = blk_org_x_in_sb + blk_org_y_in_sb * residual16bit->stride_y;
        scratch_cb_offset = ROUND_UV(blk_org_x_in_sb) / 2 + ROUND_UV(blk_org_y_in_sb) / 2 * residual16bit->stride_cb;
        scratch_cr_offset = ROUND_UV(blk_org_x_in_sb) / 2 + ROUND_UV(blk_org_y_in_sb) / 2 * residual16bit->stride_cr;
    } else {
        scratch_luma_offset = tx_org_x + tx_org_y * residual16bit->stride_y;
        scratch_cb_offset   = ROUND_UV(tx_org_x) / 2 + ROUND_UV(tx_org_y) / 2 * residual16bit->stride_cb;
        scratch_cr_offset   = ROUND_UV(tx_org_x) / 2 + ROUND_UV(tx_org_y) / 2 * residual16bit->stride_cr;
    }
    ed_ctx->three_quad_energy = 0;

    if (pcs->ppcs->blk_lambda_tuning) {
        md_ctx->blk_geom  = ed_ctx->blk_geom;
        md_ctx->blk_org_x = ed_ctx->blk_org_x;
        md_ctx->blk_org_y = ed_ctx->blk_org_y;
        //Get the new lambda for current block
        svt_aom_set_tuned_blk_lambda(md_ctx, pcs);
    } else if (pcs->ppcs->scs->static_config.tune == TUNE_SSIM || pcs->ppcs->scs->static_config.tune == TUNE_IQ ||
               pcs->ppcs->scs->static_config.tune == TUNE_MS_SSIM) {
        md_ctx->blk_geom  = ed_ctx->blk_geom;
        md_ctx->blk_org_x = ed_ctx->blk_org_x;
        md_ctx->blk_org_y = ed_ctx->blk_org_y;
        int mi_row        = ed_ctx->blk_org_y / 4;
        int mi_col        = ed_ctx->blk_org_x / 4;
        aom_av1_set_ssim_rdmult(md_ctx, pcs, mi_row, mi_col);
    }

    //**********************************
    // Luma
    //**********************************
    if (component_mask == PICTURE_BUFFER_DESC_FULL_MASK || component_mask == PICTURE_BUFFER_DESC_LUMA_MASK) {
        if (ed_ctx->md_skip_blk) {
            eob[0]                               = 0;
            blk_ptr->quant_dc.y[ed_ctx->txb_itr] = 0;
        } else {
            const TxSize tx_size   = tx_depth_to_tx_size[tx_depth][blk_geom->bsize];
            const int    tx_width  = tx_size_wide[tx_size];
            const int    tx_height = tx_size_high[tx_size];
            svt_aom_residual_kernel(input_samples->buffer_y,
                                    input_luma_offset,
                                    input_samples->stride_y,
                                    pred_samples->buffer_y,
                                    pred_luma_offset,
                                    pred_samples->stride_y,
                                    ((int16_t*)residual16bit->buffer_y),
                                    scratch_luma_offset,
                                    residual16bit->stride_y,
                                    is_16bit, // hbd
                                    tx_width,
                                    tx_height);
            svt_aom_estimate_transform(pcs,
                                       ed_ctx->md_ctx,
                                       ((int16_t*)residual16bit->buffer_y) + scratch_luma_offset,
                                       residual16bit->stride_y,
                                       ((TranLow*)transform16bit->buffer_y) + ed_ctx->coded_area_sb,
                                       NOT_USED_VALUE,
                                       tx_size,
                                       &ed_ctx->three_quad_energy,
                                       bit_depth,
                                       blk_ptr->tx_type[ed_ctx->txb_itr],
                                       PLANE_TYPE_Y,
                                       DEFAULT_SHAPE);

            blk_ptr->quant_dc.y[ed_ctx->txb_itr] = svt_aom_quantize_inv_quantize(
                pcs,
                md_ctx,
                ((int32_t*)transform16bit->buffer_y) + ed_ctx->coded_area_sb,
                ((int32_t*)coeff_samples_sb->buffer_y) + ed_ctx->coded_area_sb,
                ((int32_t*)inverse_quant_buffer->buffer_y) + ed_ctx->coded_area_sb,
                qindex,
                seg_qp,
                tx_size,
                &eob[0],
                COMPONENT_LUMA,
                bit_depth,
                blk_ptr->tx_type[ed_ctx->txb_itr],
                md_ctx->luma_txb_skip_context,
                md_ctx->luma_dc_sign_context,
                blk_ptr->block_mi.mode,
                md_ctx->full_lambda_md[(bit_depth == EB_TEN_BIT) ? EB_10_BIT_MD : EB_8_BIT_MD],
                true);
        }

        blk_ptr->y_has_coeff |= (eob[0] > 0) << ed_ctx->txb_itr;
        blk_ptr->eob.y[ed_ctx->txb_itr] = (uint16_t)eob[0];

        if (eob[0] == 0) {
            blk_ptr->tx_type[ed_ctx->txb_itr] = DCT_DCT;
            // INTER. Chroma follows Luma in transform type
            if (ed_ctx->txb_itr == 0 && is_inter) {
                blk_ptr->tx_type_uv = DCT_DCT;
            }
        }
    }

    if (component_mask == PICTURE_BUFFER_DESC_FULL_MASK || component_mask == PICTURE_BUFFER_DESC_CHROMA_MASK) {
        // If chroma uses CfL prediction, generate predicted samples based on previously computed recon luma
        // samples. The recon luma samples must be from a previous call to av1_encode_loop/av1_encode_generate_recon
        // because this function does not generate reconstructed samples.
        if (is_intra_mode(blk_ptr->block_mi.mode) && blk_ptr->block_mi.uv_mode == UV_CFL_PRED) {
            av1_encode_generate_cfl_prediction(
                pred_samples, ed_ctx, pred_cb_offset, pred_cr_offset, round_origin_x, round_origin_y);
        }

        //**********************************
        // Chroma
        //**********************************
        if (ed_ctx->md_skip_blk) {
            eob[1]                               = 0;
            blk_ptr->quant_dc.u[ed_ctx->txb_itr] = 0;
            eob[2]                               = 0;
            blk_ptr->quant_dc.v[ed_ctx->txb_itr] = 0;
        } else {
            const TxSize tx_size_uv   = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
            const int    tx_width_uv  = tx_size_wide[tx_size_uv];
            const int    tx_height_uv = tx_size_high[tx_size_uv];
            //**********************************
            // Cb
            //**********************************
            svt_aom_residual_kernel(input_samples->buffer_cb,
                                    input_cb_offset,
                                    input_samples->stride_cb,
                                    pred_samples->buffer_cb,
                                    pred_cb_offset,
                                    pred_samples->stride_cb,
                                    ((int16_t*)residual16bit->buffer_cb),
                                    scratch_cb_offset,
                                    residual16bit->stride_cb,
                                    is_16bit, // hbd
                                    tx_width_uv,
                                    tx_height_uv);
            svt_aom_estimate_transform(pcs,
                                       ed_ctx->md_ctx,
                                       ((int16_t*)residual16bit->buffer_cb) + scratch_cb_offset,
                                       residual16bit->stride_cb,
                                       ((TranLow*)transform16bit->buffer_cb) + ed_ctx->coded_area_sb_uv,
                                       NOT_USED_VALUE,
                                       tx_size_uv,
                                       &ed_ctx->three_quad_energy,
                                       bit_depth,
                                       blk_ptr->tx_type_uv,
                                       PLANE_TYPE_UV,
                                       DEFAULT_SHAPE);

            blk_ptr->quant_dc.u[ed_ctx->txb_itr] = svt_aom_quantize_inv_quantize(
                pcs,
                md_ctx,
                ((int32_t*)transform16bit->buffer_cb) + ed_ctx->coded_area_sb_uv,
                ((int32_t*)coeff_samples_sb->buffer_cb) + ed_ctx->coded_area_sb_uv,
                ((int32_t*)inverse_quant_buffer->buffer_cb) + ed_ctx->coded_area_sb_uv,
                qindex,
                seg_qp,
                tx_size_uv,
                &eob[1],
                COMPONENT_CHROMA_CB,
                bit_depth,
                blk_ptr->tx_type_uv,
                md_ctx->cb_txb_skip_context,
                md_ctx->cb_dc_sign_context,
                blk_ptr->block_mi.mode,
                md_ctx->full_lambda_md[(bit_depth == EB_TEN_BIT) ? EB_10_BIT_MD : EB_8_BIT_MD],
                true);

            //**********************************
            // Cr
            //**********************************
            svt_aom_residual_kernel(input_samples->buffer_cr,
                                    input_cr_offset,
                                    input_samples->stride_cr,
                                    pred_samples->buffer_cr,
                                    pred_cr_offset,
                                    pred_samples->stride_cr,
                                    ((int16_t*)residual16bit->buffer_cr),
                                    scratch_cr_offset,
                                    residual16bit->stride_cr,
                                    is_16bit, // hbd
                                    tx_width_uv,
                                    tx_height_uv);
            svt_aom_estimate_transform(pcs,
                                       ed_ctx->md_ctx,
                                       ((int16_t*)residual16bit->buffer_cr) + scratch_cb_offset,
                                       residual16bit->stride_cr,
                                       ((TranLow*)transform16bit->buffer_cr) + ed_ctx->coded_area_sb_uv,
                                       NOT_USED_VALUE,
                                       tx_size_uv,
                                       &ed_ctx->three_quad_energy,
                                       bit_depth,
                                       blk_ptr->tx_type_uv,
                                       PLANE_TYPE_UV,
                                       DEFAULT_SHAPE);

            blk_ptr->quant_dc.v[ed_ctx->txb_itr] = svt_aom_quantize_inv_quantize(
                pcs,
                md_ctx,
                ((int32_t*)transform16bit->buffer_cr) + ed_ctx->coded_area_sb_uv,
                ((int32_t*)coeff_samples_sb->buffer_cr) + ed_ctx->coded_area_sb_uv,
                ((int32_t*)inverse_quant_buffer->buffer_cr) + ed_ctx->coded_area_sb_uv,
                qindex,
                seg_qp,
                tx_size_uv,
                &eob[2],
                COMPONENT_CHROMA_CR,
                bit_depth,
                blk_ptr->tx_type_uv,
                md_ctx->cr_txb_skip_context,
                md_ctx->cr_dc_sign_context,
                blk_ptr->block_mi.mode,
                md_ctx->full_lambda_md[(bit_depth == EB_TEN_BIT) ? EB_10_BIT_MD : EB_8_BIT_MD],
                true);
        }

        blk_ptr->u_has_coeff |= (eob[1] > 0) << ed_ctx->txb_itr;
        blk_ptr->v_has_coeff |= (eob[2] > 0) << ed_ctx->txb_itr;
        blk_ptr->eob.u[ed_ctx->txb_itr] = (uint16_t)eob[1];
        blk_ptr->eob.v[ed_ctx->txb_itr] = (uint16_t)eob[2];
    }

    return;
}

/**********************************************************
* Encode Generate Recon
*
* Summary: Performs an AV1 conformant
*   Inverse Transform and generate
*   the reconstructed samples of a TU.
*
* Inputs:
*   org_x
*   org_y
*   txb_size
*   sb_sz
*   input - Inverse Quantized Coeff (position sensitive)
*   pred - prediction samples (position independent)
*
* Outputs:
*   Recon  (position independent)
*
**********************************************************/
static void av1_encode_generate_recon(PictureControlSet* pcs, EncDecContext* ed_ctx, uint32_t org_x, uint32_t org_y,
                                      EbPictureBufferDesc* pred_samples, // no basis/offset
                                      EbPictureBufferDesc* residual16bit, // no basis/offset
                                      uint32_t component_mask, uint16_t* eob) {
    BlkStruct* blk_ptr = ed_ctx->blk_ptr;

    //**********************************
    // Luma
    //**********************************
    if (component_mask & PICTURE_BUFFER_DESC_LUMA_MASK) {
        if ((blk_ptr->y_has_coeff & (1 << ed_ctx->txb_itr)) && blk_ptr->block_mi.skip_mode == false) {
            const TxSize   tx_size          = tx_depth_to_tx_size[blk_ptr->block_mi.tx_depth][ed_ctx->blk_geom->bsize];
            const uint32_t pred_luma_offset = (pred_samples->org_y + org_y) * pred_samples->stride_y +
                (pred_samples->org_x + org_x);
            svt_aom_inv_transform_recon_wrapper(pcs,
                                                ed_ctx->md_ctx,
                                                pred_samples->buffer_y,
                                                pred_luma_offset,
                                                pred_samples->stride_y,
                                                pred_samples->buffer_y,
                                                pred_luma_offset,
                                                pred_samples->stride_y,
                                                ((int32_t*)residual16bit->buffer_y),
                                                ed_ctx->coded_area_sb,
                                                ed_ctx->bit_depth == EB_TEN_BIT ? 1 : 0, // hbd
                                                tx_size,
                                                blk_ptr->tx_type[ed_ctx->txb_itr],
                                                PLANE_TYPE_Y,
                                                eob[0]);
        }
    }

    //**********************************
    // Chroma
    //**********************************
    if (component_mask & PICTURE_BUFFER_DESC_CHROMA_MASK) {
        const TxSize   tx_size_uv     = av1_get_max_uv_txsize(ed_ctx->blk_geom->bsize, 1, 1);
        const uint32_t round_origin_x = (org_x >> 3) << 3; // for Chroma blocks with size of 4
        const uint32_t round_origin_y = (org_y >> 3) << 3; // for Chroma blocks with size of 4

        //**********************************
        // Cb
        //**********************************
        if ((blk_ptr->u_has_coeff & (1 << ed_ctx->txb_itr)) && blk_ptr->block_mi.skip_mode == false) {
            const uint32_t pred_offset_cb = (((pred_samples->org_y + round_origin_y) >> 1) * pred_samples->stride_cb) +
                ((pred_samples->org_x + round_origin_x) >> 1);
            svt_aom_inv_transform_recon_wrapper(pcs,
                                                ed_ctx->md_ctx,
                                                pred_samples->buffer_cb,
                                                pred_offset_cb,
                                                pred_samples->stride_cb,
                                                pred_samples->buffer_cb,
                                                pred_offset_cb,
                                                pred_samples->stride_cb,
                                                ((int32_t*)residual16bit->buffer_cb),
                                                ed_ctx->coded_area_sb_uv,
                                                ed_ctx->bit_depth == EB_TEN_BIT ? 1 : 0, // hbd
                                                tx_size_uv,
                                                blk_ptr->tx_type_uv,
                                                PLANE_TYPE_UV,
                                                eob[1]);
        }

        //**********************************
        // Cr
        //**********************************
        if ((blk_ptr->v_has_coeff & (1 << ed_ctx->txb_itr)) && blk_ptr->block_mi.skip_mode == false) {
            const uint32_t pred_offset_cr = (((pred_samples->org_y + round_origin_y) >> 1) * pred_samples->stride_cr) +
                ((pred_samples->org_x + round_origin_x) >> 1);
            svt_aom_inv_transform_recon_wrapper(pcs,
                                                ed_ctx->md_ctx,
                                                pred_samples->buffer_cr,
                                                pred_offset_cr,
                                                pred_samples->stride_cr,
                                                pred_samples->buffer_cr,
                                                pred_offset_cr,
                                                pred_samples->stride_cr,
                                                ((int32_t*)residual16bit->buffer_cr),
                                                ed_ctx->coded_area_sb_uv,
                                                ed_ctx->bit_depth == EB_TEN_BIT ? 1 : 0, // hbd
                                                tx_size_uv,
                                                blk_ptr->tx_type_uv,
                                                PLANE_TYPE_UV,
                                                eob[2]);
        }
    }

    return;
}

void svt_aom_store16bit_input_src(EbPictureBufferDesc* input_sample16bit_buffer, PictureControlSet* pcs, uint32_t sb_x,
                                  uint32_t sb_y, uint32_t sb_w, uint32_t sb_h) {
    uint32_t  row_it;
    uint16_t* from_ptr;
    uint16_t* to_ptr;

    from_ptr = (uint16_t*)input_sample16bit_buffer->buffer_y;
    to_ptr   = (uint16_t*)pcs->input_frame16bit->buffer_y + (sb_x + pcs->input_frame16bit->org_x) +
        (sb_y + pcs->input_frame16bit->org_y) * pcs->input_frame16bit->stride_y;

    for (row_it = 0; row_it < sb_h; row_it++) {
        svt_memcpy(to_ptr + row_it * pcs->input_frame16bit->stride_y,
                   from_ptr + row_it * input_sample16bit_buffer->stride_y,
                   sb_w * 2);
    }

    sb_x = sb_x / 2;
    sb_y = sb_y / 2;
    sb_w = sb_w / 2;
    sb_h = sb_h / 2;

    from_ptr = (uint16_t*)input_sample16bit_buffer->buffer_cb;
    to_ptr   = (uint16_t*)pcs->input_frame16bit->buffer_cb + (sb_x + pcs->input_frame16bit->org_x / 2) +
        (sb_y + pcs->input_frame16bit->org_y / 2) * pcs->input_frame16bit->stride_cb;

    for (row_it = 0; row_it < sb_h; row_it++) {
        svt_memcpy(to_ptr + row_it * pcs->input_frame16bit->stride_cb,
                   from_ptr + row_it * input_sample16bit_buffer->stride_cb,
                   sb_w * 2);
    }

    from_ptr = (uint16_t*)input_sample16bit_buffer->buffer_cr;
    to_ptr   = (uint16_t*)pcs->input_frame16bit->buffer_cr + (sb_x + pcs->input_frame16bit->org_x / 2) +
        (sb_y + pcs->input_frame16bit->org_y / 2) * pcs->input_frame16bit->stride_cb;

    for (row_it = 0; row_it < sb_h; row_it++) {
        svt_memcpy(to_ptr + row_it * pcs->input_frame16bit->stride_cr,
                   from_ptr + row_it * input_sample16bit_buffer->stride_cr,
                   sb_w * 2);
    }
}

void svt_aom_update_mi_map_enc_dec(BlkStruct* blk_ptr, ModeDecisionContext* ctx, PictureControlSet* pcs);

static void perform_intra_coding_loop(PictureControlSet* pcs, EncDecContext* ed_ctx) {
    BlkStruct*           blk_ptr  = ed_ctx->blk_ptr;
    bool                 is_16bit = ed_ctx->is_16bit;
    uint8_t              is_inter = 0; // set to 0 b/c this is the intra path
    EbPictureBufferDesc* recon_buffer;
    EbPictureBufferDesc* coeff_buffer_sb  = pcs->ppcs->enc_dec_ptr->quantized_coeff[ed_ctx->sb_index];
    uint16_t             tile_idx         = ed_ctx->tile_index;
    NeighborArrayUnit*   ep_luma_recon_na = is_16bit ? pcs->ep_luma_recon_na_16bit[tile_idx]
                                                     : pcs->ep_luma_recon_na[tile_idx];
    NeighborArrayUnit* ep_cb_recon_na = is_16bit ? pcs->ep_cb_recon_na_16bit[tile_idx] : pcs->ep_cb_recon_na[tile_idx];
    NeighborArrayUnit* ep_cr_recon_na = is_16bit ? pcs->ep_cr_recon_na_16bit[tile_idx] : pcs->ep_cr_recon_na[tile_idx];

    // temp buffers for performing the transform/generating the recon
    EbPictureBufferDesc* residual_buffer      = ed_ctx->md_ctx->temp_residual;
    EbPictureBufferDesc* transform_buffer     = ed_ctx->md_ctx->tx_coeffs;
    EbPictureBufferDesc* inverse_quant_buffer = ed_ctx->md_ctx->cand_bf_ptr_array[0]->rec_coeff;

    blk_ptr->y_has_coeff = 0;
    blk_ptr->u_has_coeff = 0;
    blk_ptr->v_has_coeff = 0;
    uint16_t eobs[MAX_TXB_COUNT][3];
    svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);
    const uint8_t  tx_depth       = blk_ptr->block_mi.tx_depth;
    const TxSize   tx_size        = tx_depth_to_tx_size[tx_depth][ed_ctx->blk_geom->bsize];
    const int      tx_width       = tx_size_wide[tx_size];
    const int      tx_height      = tx_size_high[tx_size];
    const TxSize   tx_size_uv     = av1_get_max_uv_txsize(ed_ctx->blk_geom->bsize, 1, 1);
    const int      tx_width_uv    = tx_size_wide[tx_size_uv];
    const int      tx_height_uv   = tx_size_high[tx_size_uv];
    const uint32_t tot_tu         = tx_blocks_per_depth[ed_ctx->blk_geom->bsize][tx_depth];
    const uint32_t sb_size_luma   = pcs->ppcs->scs->sb_size;
    const uint32_t sb_size_chroma = pcs->ppcs->scs->sb_size >> 1;

    // Luma path
    for (ed_ctx->txb_itr = 0; ed_ctx->txb_itr < tot_tu; ed_ctx->txb_itr++) {
        const uint16_t txb_origin_x = ed_ctx->blk_org_x +
            tx_org[ed_ctx->blk_geom->bsize][is_inter][tx_depth][ed_ctx->txb_itr].x;
        const uint16_t txb_origin_y = ed_ctx->blk_org_y +
            tx_org[ed_ctx->blk_geom->bsize][is_inter][tx_depth][ed_ctx->txb_itr].y;
        ed_ctx->md_ctx->luma_txb_skip_context = 0;
        ed_ctx->md_ctx->luma_dc_sign_context  = 0;
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_LUMA,
                            pcs->ep_luma_dc_sign_level_coeff_na[tile_idx],
                            txb_origin_x,
                            txb_origin_y,
                            ed_ctx->blk_geom->bsize,
                            tx_size,
                            &ed_ctx->md_ctx->luma_txb_skip_context,
                            &ed_ctx->md_ctx->luma_dc_sign_context);

        // Copy neighbour arrays for intra prediction
        const PredictionMode mode       = blk_ptr->block_mi.mode;
        const int            ang        = blk_ptr->block_mi.angle_delta[PLANE_TYPE_Y];
        const IntraSize      intra_size = ang == 0 ? svt_aom_intra_unit[mode] : (IntraSize){2, 2};
        uint8_t              top_neigh_array[(64 * 2 + 1) << 1];
        uint8_t              left_neigh_array[(64 * 2 + 1) << 1];
        if (txb_origin_y != 0) {
            svt_memcpy(top_neigh_array + ((uint64_t)1 << is_16bit),
                       ep_luma_recon_na->top_array + (txb_origin_x << is_16bit),
                       (tx_width * intra_size.top) << is_16bit);
        }

        if (txb_origin_x != 0) {
            uint16_t multipler = (txb_origin_y % sb_size_luma + tx_height * intra_size.left) > sb_size_luma
                ? 1
                : intra_size.left;
            svt_memcpy(left_neigh_array + ((uint64_t)1 << is_16bit),
                       ep_luma_recon_na->left_array + (txb_origin_y << is_16bit),
                       (tx_height * multipler) << is_16bit);
        }

        if (txb_origin_y != 0 && txb_origin_x != 0) {
            if (is_16bit) {
                uint16_t* top_hbd  = (uint16_t*)top_neigh_array;
                uint16_t* left_hbd = (uint16_t*)left_neigh_array;
                top_hbd[0] = left_hbd[0] = ((uint16_t*)(ep_luma_recon_na->top_left_array) +
                                            ep_luma_recon_na->max_pic_h + txb_origin_x - txb_origin_y)[0];

            } else {
                top_neigh_array[0] = left_neigh_array[0] =
                    ep_luma_recon_na->top_left_array[ep_luma_recon_na->max_pic_h + txb_origin_x - txb_origin_y];
            }
        }

        svt_av1_predict_intra_block(blk_ptr->av1xd,
                                    ed_ctx->blk_geom->bsize,
                                    tx_size,
                                    mode,
                                    blk_ptr->block_mi.angle_delta[PLANE_TYPE_Y],
                                    blk_ptr->palette_size[0] > 0,
                                    blk_ptr->palette_info,
                                    blk_ptr->block_mi.filter_intra_mode,
                                    top_neigh_array + ((uint64_t)1 << is_16bit),
                                    left_neigh_array + ((uint64_t)1 << is_16bit),
                                    recon_buffer,
                                    (tx_org[ed_ctx->blk_geom->bsize][is_inter][tx_depth][ed_ctx->txb_itr].x) >> 2,
                                    (tx_org[ed_ctx->blk_geom->bsize][is_inter][tx_depth][ed_ctx->txb_itr].y) >> 2,
                                    AOM_PLANE_Y,
                                    ed_ctx->md_ctx->shape,
                                    txb_origin_x,
                                    txb_origin_y,
                                    &pcs->scs->seq_header,
                                    ed_ctx->bit_depth);

        // Encode Transform Unit -INTRA-
        av1_encode_loop(pcs,
                        ed_ctx,
                        txb_origin_x,
                        txb_origin_y,
                        recon_buffer,
                        coeff_buffer_sb,
                        residual_buffer,
                        transform_buffer,
                        inverse_quant_buffer,
                        PICTURE_BUFFER_DESC_LUMA_MASK,
                        eobs[ed_ctx->txb_itr]);
        av1_encode_generate_recon(pcs,
                                  ed_ctx,
                                  txb_origin_x,
                                  txb_origin_y,
                                  recon_buffer,
                                  inverse_quant_buffer,
                                  PICTURE_BUFFER_DESC_LUMA_MASK,
                                  eobs[ed_ctx->txb_itr]);

        // Update Recon Samples-INTRA-
        encode_pass_update_recon_sample_neighbour_arrays(ep_luma_recon_na,
                                                         ep_cb_recon_na,
                                                         ep_cr_recon_na,
                                                         recon_buffer,
                                                         txb_origin_x,
                                                         txb_origin_y,
                                                         tx_width,
                                                         tx_height,
                                                         tx_width_uv,
                                                         tx_height_uv,
                                                         PICTURE_BUFFER_DESC_LUMA_MASK,
                                                         is_16bit);

        ed_ctx->coded_area_sb += tx_width * tx_height;

        // Update the luma Dc Sign Level Coeff Neighbor Array
        {
            uint8_t dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.y[ed_ctx->txb_itr];
            svt_aom_neighbor_array_unit_mode_write(pcs->ep_luma_dc_sign_level_coeff_na[tile_idx],
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   txb_origin_x,
                                                   txb_origin_y,
                                                   tx_width,
                                                   tx_height,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        }
    } // Transform Loop

    // Chroma path

    if (ed_ctx->md_ctx->has_uv) {
        ed_ctx->txb_itr       = 0;
        uint16_t txb_origin_x = ed_ctx->blk_org_x +
            tx_org[ed_ctx->blk_geom->bsize][is_inter][tx_depth][ed_ctx->txb_itr].x;
        uint16_t txb_origin_y = ed_ctx->blk_org_y +
            tx_org[ed_ctx->blk_geom->bsize][is_inter][tx_depth][ed_ctx->txb_itr].y;
        uint32_t blk_originx_uv = (ed_ctx->blk_org_x >> 3 << 3) >> 1;
        uint32_t blk_originy_uv = (ed_ctx->blk_org_y >> 3 << 3) >> 1;

        ed_ctx->md_ctx->cb_txb_skip_context = 0;
        ed_ctx->md_ctx->cb_dc_sign_context  = 0;
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_CHROMA,
                            pcs->ep_cb_dc_sign_level_coeff_na[tile_idx],
                            blk_originx_uv,
                            blk_originy_uv,
                            ed_ctx->blk_geom->bsize_uv,
                            tx_size_uv,
                            &ed_ctx->md_ctx->cb_txb_skip_context,
                            &ed_ctx->md_ctx->cb_dc_sign_context);

        ed_ctx->md_ctx->cr_txb_skip_context = 0;
        ed_ctx->md_ctx->cr_dc_sign_context  = 0;
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_CHROMA,
                            pcs->ep_cr_dc_sign_level_coeff_na[tile_idx],
                            blk_originx_uv,
                            blk_originy_uv,
                            ed_ctx->blk_geom->bsize_uv,
                            tx_size_uv,
                            &ed_ctx->md_ctx->cr_txb_skip_context,
                            &ed_ctx->md_ctx->cr_dc_sign_context);

        // Generate prediction for both chroma planes
        for (Plane plane = AOM_PLANE_U; plane <= AOM_PLANE_V; ++plane) {
            uint8_t top_neigh_array[(64 * 2 + 1) << 1];
            uint8_t left_neigh_array[(64 * 2 + 1) << 1];

            // Copy neighbour arrays for intra prediction
            const PredictionMode mode              = (blk_ptr->block_mi.uv_mode == UV_CFL_PRED)
                             ? (PredictionMode)UV_DC_PRED
                             : (PredictionMode)blk_ptr->block_mi.uv_mode;
            const int            ang               = blk_ptr->block_mi.angle_delta[PLANE_TYPE_UV];
            const IntraSize      intra_size        = ang == 0 ? svt_aom_intra_unit[mode] : (IntraSize){2, 2};
            NeighborArrayUnit*   eb_uv_neigh_array = plane == 1 ? ep_cb_recon_na : ep_cr_recon_na;
            if (blk_originy_uv != 0) {
                svt_memcpy(top_neigh_array + ((uint64_t)1 << is_16bit),
                           eb_uv_neigh_array->top_array + (blk_originx_uv << is_16bit),
                           (ed_ctx->blk_geom->bwidth_uv * intra_size.top) << is_16bit);
            }

            if (blk_originx_uv != 0) {
                uint16_t multipler = (blk_originy_uv % sb_size_chroma +
                                      ed_ctx->blk_geom->bheight_uv * intra_size.left) > sb_size_chroma
                    ? 1
                    : intra_size.left;
                svt_memcpy(left_neigh_array + ((uint64_t)1 << is_16bit),
                           eb_uv_neigh_array->left_array + (blk_originy_uv << is_16bit),
                           (ed_ctx->blk_geom->bheight_uv * multipler) << is_16bit);
            }

            if (blk_originy_uv != 0 && blk_originx_uv != 0) {
                if (is_16bit) {
                    uint16_t* top_hbd  = (uint16_t*)top_neigh_array;
                    uint16_t* left_hbd = (uint16_t*)left_neigh_array;
                    top_hbd[0] = left_hbd[0] = ((uint16_t*)(eb_uv_neigh_array->top_left_array) +
                                                eb_uv_neigh_array->max_pic_h + blk_originx_uv - blk_originy_uv)[0];
                } else {
                    top_neigh_array[0] = left_neigh_array[0] =
                        eb_uv_neigh_array
                            ->top_left_array[eb_uv_neigh_array->max_pic_h + blk_originx_uv - blk_originy_uv];
                }
            }

            svt_av1_predict_intra_block(blk_ptr->av1xd,
                                        ed_ctx->blk_geom->bsize,
                                        tx_size_uv,
                                        mode,
                                        blk_ptr->block_mi.angle_delta[PLANE_TYPE_UV],
                                        0, //chroma
                                        blk_ptr->palette_info,
                                        FILTER_INTRA_MODES,
                                        top_neigh_array + ((uint64_t)1 << is_16bit),
                                        left_neigh_array + ((uint64_t)1 << is_16bit),
                                        recon_buffer,
                                        0,
                                        0,
                                        plane,
                                        ed_ctx->md_ctx->shape,
                                        plane ? ROUND_UV(ed_ctx->blk_org_x) >> 1 : txb_origin_x,
                                        plane ? ROUND_UV(ed_ctx->blk_org_y) >> 1 : txb_origin_y,
                                        &pcs->scs->seq_header,
                                        ed_ctx->bit_depth);
        }

        // Encode Transform Unit -INTRA-
        av1_encode_loop(pcs,
                        ed_ctx,
                        txb_origin_x,
                        txb_origin_y,
                        recon_buffer,
                        coeff_buffer_sb,
                        residual_buffer,
                        transform_buffer,
                        inverse_quant_buffer,
                        PICTURE_BUFFER_DESC_CHROMA_MASK,
                        eobs[ed_ctx->txb_itr]);
        av1_encode_generate_recon(pcs,
                                  ed_ctx,
                                  txb_origin_x,
                                  txb_origin_y,
                                  recon_buffer,
                                  inverse_quant_buffer,
                                  PICTURE_BUFFER_DESC_CHROMA_MASK,
                                  eobs[ed_ctx->txb_itr]);

        // Update Recon Samples-INTRA-
        encode_pass_update_recon_sample_neighbour_arrays(ep_luma_recon_na,
                                                         ep_cb_recon_na,
                                                         ep_cr_recon_na,
                                                         recon_buffer,
                                                         txb_origin_x,
                                                         txb_origin_y,
                                                         tx_width,
                                                         tx_height,
                                                         tx_width_uv,
                                                         tx_height_uv,
                                                         PICTURE_BUFFER_DESC_CHROMA_MASK,
                                                         is_16bit);

        ed_ctx->coded_area_sb_uv += tx_width_uv * tx_height_uv;

        // Update the cb Dc Sign Level Coeff Neighbor Array
        {
            uint8_t dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.u[ed_ctx->txb_itr];
            svt_aom_neighbor_array_unit_mode_write(pcs->ep_cb_dc_sign_level_coeff_na[tile_idx],
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   ROUND_UV(txb_origin_x) >> 1,
                                                   ROUND_UV(txb_origin_y) >> 1,
                                                   tx_width_uv,
                                                   tx_height_uv,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        }

        // Update the cr DC Sign Level Coeff Neighbor Array
        {
            uint8_t dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.v[ed_ctx->txb_itr];
            svt_aom_neighbor_array_unit_mode_write(pcs->ep_cr_dc_sign_level_coeff_na[tile_idx],
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   ROUND_UV(txb_origin_x) >> 1,
                                                   ROUND_UV(txb_origin_y) >> 1,
                                                   tx_width_uv,
                                                   tx_height_uv,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        }
    } // Transform Loop
    assert(IMPLIES(!ed_ctx->md_ctx->has_uv, blk_ptr->u_has_coeff == 0 && blk_ptr->v_has_coeff == 0));
    blk_ptr->block_has_coeff = (blk_ptr->y_has_coeff || blk_ptr->u_has_coeff || blk_ptr->v_has_coeff);
}

#define REFMVS_LIMIT ((1 << 12) - 1)

static void av1_copy_frame_mvs(PictureControlSet* pcs, const Av1Common* const cm, MbModeInfo mi, int mi_row, int mi_col,
                               int x_mis, int y_mis, EbReferenceObject* object_ptr) {
    const int frame_mvs_stride = ROUND_POWER_OF_TWO(cm->mi_cols, 1);
    MV_REF*   frame_mvs        = object_ptr->mvs + (mi_row >> 1) * frame_mvs_stride + (mi_col >> 1);
    x_mis                      = ROUND_POWER_OF_TWO(x_mis, 1);
    y_mis                      = ROUND_POWER_OF_TWO(y_mis, 1);
    int w, h;

    for (h = 0; h < y_mis; h++) {
        MV_REF* mv = frame_mvs;
        for (w = 0; w < x_mis; w++) {
            mv->ref_frame = NONE_FRAME;
            mv->mv.as_int = 0;

            for (int idx = 0; idx < 2; ++idx) {
                MvReferenceFrame ref_frame = mi.block_mi.ref_frame[idx];
                if (ref_frame > INTRA_FRAME) {
                    int8_t ref_idx = pcs->ref_frame_side[ref_frame];
                    if (ref_idx) {
                        continue;
                    }
                    if ((abs(mi.block_mi.mv[idx].y) > REFMVS_LIMIT) || (abs(mi.block_mi.mv[idx].x) > REFMVS_LIMIT)) {
                        continue;
                    }
                    mv->ref_frame = ref_frame;
                    mv->mv.as_int = mi.block_mi.mv[idx].as_int;
                }
            }
            mv++;
        }
        frame_mvs += frame_mvs_stride;
    }
}

/*
 * Convert the recon picture from 16bit to 8bit.  Recon pic is passed through the pcs.
 */
void svt_aom_convert_recon_16bit_to_8bit(PictureControlSet* pcs, EncDecContext* ctx) {
    EbPictureBufferDesc* recon_buffer_16bit;
    EbPictureBufferDesc* recon_buffer_8bit;
    svt_aom_get_recon_pic(pcs, &recon_buffer_16bit, 1);
    if (pcs->ppcs->is_ref == true) {
        // get the 16bit form of the input SB
        recon_buffer_8bit = ((EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr)->reference_picture;
    } else { // non ref pictures
        recon_buffer_8bit = pcs->ppcs->enc_dec_ptr->recon_pic;
    }

    uint32_t pred_buf_x_offest = ctx->blk_org_x;
    uint32_t pred_buf_y_offest = ctx->blk_org_y;

    uint16_t* dst_16bit = (uint16_t*)(recon_buffer_16bit->buffer_y) + pred_buf_x_offest + recon_buffer_16bit->org_x +
        (pred_buf_y_offest + recon_buffer_16bit->org_y) * recon_buffer_16bit->stride_y;
    int32_t dst_stride_16bit = recon_buffer_16bit->stride_y;

    uint8_t* dst;
    int32_t  dst_stride;

    dst = recon_buffer_8bit->buffer_y + pred_buf_x_offest + recon_buffer_8bit->org_x +
        (pred_buf_y_offest + recon_buffer_8bit->org_y) * recon_buffer_8bit->stride_y;
    dst_stride = recon_buffer_8bit->stride_y;

    svt_convert_16bit_to_8bit(
        dst_16bit, dst_stride_16bit, dst, dst_stride, ctx->blk_geom->bwidth, ctx->blk_geom->bheight);

    //copy recon from 16bit to 8bit
    pred_buf_x_offest = ((ctx->blk_org_x >> 3) << 3) >> 1;
    pred_buf_y_offest = ((ctx->blk_org_y >> 3) << 3) >> 1;

    dst_16bit = (uint16_t*)(recon_buffer_16bit->buffer_cb) + pred_buf_x_offest + recon_buffer_16bit->org_x / 2 +
        (pred_buf_y_offest + recon_buffer_16bit->org_y / 2) * recon_buffer_16bit->stride_cb;
    dst_stride_16bit = recon_buffer_16bit->stride_cb;

    dst = recon_buffer_8bit->buffer_cb + pred_buf_x_offest + recon_buffer_8bit->org_x / 2 +
        (pred_buf_y_offest + recon_buffer_8bit->org_y / 2) * recon_buffer_8bit->stride_cb;
    dst_stride = recon_buffer_8bit->stride_cb;

    svt_convert_16bit_to_8bit(
        dst_16bit, dst_stride_16bit, dst, dst_stride, ctx->blk_geom->bwidth_uv, ctx->blk_geom->bheight_uv);

    dst_16bit = (uint16_t*)(recon_buffer_16bit->buffer_cr) +
        (pred_buf_x_offest + recon_buffer_16bit->org_x / 2 +
         (pred_buf_y_offest + recon_buffer_16bit->org_y / 2) * recon_buffer_16bit->stride_cr);
    dst_stride_16bit = recon_buffer_16bit->stride_cr;
    dst              = recon_buffer_8bit->buffer_cr + pred_buf_x_offest + recon_buffer_8bit->org_x / 2 +
        (pred_buf_y_offest + recon_buffer_8bit->org_y / 2) * recon_buffer_8bit->stride_cr;
    dst_stride = recon_buffer_8bit->stride_cr;

    svt_convert_16bit_to_8bit(
        dst_16bit, dst_stride_16bit, dst, dst_stride, ctx->blk_geom->bwidth_uv, ctx->blk_geom->bheight_uv);
}

/*
 * Inter coding loop for EncDec process.
 *
 * For the given mode info, perform inter prediction, transform and recon.
 * Update relevant neighbour arrays.
 */
static void perform_inter_coding_loop(PictureControlSet* pcs, EncDecContext* ctx) {
    SequenceControlSet* scs      = pcs->scs;
    const BlockGeom*    blk_geom = ctx->blk_geom;
    BlkStruct*          blk_ptr  = ctx->blk_ptr;

    // temp buffers for performing the transform/generating the recon
    EbPictureBufferDesc* residual_buffer      = ctx->md_ctx->temp_residual;
    EbPictureBufferDesc* transform_buffer     = ctx->md_ctx->tx_coeffs;
    EbPictureBufferDesc* inverse_quant_buffer = ctx->md_ctx->cand_bf_ptr_array[0]->rec_coeff;

    bool                 is_16bit = ctx->is_16bit;
    EbPictureBufferDesc* recon_buffer;
    EbPictureBufferDesc* coeff_buffer_sb = pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index];
    ModeDecisionContext* md_ctx          = ctx->md_ctx;
    const int            is_inter        = is_inter_block(&blk_ptr->block_mi);
    assert(is_inter);

    // Dereferencing early
    uint16_t tile_idx = ctx->tile_index;

    NeighborArrayUnit* ep_luma_recon_na = is_16bit ? pcs->ep_luma_recon_na_16bit[tile_idx]
                                                   : pcs->ep_luma_recon_na[tile_idx];
    NeighborArrayUnit* ep_cb_recon_na = is_16bit ? pcs->ep_cb_recon_na_16bit[tile_idx] : pcs->ep_cb_recon_na[tile_idx];
    NeighborArrayUnit* ep_cr_recon_na = is_16bit ? pcs->ep_cr_recon_na_16bit[tile_idx] : pcs->ep_cr_recon_na[tile_idx];

    svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);

    // Inter Prediction
    EbPictureBufferDesc* ref_pic_list0;
    EbPictureBufferDesc* ref_pic_list1;
    if (blk_ptr->block_mi.use_intrabc) {
        svt_aom_get_recon_pic(pcs, &ref_pic_list0, is_16bit);
        ref_pic_list1 = (EbPictureBufferDesc*)NULL;
    } else {
        ref_pic_list0 = svt_aom_get_ref_pic_buffer(pcs, blk_ptr->block_mi.ref_frame[0]);
        ref_pic_list1 = svt_aom_get_ref_pic_buffer(pcs, blk_ptr->block_mi.ref_frame[1]);
    }

    svt_aom_inter_prediction(scs,
                             pcs,
                             &blk_ptr->block_mi,
                             &md_ctx->blk_ptr->wm_params_l0,
                             &md_ctx->blk_ptr->wm_params_l1,
                             blk_ptr,
                             blk_geom->bsize,
                             ctx->md_ctx->shape,
                             false, //use_precomputed_obmc,
                             false, //use_precomputed_ii
                             NULL, // md_ctx - only needed for precompute obmc/ii
                             ep_luma_recon_na,
                             ep_cb_recon_na,
                             ep_cr_recon_na,
                             ref_pic_list0,
                             ref_pic_list1,
                             ctx->blk_org_x,
                             ctx->blk_org_y,
                             recon_buffer,
                             ctx->blk_org_x,
                             ctx->blk_org_y,
                             md_ctx->has_uv ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
                             (uint8_t)scs->static_config.encoder_bit_depth,
                             is_16bit);

    // Transform Loop
    blk_ptr->y_has_coeff = 0;
    blk_ptr->u_has_coeff = 0;
    blk_ptr->v_has_coeff = 0;

    // Initialize the Transform Loop
    uint16_t       eobs[MAX_TXB_COUNT][3];
    const uint8_t  tx_depth     = blk_ptr->block_mi.tx_depth;
    const uint16_t tot_tu       = tx_blocks_per_depth[blk_geom->bsize][tx_depth];
    const TxSize   tx_size      = tx_depth_to_tx_size[tx_depth][blk_geom->bsize];
    const int      tx_width     = tx_size_wide[tx_size];
    const int      tx_height    = tx_size_high[tx_size];
    const TxSize   tx_size_uv   = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
    const int      tx_width_uv  = tx_size_wide[tx_size_uv];
    const int      tx_height_uv = tx_size_high[tx_size_uv];

    for (ctx->txb_itr = 0; ctx->txb_itr < tot_tu; ctx->txb_itr++) {
        const uint8_t  uv_pass        = tx_depth && ctx->txb_itr ? 0 : 1; //NM: 128x128 exeption
        const uint16_t txb_origin_x   = ctx->blk_org_x + tx_org[blk_geom->bsize][is_inter][tx_depth][ctx->txb_itr].x;
        const uint16_t txb_origin_y   = ctx->blk_org_y + tx_org[blk_geom->bsize][is_inter][tx_depth][ctx->txb_itr].y;
        md_ctx->luma_txb_skip_context = 0;
        md_ctx->luma_dc_sign_context  = 0;
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_LUMA,
                            pcs->ep_luma_dc_sign_level_coeff_na[tile_idx],
                            txb_origin_x,
                            txb_origin_y,
                            blk_geom->bsize,
                            tx_size,
                            &md_ctx->luma_txb_skip_context,
                            &md_ctx->luma_dc_sign_context);

        if (md_ctx->has_uv && uv_pass) {
            md_ctx->cb_txb_skip_context = 0;
            md_ctx->cb_dc_sign_context  = 0;
            svt_aom_get_txb_ctx(pcs,
                                COMPONENT_CHROMA,
                                pcs->ep_cb_dc_sign_level_coeff_na[tile_idx],
                                ROUND_UV(txb_origin_x) >> 1,
                                ROUND_UV(txb_origin_y) >> 1,
                                blk_geom->bsize_uv,
                                tx_size_uv,
                                &md_ctx->cb_txb_skip_context,
                                &md_ctx->cb_dc_sign_context);

            md_ctx->cr_txb_skip_context = 0;
            md_ctx->cr_dc_sign_context  = 0;
            svt_aom_get_txb_ctx(pcs,
                                COMPONENT_CHROMA,
                                pcs->ep_cr_dc_sign_level_coeff_na[tile_idx],
                                ROUND_UV(txb_origin_x) >> 1,
                                ROUND_UV(txb_origin_y) >> 1,
                                blk_geom->bsize_uv,
                                tx_size_uv,
                                &md_ctx->cr_txb_skip_context,
                                &md_ctx->cr_dc_sign_context);
        }
        if (blk_ptr->block_mi.skip_mode == true) {
            blk_ptr->y_has_coeff = 0;
            blk_ptr->u_has_coeff = 0;
            blk_ptr->v_has_coeff = 0;

            blk_ptr->quant_dc.y[ctx->txb_itr] = 0;
            blk_ptr->quant_dc.u[ctx->txb_itr] = 0;
            blk_ptr->quant_dc.v[ctx->txb_itr] = 0;
        } else {
            //inter mode  2
            av1_encode_loop(pcs,
                            ctx,
                            txb_origin_x, //pic offset
                            txb_origin_y,
                            recon_buffer,
                            coeff_buffer_sb,
                            residual_buffer,
                            transform_buffer,
                            inverse_quant_buffer,
                            md_ctx->has_uv && uv_pass ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
                            eobs[ctx->txb_itr]);
        }

        //inter mode
        av1_encode_generate_recon(
            pcs,
            ctx,
            txb_origin_x, //pic offset
            txb_origin_y,
            recon_buffer,
            inverse_quant_buffer,
            md_ctx->has_uv && uv_pass ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
            eobs[ctx->txb_itr]);

        ctx->coded_area_sb += tx_width * tx_height;

        if (md_ctx->has_uv && uv_pass) {
            ctx->coded_area_sb_uv += tx_width_uv * tx_height_uv;
        }

        // Update the luma Dc Sign Level Coeff Neighbor Array
        uint8_t dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.y[ctx->txb_itr];

        svt_aom_neighbor_array_unit_mode_write(pcs->ep_luma_dc_sign_level_coeff_na[tile_idx],
                                               (uint8_t*)&dc_sign_level_coeff,
                                               txb_origin_x,
                                               txb_origin_y,
                                               tx_width,
                                               tx_height,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

        // Update the cb Dc Sign Level Coeff Neighbor Array
        if (md_ctx->has_uv && uv_pass) {
            dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.u[ctx->txb_itr];

            svt_aom_neighbor_array_unit_mode_write(pcs->ep_cb_dc_sign_level_coeff_na[tile_idx],
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   ROUND_UV(txb_origin_x) >> 1,
                                                   ROUND_UV(txb_origin_y) >> 1,
                                                   tx_width_uv,
                                                   tx_height_uv,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
            // Update the cr DC Sign Level Coeff Neighbor Array
            dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.v[ctx->txb_itr];

            svt_aom_neighbor_array_unit_mode_write(pcs->ep_cr_dc_sign_level_coeff_na[tile_idx],
                                                   (uint8_t*)&dc_sign_level_coeff,
                                                   ROUND_UV(txb_origin_x) >> 1,
                                                   ROUND_UV(txb_origin_y) >> 1,
                                                   tx_width_uv,
                                                   tx_height_uv,
                                                   NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
        }

    } // Transform Loop

    assert(IMPLIES(!md_ctx->has_uv, blk_ptr->u_has_coeff == 0 && blk_ptr->v_has_coeff == 0));
    blk_ptr->block_has_coeff = (blk_ptr->y_has_coeff || blk_ptr->u_has_coeff || blk_ptr->v_has_coeff);

    // Update Recon Samples Neighbor Arrays -INTER-
    encode_pass_update_recon_sample_neighbour_arrays(
        ep_luma_recon_na,
        ep_cb_recon_na,
        ep_cr_recon_na,
        recon_buffer,
        ctx->blk_org_x,
        ctx->blk_org_y,
        ctx->blk_geom->bwidth,
        ctx->blk_geom->bheight,
        ctx->blk_geom->bwidth_uv,
        ctx->blk_geom->bheight_uv,
        md_ctx->has_uv ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
        is_16bit);
}

// Copy recon to EncDec buffers if EncDec was bypassed. If pred depth only was used and NSQ is OFF data
// was copied directly to EncDec buffers in MD.
static void copy_recon(PictureControlSet* pcs, ModeDecisionContext* ctx, BlkStruct* blk_ptr) {
    const bool           is_16bit = ctx->ed_ctx->is_16bit;
    EbPictureBufferDesc* recon_buffer;
    svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);
    if (ctx->encoder_bit_depth > EB_EIGHT_BIT) {
        uint32_t recon_luma_offset = (recon_buffer->org_y + ctx->blk_org_y) * recon_buffer->stride_y +
            (recon_buffer->org_x + ctx->blk_org_x);
        uint16_t* ep_recon = ((uint16_t*)(recon_buffer->buffer_y)) + recon_luma_offset;
        uint16_t* md_recon = (uint16_t*)(blk_ptr->recon_tmp->buffer_y);

        for (uint32_t i = 0; i < ctx->blk_geom->bheight; i++) {
            svt_memcpy(ep_recon + i * recon_buffer->stride_y,
                       md_recon + i * blk_ptr->recon_tmp->stride_y,
                       ctx->blk_geom->bwidth * sizeof(uint16_t));
        }

        if (ctx->has_uv) {
            uint32_t round_origin_x = (ctx->blk_org_x >> 3) << 3; // for Chroma blocks with size of 4
            uint32_t round_origin_y = (ctx->blk_org_y >> 3) << 3; // for Chroma blocks with size of 4

            // Cr
            uint32_t recon_cr_offset = (((recon_buffer->org_y + round_origin_y) >> 1) * recon_buffer->stride_cr) +
                ((recon_buffer->org_x + round_origin_x) >> 1);
            uint16_t* ep_recon_cr = ((uint16_t*)(recon_buffer->buffer_cr)) + recon_cr_offset;
            uint16_t* md_recon_cr = (uint16_t*)(blk_ptr->recon_tmp->buffer_cr);

            for (uint32_t i = 0; i < ctx->blk_geom->bheight_uv; i++) {
                svt_memcpy(ep_recon_cr + i * recon_buffer->stride_cr,
                           md_recon_cr + i * blk_ptr->recon_tmp->stride_cr,
                           ctx->blk_geom->bwidth_uv * sizeof(uint16_t));
            }

            // Cb
            uint32_t recon_cb_offset = (((recon_buffer->org_y + round_origin_y) >> 1) * recon_buffer->stride_cb) +
                ((recon_buffer->org_x + round_origin_x) >> 1);
            uint16_t* ep_recon_cb = ((uint16_t*)(recon_buffer->buffer_cb)) + recon_cb_offset;
            uint16_t* md_recon_cb = (uint16_t*)(blk_ptr->recon_tmp->buffer_cb);

            for (uint32_t i = 0; i < ctx->blk_geom->bheight_uv; i++) {
                svt_memcpy(ep_recon_cb + i * recon_buffer->stride_cb,
                           md_recon_cb + i * blk_ptr->recon_tmp->stride_cb,
                           ctx->blk_geom->bwidth_uv * sizeof(uint16_t));
            }
        }
    } else {
        uint32_t recon_luma_offset = (recon_buffer->org_y + ctx->blk_org_y) * recon_buffer->stride_y +
            (recon_buffer->org_x + ctx->blk_org_x);
        uint8_t* ep_recon = recon_buffer->buffer_y + recon_luma_offset;
        uint8_t* md_recon = blk_ptr->recon_tmp->buffer_y;

        for (uint32_t i = 0; i < ctx->blk_geom->bheight; i++) {
            svt_memcpy(ep_recon + i * recon_buffer->stride_y,
                       md_recon + i * blk_ptr->recon_tmp->stride_y,
                       ctx->blk_geom->bwidth * sizeof(uint8_t));
        }

        if (ctx->has_uv) {
            uint32_t round_origin_x = (ctx->blk_org_x >> 3) << 3; // for Chroma blocks with size of 4
            uint32_t round_origin_y = (ctx->blk_org_y >> 3) << 3; // for Chroma blocks with size of 4

            // Cr
            uint32_t recon_cr_offset = (((recon_buffer->org_y + round_origin_y) >> 1) * recon_buffer->stride_cr) +
                ((recon_buffer->org_x + round_origin_x) >> 1);
            uint8_t* ep_recon_cr = recon_buffer->buffer_cr + recon_cr_offset;
            uint8_t* md_recon_cr = blk_ptr->recon_tmp->buffer_cr;

            for (uint32_t i = 0; i < ctx->blk_geom->bheight_uv; i++) {
                svt_memcpy(ep_recon_cr + i * recon_buffer->stride_cr,
                           md_recon_cr + i * blk_ptr->recon_tmp->stride_cr,
                           ctx->blk_geom->bwidth_uv * sizeof(uint8_t));
            }

            // Cb
            uint32_t recon_cb_offset = (((recon_buffer->org_y + round_origin_y) >> 1) * recon_buffer->stride_cb) +
                ((recon_buffer->org_x + round_origin_x) >> 1);
            uint8_t* ep_recon_cb = recon_buffer->buffer_cb + recon_cb_offset;
            uint8_t* md_recon_cb = blk_ptr->recon_tmp->buffer_cb;

            for (uint32_t i = 0; i < ctx->blk_geom->bheight_uv; i++) {
                svt_memcpy(ep_recon_cb + i * recon_buffer->stride_cb,
                           md_recon_cb + i * blk_ptr->recon_tmp->stride_cb,
                           ctx->blk_geom->bwidth_uv * sizeof(uint8_t));
            }
        }
    }
}

// Copy quantized coeffs to EncDec buffers if EncDec was bypassed. If pred depth only was used and NSQ is OFF data
// was copied directly to EncDec buffers in MD.
static void copy_qcoeffs(PictureControlSet* pcs, EncDecContext* ctx, BlkStruct* blk_ptr, uint32_t blk_coded_area,
                         uint32_t blk_coded_area_uv) {
    const BlockGeom*     blk_geom        = ctx->blk_geom;
    EbPictureBufferDesc* coeff_buffer_sb = pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index];
    const uint8_t        tx_depth        = blk_ptr->block_mi.tx_depth;
    const uint8_t        txb_itr         = ctx->txb_itr;
    const uint8_t        uv_pass         = tx_depth && txb_itr ? 0 : 1; //NM: 128x128 exeption

    int32_t* ep_coeff = ((int32_t*)coeff_buffer_sb->buffer_y) + ctx->coded_area_sb_update;
    int32_t* md_coeff = ((int32_t*)blk_ptr->coeff_tmp->buffer_y) + blk_coded_area;

    if ((blk_ptr->y_has_coeff & (1 << txb_itr))) {
        const TxSize tx_size   = tx_depth_to_tx_size[tx_depth][blk_geom->bsize];
        const int    tx_width  = tx_size_wide[tx_size];
        const int    tx_height = tx_size_high[tx_size];
        svt_memcpy(ep_coeff, md_coeff, sizeof(int32_t) * tx_height * tx_width);
    }

    if (ctx->md_ctx->has_uv && uv_pass) {
        const TxSize tx_size_uv   = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
        const int    tx_width_uv  = tx_size_wide[tx_size_uv];
        const int    tx_height_uv = tx_size_high[tx_size_uv];
        int32_t*     ep_coeff_cb  = ((int32_t*)coeff_buffer_sb->buffer_cb) + ctx->coded_area_sb_uv_update;
        int32_t*     md_coeff_cb  = ((int32_t*)blk_ptr->coeff_tmp->buffer_cb) + blk_coded_area_uv;

        if ((blk_ptr->u_has_coeff & (1 << txb_itr))) {
            svt_memcpy(ep_coeff_cb, md_coeff_cb, sizeof(int32_t) * tx_height_uv * tx_width_uv);
        }

        int32_t* ep_coeff_cr = ((int32_t*)coeff_buffer_sb->buffer_cr) + ctx->coded_area_sb_uv_update;
        int32_t* md_coeff_cr = ((int32_t*)blk_ptr->coeff_tmp->buffer_cr) + blk_coded_area_uv;

        if ((blk_ptr->v_has_coeff & (1 << txb_itr))) {
            svt_memcpy(ep_coeff_cr, md_coeff_cr, sizeof(int32_t) * tx_height_uv * tx_width_uv);
        }
    }
}

// Perform CDF update (MD feature) for coeff-related CDFs
void update_coeff_cdf(PictureControlSet* pcs, EncDecContext* ctx, BlkStruct* blk_ptr) {
    ModeDecisionContext* md_ctx          = ctx->md_ctx;
    const BlockGeom*     blk_geom        = ctx->blk_geom;
    EbPictureBufferDesc* coeff_buffer_sb = pcs->ppcs->enc_dec_ptr->quantized_coeff[ctx->sb_index];
    const uint8_t        tx_depth        = blk_ptr->block_mi.tx_depth;
    const uint8_t        txb_itr         = ctx->txb_itr;
    const uint8_t        uv_pass         = tx_depth && ctx->txb_itr ? 0 : 1; //NM: 128x128 exeption
    const uint16_t       tile_idx        = ctx->tile_index;
    const int            is_inter        = is_inter_block(&blk_ptr->block_mi);
    const TxSize         tx_size         = tx_depth_to_tx_size[tx_depth][blk_geom->bsize];
    const int            tx_width        = tx_size_wide[tx_size];
    const int            tx_height       = tx_size_high[tx_size];
    const TxSize         tx_size_uv      = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
    const int            tx_width_uv     = tx_size_wide[tx_size_uv];
    const int            tx_height_uv    = tx_size_high[tx_size_uv];
    const uint16_t       txb_origin_x    = ctx->blk_org_x + tx_org[blk_geom->bsize][is_inter][tx_depth][txb_itr].x;
    const uint16_t       txb_origin_y    = ctx->blk_org_y + tx_org[blk_geom->bsize][is_inter][tx_depth][txb_itr].y;

    md_ctx->luma_txb_skip_context = 0;
    md_ctx->luma_dc_sign_context  = 0;
    svt_aom_get_txb_ctx(pcs,
                        COMPONENT_LUMA,
                        pcs->ep_luma_dc_sign_level_coeff_na_update[tile_idx],
                        txb_origin_x,
                        txb_origin_y,
                        blk_geom->bsize,
                        tx_size,
                        &md_ctx->luma_txb_skip_context,
                        &md_ctx->luma_dc_sign_context);

    if (md_ctx->has_uv && uv_pass) {
        md_ctx->cb_txb_skip_context = 0;
        md_ctx->cb_dc_sign_context  = 0;
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_CHROMA,
                            pcs->ep_cb_dc_sign_level_coeff_na_update[tile_idx],
                            ROUND_UV(txb_origin_x) >> 1,
                            ROUND_UV(txb_origin_y) >> 1,
                            blk_geom->bsize_uv,
                            tx_size_uv,
                            &md_ctx->cb_txb_skip_context,
                            &md_ctx->cb_dc_sign_context);

        md_ctx->cr_txb_skip_context = 0;
        md_ctx->cr_dc_sign_context  = 0;
        svt_aom_get_txb_ctx(pcs,
                            COMPONENT_CHROMA,
                            pcs->ep_cr_dc_sign_level_coeff_na_update[tile_idx],
                            ROUND_UV(txb_origin_x) >> 1,
                            ROUND_UV(txb_origin_y) >> 1,
                            blk_geom->bsize_uv,
                            tx_size_uv,
                            &md_ctx->cr_txb_skip_context,
                            &md_ctx->cr_dc_sign_context);
    }

    ModeDecisionCandidateBuffer** cand_bf_ptr_array_base = md_ctx->cand_bf_ptr_array;
    ModeDecisionCandidateBuffer** cand_bf_ptr_array      = &(cand_bf_ptr_array_base[0]);
    ModeDecisionCandidateBuffer*  cand_bf;

    // Set the Candidate Buffer
    cand_bf = cand_bf_ptr_array[0];
    // Rate estimation function uses the values from CandidatePtr. The right values are copied from blk_ptr to CandidatePtr
    cand_bf->cand->block_mi.mode              = blk_ptr->block_mi.mode;
    cand_bf->cand->block_mi.filter_intra_mode = blk_ptr->block_mi.filter_intra_mode;
    if (blk_ptr->block_has_coeff) {
        uint64_t y_txb_coeff_bits;
        uint64_t cb_txb_coeff_bits;
        uint64_t cr_txb_coeff_bits;
        svt_aom_txb_estimate_coeff_bits(md_ctx,
                                        1, //allow_update_cdf,
                                        &pcs->ec_ctx_array[ctx->sb_index],
                                        pcs,
                                        cand_bf,
                                        ctx->coded_area_sb_update,
                                        ctx->coded_area_sb_uv_update,
                                        coeff_buffer_sb,
                                        blk_ptr->eob.y[txb_itr],
                                        blk_ptr->eob.u[txb_itr],
                                        blk_ptr->eob.v[txb_itr],
                                        &y_txb_coeff_bits,
                                        &cb_txb_coeff_bits,
                                        &cr_txb_coeff_bits,
                                        tx_size,
                                        tx_size_uv,
                                        blk_ptr->tx_type[txb_itr],
                                        blk_ptr->tx_type_uv,
                                        (md_ctx->has_uv && uv_pass) ? COMPONENT_ALL : COMPONENT_LUMA);
    }

    // Update the luma DC Sign Level Coeff Neighbor Array
    uint8_t dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.y[txb_itr];

    svt_aom_neighbor_array_unit_mode_write(pcs->ep_luma_dc_sign_level_coeff_na_update[tile_idx],
                                           (uint8_t*)&dc_sign_level_coeff,
                                           txb_origin_x,
                                           txb_origin_y,
                                           tx_width,
                                           tx_height,
                                           NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

    // Update the Cb DC Sign Level Coeff Neighbor Array
    if (md_ctx->has_uv && uv_pass) {
        dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.u[txb_itr];

        svt_aom_neighbor_array_unit_mode_write(pcs->ep_cb_dc_sign_level_coeff_na_update[tile_idx],
                                               (uint8_t*)&dc_sign_level_coeff,
                                               ROUND_UV(txb_origin_x) >> 1,
                                               ROUND_UV(txb_origin_y) >> 1,
                                               tx_width_uv,
                                               tx_height_uv,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

        // Update the Cr DC Sign Level Coeff Neighbor Array
        dc_sign_level_coeff = (uint8_t)blk_ptr->quant_dc.v[txb_itr];

        svt_aom_neighbor_array_unit_mode_write(pcs->ep_cr_dc_sign_level_coeff_na_update[tile_idx],
                                               (uint8_t*)&dc_sign_level_coeff,
                                               ROUND_UV(txb_origin_x) >> 1,
                                               ROUND_UV(txb_origin_y) >> 1,
                                               tx_width_uv,
                                               tx_height_uv,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);
    }
}

// Update encode-related data for the passed block
// expects ctx->blk_geom, ctx->blk_ptr, ctx->blk_org_x, ctx->blk_org_y to be set
static void update_b(PictureControlSet* pcs, EncDecContext* ctx, BlkStruct* blk_ptr, EcBlkStruct** output_blk_ptr) {
    ModeDecisionContext* md_ctx   = ctx->md_ctx;
    const BlockGeom*     blk_geom = ctx->blk_geom;
    SuperBlock*          sb_ptr   = md_ctx->sb_ptr;
    int                  sb_index = ctx->sb_index;
    const uint16_t       tile_idx = ctx->tile_index;

    if (!pcs->scs->allintra) {
        if (is_intra_mode(blk_ptr->block_mi.mode)) {
            ctx->tot_intra_coded_area += blk_geom->bwidth * blk_geom->bheight;
            pcs->sb_intra[sb_index] = 1;
        } else {
            if (pcs->ppcs->frm_hdr.allow_high_precision_mv) {
                bool hp = (blk_ptr->block_mi.mv[0].x % 2 != 0 || blk_ptr->block_mi.mv[0].y % 2 != 0);
                if (!hp && has_second_ref(&blk_ptr->block_mi)) {
                    hp = (blk_ptr->block_mi.mv[1].x % 2 != 0 || blk_ptr->block_mi.mv[1].y % 2 != 0);
                }
                if (hp) {
                    ctx->tot_hp_coded_area += blk_geom->bwidth * blk_geom->bheight;
                }
            }
            bool is_zero_mv = 0;
            if (abs(blk_ptr->block_mi.mv[0].x) < 8 && abs(blk_ptr->block_mi.mv[0].y) < 8) {
                is_zero_mv = 1;
            }
            if (has_second_ref(&blk_ptr->block_mi)) {
                if (abs(blk_ptr->block_mi.mv[1].x) < 8 && abs(blk_ptr->block_mi.mv[1].y) < 8) {
                    is_zero_mv = 1;
                }
            }
            if (is_zero_mv) {
                ctx->tot_cnt_zero_mv += blk_geom->bwidth * blk_geom->bheight;
            }
            if (blk_geom->sq_size == pcs->scs->sb_size && blk_ptr->block_mi.mode != NEWMV &&
                blk_ptr->block_mi.mode != NEW_NEWMV) {
                pcs->sb_64x64_mvp[sb_index] = 1;
            }
        }

        if (blk_ptr->block_has_coeff == 0) {
            ctx->tot_skip_coded_area += blk_geom->bwidth * blk_geom->bheight;
        } else {
            pcs->sb_skip[sb_index] = 0;
        }
        pcs->sb_min_sq_size[sb_index] = MIN(blk_geom->sq_size, pcs->sb_min_sq_size[sb_index]);
        pcs->sb_max_sq_size[sb_index] = MAX(blk_geom->sq_size, pcs->sb_max_sq_size[sb_index]);
    }
    svt_block_on_mutex(pcs->ppcs->pcs_total_rate_mutex);
    pcs->ppcs->pcs_total_rate += blk_ptr->total_rate;
    svt_release_mutex(pcs->ppcs->pcs_total_rate_mutex);

    // If needed, copy recon and qcoeffs from MD buffers to EC buffers and update coeff-related CDFs
    if (pcs->cdf_ctrl.update_coef || (md_ctx->bypass_encdec && !(md_ctx->fixed_partition))) {
        // Copy recon to EncDec buffers if EncDec was bypassed; if pred depth only was used
        // and NSQ is OFF data was copied directly to EncDec buffers in MD
        if (md_ctx->bypass_encdec && !(md_ctx->fixed_partition)) {
            copy_recon(pcs, md_ctx, blk_ptr);
        }

        // Initialize the Transform Loop
        const uint8_t  tx_depth          = blk_ptr->block_mi.tx_depth;
        const uint16_t txb_count         = tx_blocks_per_depth[blk_geom->bsize][tx_depth];
        const TxSize   tx_size           = tx_depth_to_tx_size[tx_depth][blk_geom->bsize];
        const int      tx_width          = tx_size_wide[tx_size];
        const int      tx_height         = tx_size_high[tx_size];
        const TxSize   tx_size_uv        = av1_get_max_uv_txsize(blk_geom->bsize, 1, 1);
        const int      tx_width_uv       = tx_size_wide[tx_size_uv];
        const int      tx_height_uv      = tx_size_high[tx_size_uv];
        uint32_t       blk_coded_area    = 0;
        uint32_t       blk_coded_area_uv = 0;
        for (ctx->txb_itr = 0; ctx->txb_itr < txb_count; ctx->txb_itr++) {
            const uint8_t uv_pass = tx_depth && ctx->txb_itr ? 0 : 1; //NM: 128x128 exeption

            // Copy quantized coeffs to EncDec buffers if EncDec was bypassed; if pred depth only was used
            // and NSQ is OFF data was copied directly to EncDec buffers in MD
            if (md_ctx->bypass_encdec && !(md_ctx->fixed_partition)) {
                copy_qcoeffs(pcs, ctx, blk_ptr, blk_coded_area, blk_coded_area_uv);
            }

            // Perform CDF update (MD feature) if enabled
            if (pcs->cdf_ctrl.update_coef) {
                update_coeff_cdf(pcs, ctx, blk_ptr);
            }

            blk_coded_area += tx_width * tx_height;
            ctx->coded_area_sb_update += tx_width * tx_height;

            if (md_ctx->has_uv && uv_pass) {
                blk_coded_area_uv += tx_width_uv * tx_height_uv;
                ctx->coded_area_sb_uv_update += tx_width_uv * tx_height_uv;
            }
        }
    }
    if (!md_ctx->bypass_encdec) {
        md_ctx->blk_org_x = ctx->blk_org_x;
        md_ctx->blk_org_y = ctx->blk_org_y;
        md_ctx->blk_geom  = ctx->blk_geom;
        svt_aom_update_mi_map_enc_dec(blk_ptr, md_ctx, pcs);
    }
    if (pcs->cdf_ctrl.update_se) {
        // Update the partition Neighbor Array
        PartitionContext partition;
        partition.above = partition_context_lookup[blk_geom->bsize].above;
        partition.left  = partition_context_lookup[blk_geom->bsize].left;

        svt_aom_neighbor_array_unit_mode_write(pcs->ep_partition_context_na[tile_idx],
                                               (uint8_t*)&partition,
                                               ctx->blk_org_x,
                                               ctx->blk_org_y,
                                               blk_geom->bwidth,
                                               blk_geom->bheight,
                                               NEIGHBOR_ARRAY_UNIT_TOP_AND_LEFT_ONLY_MASK);

        // Update the CDFs based on the current block
        blk_ptr->av1xd->tile_ctx           = &pcs->ec_ctx_array[sb_index];
        uint32_t txfm_context_left_index   = get_neighbor_array_unit_left_index(pcs->ep_txfm_context_na[tile_idx],
                                                                              ctx->blk_org_y);
        uint32_t txfm_context_above_index  = get_neighbor_array_unit_top_index(pcs->ep_txfm_context_na[tile_idx],
                                                                              ctx->blk_org_x);
        blk_ptr->av1xd->above_txfm_context = &(pcs->ep_txfm_context_na[tile_idx]->top_array[txfm_context_above_index]);
        blk_ptr->av1xd->left_txfm_context  = &(pcs->ep_txfm_context_na[tile_idx]->left_array[txfm_context_left_index]);
        svt_aom_tx_size_bits(pcs,
                             ctx->blk_ptr->segment_id,
                             md_ctx->md_rate_est_ctx,
                             blk_ptr->av1xd,
                             blk_ptr->av1xd->mi[0],
                             tx_depth_to_tx_size[blk_ptr->block_mi.tx_depth][blk_geom->bsize],
                             pcs->ppcs->frm_hdr.tx_mode,
                             blk_geom->bsize,
                             !blk_ptr->block_has_coeff,
                             &pcs->ec_ctx_array[sb_index],
                             1 /*allow_update_cdf*/);
        svt_aom_update_stats(pcs, blk_ptr, ctx->blk_org_y >> MI_SIZE_LOG2, ctx->blk_org_x >> MI_SIZE_LOG2);
    }

    // Copy final symbols and mode info from MD array to SB ptr
    // Data will be overwritten each iteration, so copying is useful. Data is updated at EntropyCoding.
    sb_ptr->final_blk_arr[sb_ptr->final_blk_cnt].av1xd = NULL;
    // ENCDEC palette info buffer
    {
        if (svt_av1_allow_palette(pcs->ppcs->palette_level, blk_geom->bsize)) {
            ec_rtime_alloc_palette_info(&sb_ptr->final_blk_arr[sb_ptr->final_blk_cnt]);
        } else {
            sb_ptr->final_blk_arr[sb_ptr->final_blk_cnt].palette_info = NULL;
        }
    }
    BlkStruct*   src_cu = blk_ptr;
    EcBlkStruct* dst_cu = &sb_ptr->final_blk_arr[sb_ptr->final_blk_cnt];
    *output_blk_ptr     = &sb_ptr->final_blk_arr[sb_ptr->final_blk_cnt];
    svt_aom_move_blk_data(pcs, ctx, src_cu, dst_cu);
    sb_ptr->final_blk_arr[sb_ptr->final_blk_cnt++].av1xd = sb_ptr->av1xd;
    // MFMV Update
    if (pcs->scs->mfmv_enabled && pcs->slice_type != I_SLICE && pcs->ppcs->is_ref) {
        uint32_t           mi_stride = pcs->mi_stride;
        int32_t            mi_row    = ctx->blk_org_y >> MI_SIZE_LOG2;
        int32_t            mi_col    = ctx->blk_org_x >> MI_SIZE_LOG2;
        const int32_t      offset    = mi_row * mi_stride + mi_col;
        MbModeInfo*        mbmi      = pcs->mi_grid_base[offset];
        const int          x_mis  = AOMMIN(ctx->blk_geom->bwidth >> MI_SIZE_LOG2, pcs->ppcs->av1_cm->mi_cols - mi_col);
        const int          y_mis  = AOMMIN(ctx->blk_geom->bheight >> MI_SIZE_LOG2, pcs->ppcs->av1_cm->mi_rows - mi_row);
        EbReferenceObject* obj_l0 = (EbReferenceObject*)pcs->ppcs->ref_pic_wrapper->object_ptr;

        av1_copy_frame_mvs(pcs, pcs->ppcs->av1_cm, mbmi[0], mi_row, mi_col, x_mis, y_mis, obj_l0);
    }
}

/*******************************************
* Encode Pass
*
* Summary: Performs an AV1 conformant encode/reconstruction
*   for a block based on the pre-determined mode info.
*
* Inputs:
*   SourcePic
*   Coding Results
*   SB Location
*   Sequence Control Set
*   Picture Control Set
*
* Outputs:
*   Reconstructed Samples
*   Coefficient Samples
*
*******************************************/
static void encode_b(PictureControlSet* pcs, EncDecContext* ctx, BlkStruct* blk_ptr, EcBlkStruct** output_blk_ptr,
                     const int mi_row, const int mi_col) {
    ModeDecisionContext* md_ctx = ctx->md_ctx;
    ctx->blk_geom = md_ctx->blk_geom = get_blk_geom_mds(pcs->scs->blk_geom_mds, blk_ptr->mds_idx);
    ctx->blk_ptr = md_ctx->blk_ptr = blk_ptr;
    ctx->blk_org_x = md_ctx->blk_org_x = mi_col << MI_SIZE_LOG2;
    ctx->blk_org_y = md_ctx->blk_org_y = mi_row << MI_SIZE_LOG2;
    md_ctx->has_uv                     = is_chroma_reference(mi_row, mi_col, md_ctx->blk_geom->bsize, 1, 1);
    if (ctx->md_ctx->bypass_encdec) {
        update_b(pcs, ctx, blk_ptr, output_blk_ptr);
        return;
    }

    /* ED should use the skip decision from MD. If MD signals 0 coeffs, the TX will
    be bypassed unless MD did not perform chroma (blk_skip_decision) or the block is an
    INTRA block (since the prediction at MD may not be conformant). */
    ctx->md_skip_blk         = md_ctx->blk_skip_decision
                ? ((is_intra_mode(blk_ptr->block_mi.mode) || blk_ptr->block_has_coeff) ? 0 : 1)
                : 0;
    blk_ptr->block_has_coeff = 0;

    if (is_inter_block(&blk_ptr->block_mi)) {
        perform_inter_coding_loop(pcs, ctx);
    } else if (is_intra_mode(blk_ptr->block_mi.mode)) {
        if (pcs->scs->static_config.encoder_bit_depth > EB_EIGHT_BIT && pcs->hbd_md == 0 &&
            blk_ptr->palette_size[0] > 0) {
            //MD was done on 8bit, scale  palette colors to 10bit
            for (uint8_t col = 0; col < blk_ptr->palette_size[0]; col++) {
                blk_ptr->palette_info->pmi.palette_colors[col] *= 4;
            }
        }
        perform_intra_coding_loop(pcs, ctx);
    } else {
        EncodeContext* enc_ctx = pcs->scs->enc_ctx;
        CHECK_REPORT_ERROR_NC(enc_ctx->app_callback_ptr, EB_ENC_CL_ERROR2);
    }

    if (pcs->ppcs->frm_hdr.allow_intrabc && ctx->is_16bit && (ctx->bit_depth == EB_EIGHT_BIT)) {
        svt_aom_convert_recon_16bit_to_8bit(pcs, ctx);
    }

    // Update block info and neighbour arrays needed for future blocks/pictures
    update_b(pcs, ctx, blk_ptr, output_blk_ptr);
}

void svt_aom_encode_sb(SequenceControlSet* scs, PictureControlSet* pcs, EncDecContext* ctx, SuperBlock* sb_ptr,
                       PC_TREE* pc_tree, PARTITION_TREE* ptree, int mi_row, int mi_col) {
    if (mi_row >= pcs->ppcs->av1_cm->mi_rows || mi_col >= pcs->ppcs->av1_cm->mi_cols) {
        return;
    }

    const BlockSize bsize = pc_tree->bsize;
    assert(bsize < BLOCK_SIZES_ALL);
    const int           hbs          = mi_size_wide[bsize] >> 1;
    const PartitionType partition    = pc_tree->partition;
    const int           quarter_step = mi_size_wide[bsize] >> 2;

    ptree->partition   = partition;
    ptree->bsize       = bsize;
    ctx->md_ctx->shape = from_part_to_shape[partition];
    if (pcs->cdf_ctrl.update_se) {
        // Update the partition stats
        svt_aom_update_part_stats(pcs, partition, bsize, ctx->tile_index, ctx->sb_index, mi_row, mi_col);
    }

    switch (partition) {
    case PARTITION_NONE:
        encode_b(pcs, ctx, pc_tree->block_data[PART_N][0], &ptree->blk_data[0], mi_row, mi_col);
        break;
    case PARTITION_HORZ:
        encode_b(pcs, ctx, pc_tree->block_data[PART_H][0], &ptree->blk_data[0], mi_row, mi_col);
        if (mi_row + hbs < pcs->ppcs->av1_cm->mi_rows) {
            encode_b(pcs, ctx, pc_tree->block_data[PART_H][1], &ptree->blk_data[1], mi_row + hbs, mi_col);
        }
        break;
    case PARTITION_VERT:
        encode_b(pcs, ctx, pc_tree->block_data[PART_V][0], &ptree->blk_data[0], mi_row, mi_col);
        if (mi_col + hbs < pcs->ppcs->av1_cm->mi_cols) {
            encode_b(pcs, ctx, pc_tree->block_data[PART_V][1], &ptree->blk_data[1], mi_row, mi_col + hbs);
        }
        break;
    case PARTITION_SPLIT:
        for (int i = 0; i < SUB_PARTITIONS_SPLIT; ++i) {
            const int x_idx = (i & 1) * hbs;
            const int y_idx = (i >> 1) * hbs;
            if (mi_row + y_idx >= pcs->ppcs->av1_cm->mi_rows || mi_col + x_idx >= pcs->ppcs->av1_cm->mi_cols) {
                continue;
            }
            svt_aom_encode_sb(
                scs, pcs, ctx, sb_ptr, pc_tree->split[i], ptree->sub_tree[i], mi_row + y_idx, mi_col + x_idx);
        }
        break;
    case PARTITION_HORZ_A:
        encode_b(pcs, ctx, pc_tree->block_data[PART_HA][0], &ptree->blk_data[0], mi_row, mi_col);
        encode_b(pcs, ctx, pc_tree->block_data[PART_HA][1], &ptree->blk_data[1], mi_row, mi_col + hbs);
        encode_b(pcs, ctx, pc_tree->block_data[PART_HA][2], &ptree->blk_data[2], mi_row + hbs, mi_col);
        break;
    case PARTITION_HORZ_B:
        encode_b(pcs, ctx, pc_tree->block_data[PART_HB][0], &ptree->blk_data[0], mi_row, mi_col);
        encode_b(pcs, ctx, pc_tree->block_data[PART_HB][1], &ptree->blk_data[1], mi_row + hbs, mi_col);
        encode_b(pcs, ctx, pc_tree->block_data[PART_HB][2], &ptree->blk_data[2], mi_row + hbs, mi_col + hbs);
        break;
    case PARTITION_VERT_A:
        encode_b(pcs, ctx, pc_tree->block_data[PART_VA][0], &ptree->blk_data[0], mi_row, mi_col);
        encode_b(pcs, ctx, pc_tree->block_data[PART_VA][1], &ptree->blk_data[1], mi_row + hbs, mi_col);
        encode_b(pcs, ctx, pc_tree->block_data[PART_VA][2], &ptree->blk_data[2], mi_row, mi_col + hbs);
        break;
    case PARTITION_VERT_B:
        encode_b(pcs, ctx, pc_tree->block_data[PART_VB][0], &ptree->blk_data[0], mi_row, mi_col);
        encode_b(pcs, ctx, pc_tree->block_data[PART_VB][1], &ptree->blk_data[1], mi_row, mi_col + hbs);
        encode_b(pcs, ctx, pc_tree->block_data[PART_VB][2], &ptree->blk_data[2], mi_row + hbs, mi_col + hbs);
        break;
    case PARTITION_HORZ_4:
        for (int i = 0; i < SUB_PARTITIONS_PART4; ++i) {
            int this_mi_row = mi_row + i * quarter_step;
            if (i > 0 && this_mi_row >= pcs->ppcs->av1_cm->mi_rows) {
                // Only the last block is able to be outside the picture boundary. If one of the first
                // 3 blocks is outside the boundary, H4 is not a valid partition (see AV1 spec 5.11.4)
                assert(i == 3);
                break;
            }
            encode_b(pcs, ctx, pc_tree->block_data[PART_H4][i], &ptree->blk_data[i], this_mi_row, mi_col);
        }
        break;
    case PARTITION_VERT_4:
        for (int i = 0; i < SUB_PARTITIONS_PART4; ++i) {
            int this_mi_col = mi_col + i * quarter_step;
            if (i > 0 && this_mi_col >= pcs->ppcs->av1_cm->mi_cols) {
                // Only the last block is able to be outside the picture boundary. If one of the first
                // 3 blocks is outside the boundary, H4 is not a valid partition (see AV1 spec 5.11.4)
                assert(i == 3);
                break;
            }
            encode_b(pcs, ctx, pc_tree->block_data[PART_V4][i], &ptree->blk_data[i], mi_row, this_mi_col);
        }
        break;
    default:
        assert(0 && "Invalid partition type.");
        break;
    }
}
