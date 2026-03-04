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
#include "common_utils.h"

const PredictionMode g_uv2y[16] = {
    DC_PRED, // UV_DC_PRED
    V_PRED, // UV_V_PRED
    H_PRED, // UV_H_PRED
    D45_PRED, // UV_D45_PRED
    D135_PRED, // UV_D135_PRED
    D113_PRED, // UV_D113_PRED
    D157_PRED, // UV_D157_PRED
    D203_PRED, // UV_D203_PRED
    D67_PRED, // UV_D67_PRED
    SMOOTH_PRED, // UV_SMOOTH_PRED
    SMOOTH_V_PRED, // UV_SMOOTH_V_PRED
    SMOOTH_H_PRED, // UV_SMOOTH_H_PRED
    PAETH_PRED, // UV_PAETH_PRED
    DC_PRED, // UV_CFL_PRED
    INTRA_INVALID, // UV_INTRA_MODES
    INTRA_INVALID, // UV_MODE_INVALID
};

const PredictionMode fimode_to_intradir[FILTER_INTRA_MODES] = {DC_PRED, V_PRED, H_PRED, D157_PRED, DC_PRED};

// AOMMIN(3, AOMMIN(b_width_log2(bsize), b_height_log2(bsize)))
const uint8_t eb_size_group_lookup[BLOCK_SIZES_ALL] = {0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3,
                                                       3, 3, 3, 3, 3, 0, 0, 1, 1, 2, 2};

const uint8_t eb_num_pels_log2_lookup[BLOCK_SIZES_ALL] = {4,  5,  5,  6,  7,  7, 8, 9, 9, 10, 11,
                                                          11, 12, 13, 13, 14, 6, 6, 8, 8, 10, 10};

// clang-format off
const TxSize  eb_max_txsize_lookup[BLOCK_SIZES_ALL] = {
    // 4X4
    TX_4X4,
    // 4X8,    8X4,      8X8
    TX_4X4,    TX_4X4,   TX_8X8,
    // 8X16,   16X8,     16X16
    TX_8X8,    TX_8X8,   TX_16X16,
    // 16X32,  32X16,    32X32
    TX_16X16,  TX_16X16, TX_32X32,
    // 32X64,  64X32,
    TX_32X32,  TX_32X32,
    // 64X64
    TX_64X64,
    // 64x128, 128x64,   128x128
    TX_64X64,  TX_64X64, TX_64X64,
    // 4x16,   16x4,     8x32
    TX_4X4,    TX_4X4,   TX_8X8,
    // 32x8,   16x64     64x16
    TX_8X8,    TX_16X16, TX_16X16 };
// clang-format on

// Transform block width in unit
const int32_t eb_tx_size_wide_unit[TX_SIZES_ALL] = {
    1, 2, 4, 8, 16, 1, 2, 2, 4, 4, 8, 8, 16, 1, 4, 2, 8, 4, 16,
};
// Transform block height in unit
const int32_t eb_tx_size_high_unit[TX_SIZES_ALL] = {
    1, 2, 4, 8, 16, 2, 1, 4, 2, 8, 4, 16, 8, 4, 1, 8, 2, 16, 4,
};

const TxSize eb_sub_tx_size_map[TX_SIZES_ALL] = {
    TX_4X4, // TX_4X4
    TX_4X4, // TX_8X8
    TX_8X8, // TX_16X16
    TX_16X16, // TX_32X32
    TX_32X32, // TX_64X64
    TX_4X4, // TX_4X8
    TX_4X4, // TX_8X4
    TX_8X8, // TX_8X16
    TX_8X8, // TX_16X8
    TX_16X16, // TX_16X32
    TX_16X16, // TX_32X16
    TX_32X32, // TX_32X64
    TX_32X32, // TX_64X32
    TX_4X8, // TX_4X16
    TX_8X4, // TX_16X4
    TX_8X16, // TX_8X32
    TX_16X8, // TX_32X8
    TX_16X32, // TX_16X64
    TX_32X16, // TX_64X16
};

