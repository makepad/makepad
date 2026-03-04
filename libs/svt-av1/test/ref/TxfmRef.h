/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file TxfmRef.h
 *
 * @brief reference implementation for txfm, including :
 * - reference_dct_1d
 * - reference_adst_1d
 * - reference_idtx_1d
 * - reference_txfm_1d
 * - reference_txfm_2d
 * - fadst_ref
 *
 * @author Cidana-Edmond, Cidana-Wenyao
 *
 ******************************************************************************/
#ifndef _TEST_REFERENCE_H_
#define _TEST_REFERENCE_H_

#include <stdint.h>

#include "definitions.h"
#include "transforms.h"
#include "gtest/gtest.h"
#include "util.h"

/* Defined in EbTransforms.c */
namespace svt_av1_test_reference {
// forward transform 1d reference
using Txfm1dFuncRef = void (*)(const double *in, double *out, int size);

void reference_dct_1d(const double *in, double *out, int size);

// TODO(any): Copied from the old 'fadst4' (same as the new 'svt_av1_fadst4_new'
// function). Should be replaced by a proper reference function that takes
// 'double' input & output.
void fadst4_ref(const TranLow *input, TranLow *output);

void reference_adst_1d(const double *in, double *out, int size);

void reference_idtx_1d(const double *in, double *out, int size);

void reference_txfm_1d(TxType1D type, const double *in, double *out, int size);

TxType1D get_txfm1d_types(TxfmType txfm_type);

// forward transform 2d reference
void reference_txfm_2d(const double *in, double *out, TxType tx_type,
                       TxSize tx_size, double scale_factor);

double get_scale_factor(const Txfm2dFlipCfg &cfg, const int tx_width,
                        const int tx_height);

}  // namespace svt_av1_test_reference

#endif  // _TEST_REFERENCE_H_
