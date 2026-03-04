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

#ifndef EbTransforms_h
#define EbTransforms_h

#include "definitions.h"
#include "coefficients.h"
#include "inv_transforms.h"

#ifdef __cplusplus
extern "C" {
#endif

#include "enc_dec_process.h"
#define UNIT_QUANT_SHIFT 2
#define UNIT_QUANT_FACTOR (1 << UNIT_QUANT_SHIFT)

extern const int8_t fwd_cos_bit_col[MAX_TXWH_IDX /*txw_idx*/][MAX_TXWH_IDX /*txh_idx*/];
extern const int8_t fwd_cos_bit_row[MAX_TXWH_IDX /*txw_idx*/][MAX_TXWH_IDX /*txh_idx*/];

extern const int8_t* fwd_txfm_shift_ls[TX_SIZES_ALL];

extern const uint8_t tx_blocks_per_depth[BLOCK_SIZES_ALL][MAX_VARTX_DEPTH + 1];

typedef struct Position {
    int x;
    int y;
} Position;

// origin is block - separate tables for INTRA (idx 0) and INTER (idx 1) needed b/c of tx depth 2
extern const Position tx_org[BLOCK_SIZES_ALL][2 /*is_inter*/][MAX_VARTX_DEPTH + 1][MAX_TXB_COUNT];

static INLINE int is_rect_tx(TxSize tx_size) {
    return tx_size >= TX_SIZES;
}

static INLINE int is_rect_tx_allowed_bsize(BlockSize bsize) {
    static const char lut[BLOCK_SIZES_ALL] = {
        0, // BLOCK_4X4
        1, // BLOCK_4X8
        1, // BLOCK_8X4
        0, // BLOCK_8X8
        1, // BLOCK_8X16
        1, // BLOCK_16X8
        0, // BLOCK_16X16
        1, // BLOCK_16X32
        1, // BLOCK_32X16
        0, // BLOCK_32X32
        1, // BLOCK_32X64
        1, // BLOCK_64X32
        0, // BLOCK_64X64
        0, // BLOCK_64X128
        0, // BLOCK_128X64
        0, // BLOCK_128X128
        1, // BLOCK_4X16
        1, // BLOCK_16X4
        1, // BLOCK_8X32
        1, // BLOCK_32X8
        1, // BLOCK_16X64
        1, // BLOCK_64X16
    };

    return lut[bsize];
}

static INLINE int is_rect_tx_allowed(/*const MacroBlockD *xd,*/
                                     const MbModeInfo* mbmi) {
    return is_rect_tx_allowed_bsize(mbmi->bsize) /*&&
            !xd->lossless[mbmi->segment_id]*/
        ;
}

////////////////////// QUANTIZATION//////////////
typedef struct QuantParam {
    int32_t      log_scale;
    TxSize       tx_size;
    const QmVal* qmatrix;
    const QmVal* iqmatrix;
} QuantParam;

static const uint32_t q_func[] = {26214, 23302, 20560, 18396, 16384, 14564};
EbErrorType svt_aom_estimate_transform(PictureControlSet* pcs, ModeDecisionContext* ctx, int16_t* residual_buffer,
                                       uint32_t residual_stride, int32_t* coeff_buffer, uint32_t coeff_stride,
                                       TxSize transform_size, uint64_t* three_quad_energy, uint32_t bit_depth,
                                       TxType transform_type, PlaneType component_type,
                                       EB_TRANS_COEFF_SHAPE trans_coeff_shape);

uint8_t svt_aom_quantize_inv_quantize(PictureControlSet* pcs, ModeDecisionContext* ctx, int32_t* coeff,
                                      int32_t* quant_coeff, int32_t* recon_coeff, uint32_t qindex,
                                      int32_t segmentation_qp_offset, TxSize txsize, uint16_t* eob,
                                      uint32_t component_type, uint32_t bit_depth, TxType tx_type,
                                      int16_t txb_skip_context, int16_t dc_sign_context, PredictionMode pred_mode,
                                      uint32_t lambda, bool is_encode_pass);

void svt_aom_quantize_inv_quantize_light(PictureControlSet* pcs, int32_t* coeff, int32_t* quant_coeff,
                                         int32_t* recon_coeff, uint32_t qindex, TxSize txsize, uint16_t* eob,
                                         uint32_t bit_depth, TxType tx_type);
void svt_av1_wht_fwd_txfm(int16_t* src_diff, int bw, int32_t* coeff, TxSize tx_size, EB_TRANS_COEFF_SHAPE pf_shape,
                          int bit_depth, int is_hbd);

TxfmFunc svt_aom_fwd_txfm_type_to_func(TxfmType txfmtype);

void av1_fwht4x4_c(int16_t* input, int32_t* output, uint32_t stride);
#ifdef __cplusplus
}
#endif

#endif // EbTransforms_h