const TxSize tx_depth_to_tx_size[3][BLOCK_SIZES_ALL] = {
    // tx_depth 0
    {TX_4X4,   TX_4X8,   TX_8X4,   TX_8X8,   TX_8X16,  TX_16X8,  TX_16X16,
     TX_16X32, TX_32X16, TX_32X32, TX_32X64, TX_64X32, TX_64X64,
     TX_64X64, //TX_64X128,
     TX_64X64, //TX_128X64,
     TX_64X64, //TX_128X128,
     TX_4X16,  TX_16X4,  TX_8X32,  TX_32X8,  TX_16X64, TX_64X16},
    // tx_depth 1:
    {TX_4X4,   TX_4X8,   TX_8X4,   TX_4X4,   TX_8X8,   TX_8X8,   TX_8X8,
     TX_16X16, TX_16X16, TX_16X16, TX_32X32, TX_32X32, TX_32X32,
     TX_64X64, //TX_64X128,
     TX_64X64, //TX_128X64,
     TX_64X64, //TX_128X128,
     TX_4X8,   TX_8X4,   TX_8X16,  TX_16X8,  TX_16X32, TX_32X16},
    // tx_depth 2
    {TX_4X4,   TX_4X8, TX_8X4, TX_8X8, TX_4X4,   TX_4X4,  TX_4X4, TX_8X8, TX_8X8, TX_8X8, TX_16X16, TX_16X16, TX_16X16,
     TX_64X64, //TX_64X128,
     TX_64X64, //TX_128X64,
     TX_64X64, //TX_128X128,
     TX_4X4,   TX_4X4, TX_8X8, TX_8X8, TX_16X16, TX_16X16}};
const int32_t tx_size_wide[TX_SIZES_ALL] = {
    4, 8, 16, 32, 64, 4, 8, 8, 16, 16, 32, 32, 64, 4, 16, 8, 32, 16, 64,
};
// Transform block height in pixels
const int32_t tx_size_high[TX_SIZES_ALL] = {
    4, 8, 16, 32, 64, 8, 4, 16, 8, 32, 16, 64, 32, 16, 4, 32, 8, 64, 16,
};

// Transform block width in log2
const int32_t tx_size_wide_log2[TX_SIZES_ALL] = {
    2, 3, 4, 5, 6, 2, 3, 3, 4, 4, 5, 5, 6, 2, 4, 3, 5, 4, 6,
};

// Transform block height in log2
const int32_t tx_size_high_log2[TX_SIZES_ALL] = {
    2, 3, 4, 5, 6, 3, 2, 4, 3, 5, 4, 6, 5, 4, 2, 5, 3, 6, 4,
};

const uint8_t intra_mode_context[INTRA_MODES] = {
    0,
    1,
    2,
    3,
    4,
    4,
    4,
    4,
    3,
    0,
    1,
    2,
    0,
};

const TxSize txsize_sqr_map[TX_SIZES_ALL] = {
    TX_4X4, // TX_4X4
    TX_8X8, // TX_8X8
    TX_16X16, // TX_16X16
    TX_32X32, // TX_32X32
    TX_64X64, // TX_64X64
    TX_4X4, // TX_4X8
    TX_4X4, // TX_8X4
    TX_8X8, // TX_8X16
    TX_8X8, // TX_16X8
    TX_16X16, // TX_16X32
    TX_16X16, // TX_32X16
    TX_32X32, // TX_32X64
    TX_32X32, // TX_64X32
    TX_4X4, // TX_4X16
    TX_4X4, // TX_16X4
    TX_8X8, // TX_8X32
    TX_8X8, // TX_32X8
    TX_16X16, // TX_16X64
    TX_16X16, // TX_64X16
};

const TxSize txsize_sqr_up_map[TX_SIZES_ALL] = {
    TX_4X4, // TX_4X4
    TX_8X8, // TX_8X8
    TX_16X16, // TX_16X16
    TX_32X32, // TX_32X32
    TX_64X64, // TX_64X64
    TX_8X8, // TX_4X8
    TX_8X8, // TX_8X4
    TX_16X16, // TX_8X16
    TX_16X16, // TX_16X8
    TX_32X32, // TX_16X32
    TX_32X32, // TX_32X16
    TX_64X64, // TX_32X64
    TX_64X64, // TX_64X32
    TX_16X16, // TX_4X16
    TX_16X16, // TX_16X4
    TX_32X32, // TX_8X32
    TX_32X32, // TX_32X8
    TX_64X64, // TX_16X64
    TX_64X64, // TX_64X16
};

// Number of transform types in each set type
const int32_t av1_num_ext_tx_set[EXT_TX_SET_TYPES] = {1, 2, 5, 7, 12, 16};

const int32_t av1_ext_tx_used[EXT_TX_SET_TYPES][TX_TYPES] = {
    {1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0},
    {1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0},
    {1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0},
    {1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0},
    {1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1},
};

