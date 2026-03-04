/*
 * Copyright (c) 2023, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef AV1_INV_TXFM_NEON_H_
#define AV1_INV_TXFM_NEON_H_

#include "definitions.h"

static inline void lowbd_inv_txfm2d_add_idtx_neon(const int32_t* input, uint8_t* output_r, int32_t stride_r,
                                                  uint8_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size,
                                                  int32_t eob);

static inline void lowbd_inv_txfm2d_add_v_identity_neon(const int32_t* input, uint8_t* output_r, int32_t stride_r,
                                                        uint8_t* output_w, int32_t stride_w, TxType tx_type,
                                                        TxSize tx_size, int32_t eob);

static inline void lowbd_inv_txfm2d_add_h_identity_neon(const int32_t* input, uint8_t* output_r, int32_t stride_r,
                                                        uint8_t* output_w, int32_t stride_w, TxType tx_type,
                                                        TxSize tx_size, int32_t eob);

static inline void lowbd_inv_txfm2d_add_no_identity_neon(const int32_t* input, uint8_t* output_r, int32_t stride_r,
                                                         uint8_t* output_w, int32_t stride_w, TxType tx_type,
                                                         TxSize tx_size, int32_t eob);

void svt_av1_lowbd_inv_txfm2d_add_neon(const int32_t* input, uint8_t* output_r, int32_t stride_r, uint8_t* output_w,
                                       int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob) {
    switch (tx_type) {
    case IDTX:
        lowbd_inv_txfm2d_add_idtx_neon(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob);
        break;

    case H_DCT:
    case H_ADST:
    case H_FLIPADST:
        lowbd_inv_txfm2d_add_v_identity_neon(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob);
        break;

    case V_DCT:
    case V_ADST:
    case V_FLIPADST:
        lowbd_inv_txfm2d_add_h_identity_neon(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob);
        break;

    default:
        lowbd_inv_txfm2d_add_no_identity_neon(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob);
        break;
    }
}

#endif // AV1_INV_TXFM_NEON_H_
