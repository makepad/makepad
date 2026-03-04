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

#ifndef EbCoefficients_h
#define EbCoefficients_h

#include "definitions.h"
#include "coefficients.h"
#include "cabac_context_model.h"

#ifdef __cplusplus
extern "C" {
#endif

#define NZ_MAP_CTX_0 SIG_COEF_CONTEXTS_2D
#define NZ_MAP_CTX_5 (NZ_MAP_CTX_0 + 5)
#define NZ_MAP_CTX_10 (NZ_MAP_CTX_0 + 10)

static const int nz_map_ctx_offset_1d[32] = {
    NZ_MAP_CTX_0,  NZ_MAP_CTX_5,  NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10,
    NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10,
    NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10,
    NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10,
    NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10, NZ_MAP_CTX_10,
};

extern const ScanOrder eb_av1_scan_orders[TX_SIZES_ALL][3];
extern const int       tx_type_to_scan_index[TX_TYPES];

static inline const ScanOrder* get_scan_order(const int tx_size, const int tx_type) {
    return &eb_av1_scan_orders[tx_size][tx_type_to_scan_index[tx_type]];
}

static const int32_t tx_size_2d[TX_SIZES_ALL + 1] = {
    16, 64, 256, 1024, 4096, 32, 32, 128, 128, 512, 512, 2048, 2048, 64, 64, 256, 256, 1024, 1024,
};

static inline int32_t av1_get_tx_scale(const TxSize tx_size) {
    const int32_t pels = tx_size_2d[tx_size];
    // Largest possible pels is 4096 (64x64).
    return (pels > 256) + (pels > 1024);
}

extern const int8_t* eb_av1_nz_map_ctx_offset[19];

static INLINE int get_lower_levels_ctx_eob(int bwl, int height, int scan_idx) {
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

static AOM_FORCE_INLINE int get_br_ctx_eob(const int c, // raster order
                                           const int bwl, const TxClass tx_class) {
    const int row = c >> bwl;
    const int col = c - (row << bwl);
    if (c == 0) {
        return 0;
    }
    if ((tx_class == TX_CLASS_2D && row < 2 && col < 2) || (tx_class == TX_CLASS_HORIZ && col == 0) ||
        (tx_class == TX_CLASS_VERT && row == 0)) {
        return 7;
    }
    return 14;
}

static AOM_FORCE_INLINE int get_br_ctx(const uint8_t* const levels,
                                       const int            c, // raster order
                                       const int bwl, const TxClass tx_class) {
    const int row    = c >> bwl;
    const int col    = c - (row << bwl);
    const int stride = (1 << bwl) + TX_PAD_HOR;
    const int pos    = row * stride + col;
    int       mag    = levels[pos + 1];
    mag += levels[pos + stride];
    switch (tx_class) {
    case TX_CLASS_2D:
        mag += levels[pos + stride + 1];
        mag = AOMMIN((mag + 1) >> 1, 6);
        if (c == 0) {
            return mag;
        }
        if ((row < 2) && (col < 2)) {
            return mag + 7;
        }
        break;
    case TX_CLASS_HORIZ:
        mag += levels[pos + 2];
        mag = AOMMIN((mag + 1) >> 1, 6);
        if (c == 0) {
            return mag;
        }
        if (col == 0) {
            return mag + 7;
        }
        break;
    case TX_CLASS_VERT:
        mag += levels[pos + (stride << 1)];
        mag = AOMMIN((mag + 1) >> 1, 6);
        if (c == 0) {
            return mag;
        }
        if (row == 0) {
            return mag + 7;
        }
        break;
    default:
        break;
    }
    return mag + 14;
}

static const uint8_t clip_max3[256] = {
    0, 1, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3};

static INLINE int get_padded_idx(const int idx, const int bwl) {
    return idx + ((idx >> bwl) << TX_PAD_HOR_LOG2);
}

static AOM_FORCE_INLINE int get_nz_mag(const uint8_t* const levels, const int bwl, const TxClass tx_class) {
    int mag;

    // Note: AOMMIN(level, 3) is useless for decoder since level < 3.
    mag = clip_max3[levels[1]]; // { 0, 1 }
    mag += clip_max3[levels[(1 << bwl) + TX_PAD_HOR]]; // { 1, 0 }

    if (tx_class == TX_CLASS_2D) {
        mag += clip_max3[levels[(1 << bwl) + TX_PAD_HOR + 1]]; // { 1, 1 }
        mag += clip_max3[levels[2]]; // { 0, 2 }
        mag += clip_max3[levels[(2 << bwl) + (2 << TX_PAD_HOR_LOG2)]]; // { 2, 0 }
    } else if (tx_class == TX_CLASS_VERT) {
        mag += clip_max3[levels[(2 << bwl) + (2 << TX_PAD_HOR_LOG2)]]; // { 2, 0 }
        mag += clip_max3[levels[(3 << bwl) + (3 << TX_PAD_HOR_LOG2)]]; // { 3, 0 }
        mag += clip_max3[levels[(4 << bwl) + (4 << TX_PAD_HOR_LOG2)]]; // { 4, 0 }
    } else {
        mag += clip_max3[levels[2]]; // { 0, 2 }
        mag += clip_max3[levels[3]]; // { 0, 3 }
        mag += clip_max3[levels[4]]; // { 0, 4 }
    }

    return mag;
}

static AOM_FORCE_INLINE int get_nz_map_ctx_from_stats(const int stats,
                                                      const int coeff_idx, // raster order
                                                      const int bwl, const TxSize tx_size, const TxClass tx_class) {
    // tx_class == 0(TX_CLASS_2D)
    if ((tx_class | coeff_idx) == 0) {
        return 0;
    }
    int ctx = (stats + 1) >> 1;
    ctx     = AOMMIN(ctx, 4);
    switch (tx_class) {
    case TX_CLASS_2D: {
        // This is the algorithm to generate eb_av1_nz_map_ctx_offset[][]
        //   const int width = tx_size_wide[tx_size];
        //   const int height = tx_size_high[tx_size];
        //   if (width < height) {
        //     if (row < 2) return 11 + ctx;
        //   } else if (width > height) {
        //     if (col < 2) return 16 + ctx;
        //   }
        //   if (row + col < 2) return ctx + 1;
        //   if (row + col < 4) return 5 + ctx + 1;
        //   return 21 + ctx;
        return ctx + eb_av1_nz_map_ctx_offset[tx_size][coeff_idx];
    }
    case TX_CLASS_HORIZ: {
        const int row = coeff_idx >> bwl;
        const int col = coeff_idx - (row << bwl);
        return ctx + nz_map_ctx_offset_1d[col];
    }
    case TX_CLASS_VERT: {
        const int row = coeff_idx >> bwl;
        return ctx + nz_map_ctx_offset_1d[row];
    }
    default:
        break;
    }
    return 0;
}

static AOM_FORCE_INLINE int get_lower_levels_ctx(const uint8_t* levels, int coeff_idx, int bwl, TxSize tx_size,
                                                 TxClass tx_class) {
    const int stats = get_nz_mag(levels + get_padded_idx(coeff_idx, bwl), bwl, tx_class);
    return get_nz_map_ctx_from_stats(stats, coeff_idx, bwl, tx_size, tx_class);
}

#ifdef __cplusplus
}
#endif

#endif // EbCoefficients_h