const int32_t ext_tx_set_index[2][EXT_TX_SET_TYPES] = {
    {0, -1, 2, 1, -1, -1}, // Intra
    {0, 3, -1, -1, 2, 1} // Inter
};

const uint8_t mi_size_wide_log2[BLOCK_SIZES_ALL] = {0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 0, 2, 1, 3, 2, 4};
const uint8_t mi_size_high_log2[BLOCK_SIZES_ALL] = {0, 1, 0, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4, 5, 4, 5, 2, 0, 3, 1, 4, 2};

const TxSize blocksize_to_txsize[BLOCK_SIZES_ALL] = {
    TX_4X4, // BLOCK_4X4
    TX_4X8, // BLOCK_4X8
    TX_8X4, // BLOCK_8X4
    TX_8X8, // BLOCK_8X8
    TX_8X16, // BLOCK_8X16
    TX_16X8, // BLOCK_16X8
    TX_16X16, // BLOCK_16X16
    TX_16X32, // BLOCK_16X32
    TX_32X16, // BLOCK_32X16
    TX_32X32, // BLOCK_32X32
    TX_32X64, // BLOCK_32X64
    TX_64X32, // BLOCK_64X32
    TX_64X64, // BLOCK_64X64
    TX_64X64, // BLOCK_64X128
    TX_64X64, // BLOCK_128X64
    TX_64X64, // BLOCK_128X128
    TX_4X16, // BLOCK_4X16
    TX_16X4, // BLOCK_16X4
    TX_8X32, // BLOCK_8X32
    TX_32X8, // BLOCK_32X8
    TX_16X64, // BLOCK_16X64
    TX_64X16 // BLOCK_64X16
};

const BlockSize svt_aom_ss_size_lookup[BLOCK_SIZES_ALL][2][2] = {
    //  ss_x == 0    ss_x == 0        ss_x == 1      ss_x == 1
    //  ss_y == 0    ss_y == 1        ss_y == 0      ss_y == 1
    {{BLOCK_4X4, BLOCK_4X4}, {BLOCK_4X4, BLOCK_4X4}},
    {{BLOCK_4X8, BLOCK_4X4}, {BLOCK_INVALID, BLOCK_4X4}},
    {{BLOCK_8X4, BLOCK_INVALID}, {BLOCK_4X4, BLOCK_4X4}},
    {{BLOCK_8X8, BLOCK_8X4}, {BLOCK_4X8, BLOCK_4X4}},
    {{BLOCK_8X16, BLOCK_8X8}, {BLOCK_INVALID, BLOCK_4X8}},
    {{BLOCK_16X8, BLOCK_INVALID}, {BLOCK_8X8, BLOCK_8X4}},
    {{BLOCK_16X16, BLOCK_16X8}, {BLOCK_8X16, BLOCK_8X8}},
    {{BLOCK_16X32, BLOCK_16X16}, {BLOCK_INVALID, BLOCK_8X16}},
    {{BLOCK_32X16, BLOCK_INVALID}, {BLOCK_16X16, BLOCK_16X8}},
    {{BLOCK_32X32, BLOCK_32X16}, {BLOCK_16X32, BLOCK_16X16}},
    {{BLOCK_32X64, BLOCK_32X32}, {BLOCK_INVALID, BLOCK_16X32}},
    {{BLOCK_64X32, BLOCK_INVALID}, {BLOCK_32X32, BLOCK_32X16}},
    {{BLOCK_64X64, BLOCK_64X32}, {BLOCK_32X64, BLOCK_32X32}},
    {{BLOCK_64X128, BLOCK_64X64}, {BLOCK_INVALID, BLOCK_32X64}},
    {{BLOCK_128X64, BLOCK_INVALID}, {BLOCK_64X64, BLOCK_64X32}},
    {{BLOCK_128X128, BLOCK_128X64}, {BLOCK_64X128, BLOCK_64X64}},
    {{BLOCK_4X16, BLOCK_4X8}, {BLOCK_INVALID, BLOCK_4X8}},
    {{BLOCK_16X4, BLOCK_INVALID}, {BLOCK_8X4, BLOCK_8X4}},
    {{BLOCK_8X32, BLOCK_8X16}, {BLOCK_INVALID, BLOCK_4X16}},
    {{BLOCK_32X8, BLOCK_INVALID}, {BLOCK_16X8, BLOCK_16X4}},
    {{BLOCK_16X64, BLOCK_16X32}, {BLOCK_INVALID, BLOCK_8X32}},
    {{BLOCK_64X16, BLOCK_INVALID}, {BLOCK_32X16, BLOCK_32X8}}};

