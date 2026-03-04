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

#ifndef EbEncIntraPrediction_h
#define EbEncIntraPrediction_h

#include "me_process.h"
#include "mode_decision.h"
#include "intra_prediction.h"

#ifdef __cplusplus
extern "C" {
#endif

// clang-format off
static const IntraSize svt_aom_intra_unit[] =
{
    /*Note: e.g for V: there are case where we need the first
            pixel from left to pad the ref array */
    {1,1},//DC_PRED
    {1,1},//V_PRED
    {1,1},//H_PRED
    {2,1},//D45_PRED
    {1,1},//D135_PRED
    {1,1},//D113_PRED
    {1,1},//D157_PRED
    {1,2},//D203_PRED
    {2,1},//D67_PRED
    {1,1},//SMOOTH_PRED
    {1,1},//SMOOTH_V_PRED
    {1,1},//SMOOTH_H_PRED
    {2,2} //PAETH_PRED
};
// clang-format on

EbErrorType svt_av1_intra_prediction(uint8_t hbd_md, struct ModeDecisionContext* ctx, PictureControlSet* pcs,
                                     ModeDecisionCandidateBuffer* cand_bf);
EbErrorType svt_aom_update_neighbor_samples_array_open_loop_mb(uint8_t use_top_righ_bottom_left,
                                                               uint8_t update_top_neighbor, uint8_t* above_ref,
                                                               uint8_t* left_ref, EbPictureBufferDesc* input_ptr,
                                                               uint32_t stride, uint32_t srcOriginX,
                                                               uint32_t srcOriginY, uint8_t bwidth, uint8_t bheight);
EbErrorType svt_aom_update_neighbor_samples_array_open_loop_mb_recon(uint8_t use_top_righ_bottom_left,
                                                                     uint8_t update_top_neighbor, uint8_t* above_ref,
                                                                     uint8_t* left_ref, uint8_t* recon_ptr,
                                                                     uint32_t stride, uint32_t src_origin_x,
                                                                     uint32_t src_origin_y, uint8_t bwidth,
                                                                     uint8_t bheight, uint32_t width, uint32_t height);

void svt_av1_predict_intra_block(MacroBlockD* xd, BlockSize bsize, TxSize tx_size, PredictionMode mode,
                                 int32_t angle_delta, int32_t use_palette, PaletteInfo* palette_info,
                                 FilterIntraMode filter_intra_mode, uint8_t* top_neigh_array, uint8_t* left_neigh_array,
                                 EbPictureBufferDesc* recon_buffer, int32_t col_off, int32_t row_off, int32_t plane,
                                 Part shape, uint32_t dst_offset_x, uint32_t dst_offset_y, SeqHeader* seq_header_ptr,
                                 EbBitDepth bit_depth);
void svt_aom_precompute_intra_pred_for_inter_intra(PictureControlSet* pcs, struct ModeDecisionContext* ctx);
#ifdef __cplusplus
}
#endif
#endif // EbEncIntraPrediction_h
