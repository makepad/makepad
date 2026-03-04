/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef EbCommonUtils_h
#define EbCommonUtils_h

#include "definitions.h"
#include "block_structures.h"
#include "cabac_context_model.h"

#ifdef __cplusplus
extern "C" {
#endif

extern const PredictionMode g_uv2y[16];
extern const PredictionMode fimode_to_intradir[FILTER_INTRA_MODES];
extern const uint8_t        intra_mode_context[INTRA_MODES];

extern const uint8_t eb_size_group_lookup[BLOCK_SIZES_ALL];

extern const uint8_t eb_num_pels_log2_lookup[BLOCK_SIZES_ALL];
extern const TxSize  eb_max_txsize_lookup[BLOCK_SIZES_ALL];

// Transform block width in unit
extern const int32_t eb_tx_size_wide_unit[TX_SIZES_ALL];
// Transform block height in unit
extern const int32_t eb_tx_size_high_unit[TX_SIZES_ALL];

extern const TxSize  eb_sub_tx_size_map[TX_SIZES_ALL];
extern const TxSize  txsize_sqr_map[TX_SIZES_ALL];
extern const TxSize  txsize_sqr_up_map[TX_SIZES_ALL];
extern const TxSize  tx_depth_to_tx_size[3][BLOCK_SIZES_ALL];
extern const int32_t tx_size_wide[TX_SIZES_ALL];
// Transform block height in pixels
extern const int32_t tx_size_high[TX_SIZES_ALL];
// Transform block width in log2
extern const int32_t tx_size_wide_log2[TX_SIZES_ALL];
// Transform block height in log2
extern const int32_t tx_size_high_log2[TX_SIZES_ALL];

extern const uint8_t mi_size_wide_log2[BLOCK_SIZES_ALL];
extern const uint8_t mi_size_high_log2[BLOCK_SIZES_ALL];
extern const TxSize  blocksize_to_txsize[BLOCK_SIZES_ALL];

extern const BlockSize svt_aom_ss_size_lookup[BLOCK_SIZES_ALL][2][2];

extern const int32_t av1_num_ext_tx_set[EXT_TX_SET_TYPES];
extern const int32_t av1_ext_tx_used[EXT_TX_SET_TYPES][TX_TYPES];

static INLINE TxSetType get_ext_tx_set_type(TxSize tx_size, int32_t is_inter, int32_t use_reduced_set) {
    const TxSize tx_size_sqr_up = txsize_sqr_up_map[tx_size];

    if (tx_size_sqr_up > TX_32X32) {
        return EXT_TX_SET_DCTONLY;
    }
    if (tx_size_sqr_up == TX_32X32) {
        return is_inter ? EXT_TX_SET_DCT_IDTX : EXT_TX_SET_DCTONLY;
    }
    if (use_reduced_set) {
        return is_inter ? EXT_TX_SET_DCT_IDTX : EXT_TX_SET_DTT4_IDTX;
    }
    const TxSize tx_size_sqr = txsize_sqr_map[tx_size];
    if (is_inter) {
        return (tx_size_sqr == TX_16X16 ? EXT_TX_SET_DTT9_IDTX_1DDCT : EXT_TX_SET_ALL16);
    } else {
        return (tx_size_sqr == TX_16X16 ? EXT_TX_SET_DTT4_IDTX : EXT_TX_SET_DTT4_IDTX_1DDCT);
    }
}

static INLINE int32_t get_ext_tx_types(TxSize tx_size, int32_t is_inter, int32_t use_reduced_set) {
    const int32_t set_type = get_ext_tx_set_type(tx_size, is_inter, use_reduced_set);
    return av1_num_ext_tx_set[set_type];
}

// Maps tx set types to the indices.
extern const int32_t ext_tx_set_index[2][EXT_TX_SET_TYPES];

static INLINE int32_t get_ext_tx_set(TxSize tx_size, int32_t is_inter, int32_t use_reduced_set) {
    const TxSetType set_type = get_ext_tx_set_type(tx_size, is_inter, use_reduced_set);
    return ext_tx_set_index[is_inter][set_type];
}

static INLINE uint8_t* set_levels(uint8_t* const levels_buf, const int32_t width) {
    return levels_buf + TX_PAD_TOP * (width + TX_PAD_HOR);
}

static INLINE TxSize av1_get_adjusted_tx_size(TxSize tx_size) {
    switch (tx_size) {
    case TX_64X64:
    case TX_64X32:
    case TX_32X64:
        return TX_32X32;
    case TX_64X16:
        return TX_32X16;
    case TX_16X64:
        return TX_16X32;
    default:
        return tx_size;
    }
}

static INLINE int get_txb_bwl(TxSize tx_size) {
    tx_size = av1_get_adjusted_tx_size(tx_size);
    return tx_size_wide_log2[tx_size];
}

static INLINE int get_txb_wide(TxSize tx_size) {
    tx_size = av1_get_adjusted_tx_size(tx_size);
    return tx_size_wide[tx_size];
}

static INLINE int get_txb_high(TxSize tx_size) {
    tx_size = av1_get_adjusted_tx_size(tx_size);
    return tx_size_high[tx_size];
}

static INLINE PredictionMode get_uv_mode(UvPredictionMode mode) {
    assert(mode < UV_INTRA_MODES);
    return g_uv2y[mode];
}

static INLINE BlockSize get_plane_block_size(BlockSize bsize, int32_t subsampling_x, int32_t subsampling_y) {
    if (bsize == BLOCK_INVALID) {
        return BLOCK_INVALID;
    }
    return svt_aom_ss_size_lookup[bsize][subsampling_x][subsampling_y];
}

static INLINE TxSize av1_get_max_uv_txsize(BlockSize bsize, int32_t subsampling_x, int32_t subsampling_y) {
    const BlockSize plane_bsize = get_plane_block_size(bsize, subsampling_x, subsampling_y);
    TxSize          uv_tx       = TX_INVALID;
    if (plane_bsize < BLOCK_SIZES_ALL) {
        uv_tx = blocksize_to_txsize[plane_bsize];
    }
    return av1_get_adjusted_tx_size(uv_tx);
}

// bsize is the luma bsize. tx_depth only used for luma.
static INLINE TxSize av1_get_tx_size(BlockSize bsize, int tx_depth, int plane /*, const MacroBlockD *xd*/) {
    //const MbModeInfo *mbmi = xd->mi[0];
    // if (xd->lossless[mbmi->segment_id]) return TX_4X4;
    if (plane == 0) {
        return tx_depth_to_tx_size[tx_depth][bsize];
    }
    // const MacroblockdPlane *pd = &xd->plane[plane];

    uint32_t ss_x = plane > 0 ? 1 : 0;
    uint32_t ss_y = plane > 0 ? 1 : 0;
    return av1_get_max_uv_txsize(bsize, ss_x, ss_y);
}

extern const PartitionType from_shape_to_part[EXT_PARTITION_TYPES];
extern const Part          from_part_to_shape[PART_S + 1];

// Width/height lookup tables in units of various block sizes
extern const uint8_t block_size_wide[BLOCK_SIZES_ALL];
extern const uint8_t block_size_high[BLOCK_SIZES_ALL];
extern const uint8_t mi_size_wide[BLOCK_SIZES_ALL];
extern const uint8_t mi_size_high[BLOCK_SIZES_ALL];

// 4X4, 8X8, 16X16, 32X32, 64X64, 128X128
#define SQR_BLOCK_SIZES 6

// Number of sub-partitions in rectangular partition types.
#define SUB_PARTITIONS_RECT 2

// Number of sub-partitions in split partition type.
#define SUB_PARTITIONS_SPLIT 4

// Number of sub-partitions in AB partition types.
#define SUB_PARTITIONS_AB 3

// Number of sub-partitions in 4-way partition types.
#define SUB_PARTITIONS_PART4 4

// A compressed version of the Partition_Subsize table in the spec (9.3.
// Conversion tables), for square block sizes only.
/* clang-format off */
extern const BlockSize svt_aom_subsize_lookup[EXT_PARTITION_TYPES][SQR_BLOCK_SIZES];

static INLINE int get_sqr_bsize_idx(BlockSize bsize) {
    switch (bsize) {
    case BLOCK_4X4: return 0;
    case BLOCK_8X8: return 1;
    case BLOCK_16X16: return 2;
    case BLOCK_32X32: return 3;
    case BLOCK_64X64: return 4;
    case BLOCK_128X128: return 5;
    default: return SQR_BLOCK_SIZES;
    }
}
// For a square block size 'bsize', returns the size of the sub-blocks used by
// the given partition type. If the partition produces sub-blocks of different
// sizes, then the function returns the largest sub-block size.
// Implements the Partition_Subsize lookup table in the spec (Section 9.3.
// Conversion tables).
// Note: the input block size should be square.
// Otherwise it's considered invalid.
static INLINE BlockSize get_partition_subsize(BlockSize bsize,
    PartitionType partition) {
    if (partition == PARTITION_INVALID) {
        return BLOCK_INVALID;
    }
    else {
        const int sqr_bsize_idx = get_sqr_bsize_idx(bsize);
        return sqr_bsize_idx >= SQR_BLOCK_SIZES
            ? BLOCK_INVALID
            : svt_aom_subsize_lookup[partition][sqr_bsize_idx];
    }
}

extern const uint8_t num_ns_per_shape[PART_S];
// gives the index offset (relative to SQ block) of the given nsq shape
// Different tables for 128x128 because H4/V4 are not allowed
extern const uint32_t ns_blk_offset_md[PART_S];
extern const uint32_t ns_blk_offset_128_md[PART_S];

/*
 * Update mi_row/mi_col to be the origin of the current block.
 * input: bsize is the block size of the square (PART_N) shape.
 * input: shape is the current partition type
 * input: nsi is the index of the block in the current partition
 * input: mi_row/col inputs are the block origin of the square (PART_N) shape and will be updated to output
 * the origin of the nsi block.
 */
static INLINE BlockSize partition_mi_offset(const BlockSize bsize, const Part shape, const unsigned int nsi, int* mi_row, int* mi_col) {
    const int hbs = mi_size_wide[bsize] >> 1;
    const int quarter_step = mi_size_wide[bsize] >> 2;
    PartitionType sub_bsize_part = PARTITION_INVALID;
    switch (shape) {
    case PART_N:
        assert(nsi == 0);
        return bsize;
    case PART_H:
        assert(nsi < SUB_PARTITIONS_RECT);
        if (nsi) {
            *mi_row += hbs;
        }
        sub_bsize_part = PARTITION_HORZ;
        break;
    case PART_V:
        assert(nsi < SUB_PARTITIONS_RECT);
        if (nsi) {
            *mi_col += hbs;
        }
        sub_bsize_part = PARTITION_VERT;
        break;
    case PART_HA:
        assert(nsi < SUB_PARTITIONS_AB);
        if (nsi) {
            *mi_col += nsi == 1 ? hbs : 0;
            *mi_row += nsi == 1 ? 0 : hbs;
        }
        sub_bsize_part = nsi < 2 ? PARTITION_SPLIT : PARTITION_HORZ_A;
        break;
    case PART_HB:
        assert(nsi < SUB_PARTITIONS_AB);
        if (nsi) {
            *mi_col += nsi == 1 ? 0 : hbs;
            *mi_row += hbs;
        }
        sub_bsize_part = nsi == 0 ? PARTITION_HORZ_B : PARTITION_SPLIT;
        break;
    case PART_VA:
        assert(nsi < SUB_PARTITIONS_AB);
        if (nsi) {
            *mi_col += nsi == 1 ? 0 : hbs;
            *mi_row += nsi == 1 ? hbs : 0;
        }
        sub_bsize_part = nsi < 2 ? PARTITION_SPLIT : PARTITION_VERT_A;
        break;
    case PART_VB:
        assert(nsi < SUB_PARTITIONS_AB);
        if (nsi) {
            *mi_col += hbs;
            *mi_row += nsi == 1 ? 0 : hbs;
        }
        sub_bsize_part = nsi == 0 ? PARTITION_VERT_B : PARTITION_SPLIT;
        break;
    case PART_H4:
        assert(nsi < SUB_PARTITIONS_PART4);
        *mi_row += nsi * quarter_step;
        sub_bsize_part = PARTITION_HORZ_4;
        break;
    case PART_V4:
        assert(nsi < SUB_PARTITIONS_PART4);
        *mi_col += nsi * quarter_step;
        sub_bsize_part = PARTITION_VERT_4;
        break;
    case PART_S:
        assert(nsi < SUB_PARTITIONS_SPLIT);
        *mi_col += (nsi & 1) * hbs;
        *mi_row += (nsi >> 1) * hbs;
        sub_bsize_part = PARTITION_SPLIT;
        break;
    default:
        assert(0 && "invalid shape");
    }
    return get_partition_subsize(bsize, sub_bsize_part);
}

static INLINE bool is_chroma_reference(int mi_row, int mi_col, BlockSize bsize, int ss_x, int ss_y) {
    const int bw = mi_size_wide[bsize];
    const int bh = mi_size_high[bsize];
    return ((mi_row & 0x01) || !(bh & 0x01) || !ss_y) &&
        ((mi_col & 0x01) || !(bw & 0x01) || !ss_x);
}

#ifdef __cplusplus
}
#endif
#endif //EbCommonUtils_h
