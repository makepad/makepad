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

#include "encode_txb_ref_c.h"
#include "common_utils.h"
#include "coefficients.h"
#include "full_loop.h"

static INLINE int get_nz_map_ctx(const uint8_t* const levels, const int coeff_idx, const int bwl, const int height,
                                 const int scan_idx, const int is_eob, const TxSize tx_size, const TxClass tx_class) {
    if (is_eob) {
        if (scan_idx == 0) {
            return 0;
        }
        if (scan_idx <= (height << bwl) / 8) {
            return 1;
        }
        if (scan_idx <= (height << bwl) / 4) {
            return 2;
        }
        return 3;
    }
    const int stats = get_nz_mag(levels + get_padded_idx(coeff_idx, bwl), bwl, tx_class);
    return get_nz_map_ctx_from_stats(stats, coeff_idx, bwl, tx_size, tx_class);
}

void svt_av1_get_nz_map_contexts_c(const uint8_t* const levels, const int16_t* const scan, const uint16_t eob,
                                   const TxSize tx_size, const TxClass tx_class, int8_t* const coeff_contexts) {
    const int bwl    = get_txb_bwl(tx_size);
    const int height = get_txb_high(tx_size);
    for (int i = 0; i < eob; ++i) {
        const int pos       = scan[i];
        coeff_contexts[pos] = get_nz_map_ctx(levels, pos, bwl, height, i, i == eob - 1, tx_size, tx_class);
    }
}