const uint8_t num_ns_per_shape[PART_S] = {1, 2, 2, 4, 4, 3, 3, 3, 3};

// gives the index offset (relative to SQ block) of the given nsq shape
// Different tables for 128x128 because H4/V4 are not allowed
const uint32_t ns_blk_offset_md[PART_S]     = {0, 1, 3, 5, 9, 13, 16, 19, 22};
const uint32_t ns_blk_offset_128_md[PART_S] = {0, 1, 3, 0 /*H4 not allowed*/, 0 /*V4 not allowed*/, 5, 8, 11, 14};

const PartitionType from_shape_to_part[EXT_PARTITION_TYPES] = {PARTITION_NONE,
                                                               PARTITION_HORZ,
                                                               PARTITION_VERT,
                                                               PARTITION_HORZ_4,
                                                               PARTITION_VERT_4,
                                                               PARTITION_HORZ_A,
                                                               PARTITION_HORZ_B,
                                                               PARTITION_VERT_A,
                                                               PARTITION_VERT_B,
                                                               PARTITION_SPLIT};
const Part          from_part_to_shape[PART_S + 1]          = {
    PART_N, PART_H, PART_V, PART_S, PART_HA, PART_HB, PART_VA, PART_VB, PART_H4, PART_V4};

// Width/height lookup tables in units of various block sizes
const uint8_t block_size_wide[BLOCK_SIZES_ALL] = {4,  4,  8,  8,   8,   16, 16, 16, 32, 32, 32,
                                                  64, 64, 64, 128, 128, 4,  16, 8,  32, 16, 64};

const uint8_t block_size_high[BLOCK_SIZES_ALL] = {4,  8,  4,   8,  16,  8,  16, 32, 16, 32, 64,
                                                  32, 64, 128, 64, 128, 16, 4,  32, 8,  64, 16};

const uint8_t mi_size_wide[BLOCK_SIZES_ALL] = {1, 1, 2, 2, 2, 4, 4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 1, 4, 2, 8, 4, 16};
const uint8_t mi_size_high[BLOCK_SIZES_ALL] = {1, 2, 1, 2, 4, 2, 4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 4, 1, 8, 2, 16, 4};

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
const BlockSize svt_aom_subsize_lookup[EXT_PARTITION_TYPES][SQR_BLOCK_SIZES] = {
  {     // PARTITION_NONE
    BLOCK_4X4, BLOCK_8X8, BLOCK_16X16,
    BLOCK_32X32, BLOCK_64X64, BLOCK_128X128
  }, {  // PARTITION_HORZ
    BLOCK_INVALID, BLOCK_8X4, BLOCK_16X8,
    BLOCK_32X16, BLOCK_64X32, BLOCK_128X64
  }, {  // PARTITION_VERT
    BLOCK_INVALID, BLOCK_4X8, BLOCK_8X16,
    BLOCK_16X32, BLOCK_32X64, BLOCK_64X128
  }, {  // PARTITION_SPLIT
    BLOCK_INVALID, BLOCK_4X4, BLOCK_8X8,
    BLOCK_16X16, BLOCK_32X32, BLOCK_64X64
  }, {  // PARTITION_HORZ_A
    BLOCK_INVALID, BLOCK_INVALID, BLOCK_16X8,
    BLOCK_32X16, BLOCK_64X32, BLOCK_128X64
  }, {  // PARTITION_HORZ_B
    BLOCK_INVALID, BLOCK_INVALID, BLOCK_16X8,
    BLOCK_32X16, BLOCK_64X32, BLOCK_128X64
  }, {  // PARTITION_VERT_A
    BLOCK_INVALID, BLOCK_INVALID, BLOCK_8X16,
    BLOCK_16X32, BLOCK_32X64, BLOCK_64X128
  }, {  // PARTITION_VERT_B
    BLOCK_INVALID, BLOCK_INVALID, BLOCK_8X16,
    BLOCK_16X32, BLOCK_32X64, BLOCK_64X128
  }, {  // PARTITION_HORZ_4
    BLOCK_INVALID, BLOCK_INVALID, BLOCK_16X4,
    BLOCK_32X8, BLOCK_64X16, BLOCK_INVALID
  }, {  // PARTITION_VERT_4
    BLOCK_INVALID, BLOCK_INVALID, BLOCK_4X16,
    BLOCK_8X32, BLOCK_16X64, BLOCK_INVALID
  }
};
