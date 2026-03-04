/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef EncodeTxbRef_h
#define EncodeTxbRef_h

#include <stdint.h>
#include "cabac_context_model.h"

#ifdef __cplusplus
extern "C" {
#endif

void svt_av1_get_nz_map_contexts_c(const uint8_t* const levels, const int16_t* const scan, const uint16_t eob,
                                   const TxSize tx_size, const TxClass tx_class, int8_t* const coeff_contexts);
#ifdef __cplusplus
} // extern "C"
#endif

#endif
