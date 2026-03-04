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

#include <stdlib.h>
#include "transforms.h"
#include "aom_dsp_rtcd.h"

const int8_t fwd_cos_bit_col[MAX_TXWH_IDX /*txw_idx*/][MAX_TXWH_IDX /*txh_idx*/] = {
    {13, 13, 13, 0, 0}, {13, 13, 13, 12, 0}, {13, 13, 13, 12, 13}, {0, 13, 13, 12, 13}, {0, 0, 13, 12, 13}};
const int8_t fwd_cos_bit_row[MAX_TXWH_IDX /*txw_idx*/][MAX_TXWH_IDX /*txh_idx*/] = {
    {13, 13, 12, 0, 0}, {13, 13, 13, 12, 0}, {13, 13, 12, 13, 12}, {0, 12, 13, 12, 11}, {0, 0, 12, 11, 10}};

const uint8_t tx_blocks_per_depth[BLOCK_SIZES_ALL][MAX_VARTX_DEPTH + 1] = {
    {1, 1, 1}, // BLOCK_4X4
    {1, 1, 1}, // BLOCK_4X8
    {1, 1, 1}, // BLOCK_8X4
    {1, 4, 4}, // BLOCK_8X8
    {1, 2, 8}, // BLOCK_8X16
    {1, 2, 8}, // BLOCK_16X8
    {1, 4, 16}, // BLOCK_16X16
    {1, 2, 8}, // BLOCK_16X32
    {1, 2, 8}, // BLOCK_32X16
    {1, 4, 16}, // BLOCK_32X32
    {1, 2, 8}, // BLOCK_32X64
    {1, 2, 8}, // BLOCK_64X32
    {1, 4, 16}, // BLOCK_64X64
    {2, 2, 2}, // BLOCK_64X128
    {2, 2, 2}, // BLOCK_128X64
    {4, 4, 4}, // BLOCK_128X128
    {1, 2, 4}, // BLOCK_4X16
    {1, 2, 4}, // BLOCK_16X4
    {1, 2, 4}, // BLOCK_8X32
    {1, 2, 4}, // BLOCK_32X8
    {1, 2, 4}, // BLOCK_16X64
    {1, 2, 4} // BLOCK_64X16
};

// origin is block - separate tables for INTRA (idx 0) and INTER (idx 1) needed b/c of tx depth 2
const Position tx_org[BLOCK_SIZES_ALL][2 /*is_inter*/][MAX_VARTX_DEPTH + 1][MAX_TXB_COUNT] = {
    {// BLOCK_4X4
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0}},
      {// tx_depth 2
       {0, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0}},
      {// tx_depth 2
       {0, 0}}}},
    {// BLOCK_4X8
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0}},
      {// tx_depth 2
       {0, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0}},
      {// tx_depth 2
       {0, 0}}}},
    {// BLOCK_8X4
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0}},
      {// tx_depth 2
       {0, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0}},
      {// tx_depth 2
       {0, 0}}}},
    {// BLOCK_8X8
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {4, 0},
       {0, 4},
       {4, 4}},
      {
          // tx_depth 2
          {0, 0} // not allowed
      }},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {4, 0},
       {0, 4},
       {4, 4}},
      {
          // tx_depth 2
          {0, 0} // not allowed
      }}},
    {// BLOCK_8X16
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 8}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {0, 4},
       {4, 4},
       {0, 8},
       {4, 8},
       {0, 12},
       {4, 12}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 8}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {0, 4},
       {4, 4},
       {0, 8},
       {4, 8},
       {0, 12},
       {4, 12}}}},
    {// BLOCK_16X8
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {8, 0}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {8, 0},
       {12, 0},
       {0, 4},
       {4, 4},
       {8, 4},
       {12, 4}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {8, 0}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {0, 4},
       {4, 4},
       {8, 0},
       {12, 0},
       {8, 4},
       {12, 4}}}},
    {// BLOCK_16X16
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {8, 0},
       {0, 8},
       {8, 8}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {8, 0},
       {12, 0},
       {0, 4},
       {4, 4},
       {8, 4},
       {12, 4},
       {0, 8},
       {4, 8},
       {8, 8},
       {12, 8},
       {0, 12},
       {4, 12},
       {8, 12},
       {12, 12}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {8, 0},
       {0, 8},
       {8, 8}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {0, 4},
       {4, 4},
       {8, 0},
       {12, 0},
       {8, 4},
       {12, 4},
       {0, 8},
       {4, 8},
       {0, 12},
       {4, 12},
       {8, 8},
       {12, 8},
       {8, 12},
       {12, 12}}}},
    {// BLOCK_16X32
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 16}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {0, 8},
       {8, 8},
       {0, 16},
       {8, 16},
       {0, 24},
       {8, 24}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 16}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {0, 8},
       {8, 8},
       {0, 16},
       {8, 16},
       {0, 24},
       {8, 24}}}},
    {// BLOCK_32X16
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {16, 0}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {16, 0},
       {24, 0},
       {0, 8},
       {8, 8},
       {16, 8},
       {24, 8}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {16, 0}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {0, 8},
       {8, 8},
       {16, 0},
       {24, 0},
       {16, 8},
       {24, 8}}}},
    {// BLOCK_32X32
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {16, 0},
       {0, 16},
       {16, 16}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {16, 0},
       {24, 0},
       {0, 8},
       {8, 8},
       {16, 8},
       {24, 8},
       {0, 16},
       {8, 16},
       {16, 16},
       {24, 16},
       {0, 24},
       {8, 24},
       {16, 24},
       {24, 24}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {16, 0},
       {0, 16},
       {16, 16}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {0, 8},
       {8, 8},
       {16, 0},
       {24, 0},
       {16, 8},
       {24, 8},
       {0, 16},
       {8, 16},
       {0, 24},
       {8, 24},
       {16, 16},
       {24, 16},
       {16, 24},
       {24, 24}}}},
    {// BLOCK_32X64
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 32}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {0, 16},
       {16, 16},
       {0, 32},
       {16, 32},
       {0, 48},
       {16, 48}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 32}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {0, 16},
       {16, 16},
       {0, 32},
       {16, 32},
       {0, 48},
       {16, 48}}}},
    {// BLOCK_64X32
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {32, 0}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {32, 0},
       {48, 0},
       {0, 16},
       {16, 16},
       {32, 16},
       {48, 16}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {32, 0}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {0, 16},
       {16, 16},
       {32, 0},
       {48, 0},
       {32, 16},
       {48, 16}}}},
    {// BLOCK_64X64
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {32, 0},
       {0, 32},
       {32, 32}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {32, 0},
       {48, 0},
       {0, 16},
       {16, 16},
       {32, 16},
       {48, 16},
       {0, 32},
       {16, 32},
       {32, 32},
       {48, 32},
       {0, 48},
       {16, 48},
       {32, 48},
       {48, 48}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {32, 0},
       {0, 32},
       {32, 32}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {0, 16},
       {16, 16},
       {32, 0},
       {48, 0},
       {32, 16},
       {48, 16},
       {0, 32},
       {16, 32},
       {0, 48},
       {16, 48},
       {32, 32},
       {48, 32},
       {32, 48},
       {48, 48}}}},
    {// BLOCK_64X128
     {// intra
      {// tx_depth 0
       {0, 0},
       {0, 64}},
      {// tx_depth 1
       {0, 0},
       {0, 64}},
      {// tx_depth 2
       {0, 0},
       {0, 64}}},
     {// inter
      {// tx_depth 0
       {0, 0},
       {0, 64}},
      {// tx_depth 1
       {0, 0},
       {0, 64}},
      {// tx_depth 2
       {0, 0},
       {0, 64}}}},
    {// BLOCK_128X64
     {// intra
      {// tx_depth 0
       {0, 0},
       {64, 0}},
      {// tx_depth 1
       {0, 0},
       {64, 0}},
      {// tx_depth 2
       {0, 0},
       {64, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0},
       {64, 0}},
      {// tx_depth 1
       {0, 0},
       {64, 0}},
      {// tx_depth 2
       {0, 0},
       {64, 0}}}},
    {// BLOCK_128X128
     {// intra
      {// tx_depth 0
       {0, 0},
       {64, 0},
       {0, 64},
       {64, 64}},
      {// tx_depth 1
       {0, 0},
       {64, 0},
       {0, 64},
       {64, 64}},
      {// tx_depth 2
       {0, 0},
       {64, 0},
       {0, 64},
       {64, 64}}},
     {// inter
      {// tx_depth 0
       {0, 0},
       {64, 0},
       {0, 64},
       {64, 64}},
      {// tx_depth 1
       {0, 0},
       {64, 0},
       {0, 64},
       {64, 64}},
      {// tx_depth 2
       {0, 0},
       {64, 0},
       {0, 64},
       {64, 64}}}},
    {// BLOCK_4X16
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 8}},
      {// tx_depth 2
       {0, 0},
       {0, 4},
       {0, 8},
       {0, 12}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 8}},
      {// tx_depth 2
       {0, 0},
       {0, 4},
       {0, 8},
       {0, 12}}}},
    {// BLOCK_16X4
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {8, 0}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {8, 0},
       {12, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {8, 0}},
      {// tx_depth 2
       {0, 0},
       {4, 0},
       {8, 0},
       {12, 0}}}},
    {// BLOCK_8X32
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 16}},
      {// tx_depth 2
       {0, 0},
       {0, 8},
       {0, 16},
       {0, 24}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 16}},
      {// tx_depth 2
       {0, 0},
       {0, 8},
       {0, 16},
       {0, 24}}}},
    {// BLOCK_32X8
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {16, 0}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {16, 0},
       {24, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {16, 0}},
      {// tx_depth 2
       {0, 0},
       {8, 0},
       {16, 0},
       {24, 0}}}},
    {// BLOCK_16X64
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 32}},
      {// tx_depth 2
       {0, 0},
       {0, 16},
       {0, 32},
       {0, 48}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {0, 32}},
      {// tx_depth 2
       {0, 0},
       {0, 16},
       {0, 32},
       {0, 48}}}},
    {// BLOCK_64X16
     {// intra
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {32, 0}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {32, 0},
       {48, 0}}},
     {// inter
      {// tx_depth 0
       {0, 0}},
      {// tx_depth 1
       {0, 0},
       {32, 0}},
      {// tx_depth 2
       {0, 0},
       {16, 0},
       {32, 0},
       {48, 0}}}}};

static const int8_t fdct4_range_mult2[4]    = {0, 2, 3, 3};
static const int8_t fdct8_range_mult2[6]    = {0, 2, 4, 5, 5, 5};
static const int8_t fdct16_range_mult2[8]   = {0, 2, 4, 6, 7, 7, 7, 7};
static const int8_t fdct32_range_mult2[10]  = {0, 2, 4, 6, 8, 9, 9, 9, 9, 9};
static const int8_t fdct64_range_mult2[12]  = {0, 2, 4, 6, 8, 10, 11, 11, 11, 11, 11, 11};
static const int8_t fadst4_range_mult2[7]   = {0, 2, 4, 3, 3, 3, 3};
static const int8_t fadst8_range_mult2[8]   = {0, 0, 1, 3, 3, 5, 5, 5};
static const int8_t fadst16_range_mult2[10] = {0, 0, 1, 3, 3, 5, 5, 7, 7, 7};
static const int8_t fadst32_range_mult2[12] = {0, 0, 1, 3, 3, 5, 5, 7, 7, 9, 9, 9};
static const int8_t fidtx4_range_mult2[1]   = {1};
static const int8_t fidtx8_range_mult2[1]   = {2};
static const int8_t fidtx16_range_mult2[1]  = {3};
static const int8_t fidtx32_range_mult2[1]  = {4};
static const int8_t fidtx64_range_mult2[1]  = {5};

static const int8_t* fwd_txfm_range_mult2_list[TXFM_TYPES] = {fdct4_range_mult2,
                                                              fdct8_range_mult2,
                                                              fdct16_range_mult2,
                                                              fdct32_range_mult2,
                                                              fdct64_range_mult2,
                                                              fadst4_range_mult2,
                                                              fadst8_range_mult2,
                                                              fadst16_range_mult2,
                                                              fadst32_range_mult2,
                                                              fidtx4_range_mult2,
                                                              fidtx8_range_mult2,
                                                              fidtx16_range_mult2,
                                                              fidtx32_range_mult2,
                                                              fidtx64_range_mult2};

static const int8_t fwd_shift_4x4[3]   = {2, 0, 0};
static const int8_t fwd_shift_8x8[3]   = {2, -1, 0};
static const int8_t fwd_shift_16x16[3] = {2, -2, 0};
static const int8_t fwd_shift_32x32[3] = {2, -4, 0};
static const int8_t fwd_shift_64x64[3] = {0, -2, -2};
static const int8_t fwd_shift_4x8[3]   = {2, -1, 0};
static const int8_t fwd_shift_8x4[3]   = {2, -1, 0};
static const int8_t fwd_shift_8x16[3]  = {2, -2, 0};
static const int8_t fwd_shift_16x8[3]  = {2, -2, 0};
static const int8_t fwd_shift_16x32[3] = {2, -4, 0};
static const int8_t fwd_shift_32x16[3] = {2, -4, 0};
static const int8_t fwd_shift_32x64[3] = {0, -2, -2};
static const int8_t fwd_shift_64x32[3] = {2, -4, -2};
static const int8_t fwd_shift_4x16[3]  = {2, -1, 0};
static const int8_t fwd_shift_16x4[3]  = {2, -1, 0};
static const int8_t fwd_shift_8x32[3]  = {2, -2, 0};
static const int8_t fwd_shift_32x8[3]  = {2, -2, 0};
static const int8_t fwd_shift_16x64[3] = {0, -2, 0};
static const int8_t fwd_shift_64x16[3] = {2, -4, 0};

const int8_t* fwd_txfm_shift_ls[TX_SIZES_ALL] = {
    fwd_shift_4x4,  fwd_shift_8x8,  fwd_shift_16x16, fwd_shift_32x32, fwd_shift_64x64, fwd_shift_4x8,   fwd_shift_8x4,
    fwd_shift_8x16, fwd_shift_16x8, fwd_shift_16x32, fwd_shift_32x16, fwd_shift_32x64, fwd_shift_64x32, fwd_shift_4x16,
    fwd_shift_16x4, fwd_shift_8x32, fwd_shift_32x8,  fwd_shift_16x64, fwd_shift_64x16,
};

void svt_av1_gen_fwd_stage_range(int8_t* stage_range_col, int8_t* stage_range_row, const Txfm2dFlipCfg* cfg,
                                 int32_t bd) {
    // Take the shift from the larger dimension in the rectangular case.
    const int8_t* shift = cfg->shift;
    // i < MAX_TXFM_STAGE_NUM will mute above array bounds warning
    for (int32_t i = 0; i < cfg->stage_num_col && i < MAX_TXFM_STAGE_NUM; ++i) {
        stage_range_col[i] = (int8_t)(cfg->stage_range_col[i] + shift[0] + bd + 1);
    }
    // i < MAX_TXFM_STAGE_NUM will mute above array bounds warning
    for (int32_t i = 0; i < cfg->stage_num_row && i < MAX_TXFM_STAGE_NUM; ++i) {
        stage_range_row[i] = (int8_t)(cfg->stage_range_row[i] + shift[0] + shift[1] + bd + 1);
    }
}

void svt_av1_fdct4_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[4];

    // stage 0;

    // stage 1;
    bf1    = output;
    bf1[0] = input[0] + input[3];
    bf1[1] = input[1] + input[2];
    bf1[2] = -input[2] + input[1];
    bf1[3] = -input[3] + input[0];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1] = half_btf(-cospi[32], bf0[1], cospi[32], bf0[0], cos_bit);
    bf1[2] = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[48], bf0[3], -cospi[16], bf0[2], cos_bit);

    // stage 3
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[2];
    bf1[2] = bf0[1];
    bf1[3] = bf0[3];
}

void svt_av1_fdct8_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    bf1    = output;
    bf1[0] = input[0] + input[7];
    bf1[1] = input[1] + input[6];
    bf1[2] = input[2] + input[5];
    bf1[3] = input[3] + input[4];
    bf1[4] = -input[4] + input[3];
    bf1[5] = -input[5] + input[2];
    bf1[6] = -input[6] + input[1];
    bf1[7] = -input[7] + input[0];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0] + bf0[3];
    bf1[1] = bf0[1] + bf0[2];
    bf1[2] = -bf0[2] + bf0[1];
    bf1[3] = -bf0[3] + bf0[0];
    bf1[4] = bf0[4];
    bf1[5] = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7] = bf0[7];

    // stage 3
    cospi  = cospi_arr(cos_bit);
    bf0    = step;
    bf1    = output;
    bf1[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1] = half_btf(-cospi[32], bf0[1], cospi[32], bf0[0], cos_bit);
    bf1[2] = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[48], bf0[3], -cospi[16], bf0[2], cos_bit);
    bf1[4] = bf0[4] + bf0[5];
    bf1[5] = -bf0[5] + bf0[4];
    bf1[6] = -bf0[6] + bf0[7];
    bf1[7] = bf0[7] + bf0[6];

    // stage 4
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = bf0[2];
    bf1[3] = bf0[3];
    bf1[4] = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[5] = half_btf(cospi[24], bf0[5], cospi[40], bf0[6], cos_bit);
    bf1[6] = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[7] = half_btf(cospi[56], bf0[7], -cospi[8], bf0[4], cos_bit);

    // stage 5
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[4];
    bf1[2] = bf0[2];
    bf1[3] = bf0[6];
    bf1[4] = bf0[1];
    bf1[5] = bf0[5];
    bf1[6] = bf0[3];
    bf1[7] = bf0[7];
}

void svt_av1_fdct16_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[15];
    bf1[1]  = input[1] + input[14];
    bf1[2]  = input[2] + input[13];
    bf1[3]  = input[3] + input[12];
    bf1[4]  = input[4] + input[11];
    bf1[5]  = input[5] + input[10];
    bf1[6]  = input[6] + input[9];
    bf1[7]  = input[7] + input[8];
    bf1[8]  = -input[8] + input[7];
    bf1[9]  = -input[9] + input[6];
    bf1[10] = -input[10] + input[5];
    bf1[11] = -input[11] + input[4];
    bf1[12] = -input[12] + input[3];
    bf1[13] = -input[13] + input[2];
    bf1[14] = -input[14] + input[1];
    bf1[15] = -input[15] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[2]  = -bf0[2] + bf0[1];
    bf1[3]  = -bf0[3] + bf0[0];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1]  = half_btf(-cospi[32], bf0[1], cospi[32], bf0[0], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[48], bf0[3], -cospi[16], bf0[2], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[5]  = -bf0[5] + bf0[4];
    bf1[6]  = -bf0[6] + bf0[7];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[5]  = half_btf(cospi[24], bf0[5], cospi[40], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[7]  = half_btf(cospi[56], bf0[7], -cospi[8], bf0[4], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[9]  = -bf0[9] + bf0[8];
    bf1[10] = -bf0[10] + bf0[11];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[13] = -bf0[13] + bf0[12];
    bf1[14] = -bf0[14] + bf0[15];
    bf1[15] = bf0[15] + bf0[14];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[9]  = half_btf(cospi[28], bf0[9], cospi[36], bf0[14], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], cospi[20], bf0[13], cos_bit);
    bf1[11] = half_btf(cospi[12], bf0[11], cospi[52], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[44], bf0[13], -cospi[20], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[28], bf0[14], -cospi[36], bf0[9], cos_bit);
    bf1[15] = half_btf(cospi[60], bf0[15], -cospi[4], bf0[8], cos_bit);

    // stage 7
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[8];
    bf1[2]  = bf0[4];
    bf1[3]  = bf0[12];
    bf1[4]  = bf0[2];
    bf1[5]  = bf0[10];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[14];
    bf1[8]  = bf0[1];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[5];
    bf1[11] = bf0[13];
    bf1[12] = bf0[3];
    bf1[13] = bf0[11];
    bf1[14] = bf0[7];
    bf1[15] = bf0[15];
}

void svt_av1_fdct32_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[32];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[31];
    bf1[1]  = input[1] + input[30];
    bf1[2]  = input[2] + input[29];
    bf1[3]  = input[3] + input[28];
    bf1[4]  = input[4] + input[27];
    bf1[5]  = input[5] + input[26];
    bf1[6]  = input[6] + input[25];
    bf1[7]  = input[7] + input[24];
    bf1[8]  = input[8] + input[23];
    bf1[9]  = input[9] + input[22];
    bf1[10] = input[10] + input[21];
    bf1[11] = input[11] + input[20];
    bf1[12] = input[12] + input[19];
    bf1[13] = input[13] + input[18];
    bf1[14] = input[14] + input[17];
    bf1[15] = input[15] + input[16];
    bf1[16] = -input[16] + input[15];
    bf1[17] = -input[17] + input[14];
    bf1[18] = -input[18] + input[13];
    bf1[19] = -input[19] + input[12];
    bf1[20] = -input[20] + input[11];
    bf1[21] = -input[21] + input[10];
    bf1[22] = -input[22] + input[9];
    bf1[23] = -input[23] + input[8];
    bf1[24] = -input[24] + input[7];
    bf1[25] = -input[25] + input[6];
    bf1[26] = -input[26] + input[5];
    bf1[27] = -input[27] + input[4];
    bf1[28] = -input[28] + input[3];
    bf1[29] = -input[29] + input[2];
    bf1[30] = -input[30] + input[1];
    bf1[31] = -input[31] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[15];
    bf1[1]  = bf0[1] + bf0[14];
    bf1[2]  = bf0[2] + bf0[13];
    bf1[3]  = bf0[3] + bf0[12];
    bf1[4]  = bf0[4] + bf0[11];
    bf1[5]  = bf0[5] + bf0[10];
    bf1[6]  = bf0[6] + bf0[9];
    bf1[7]  = bf0[7] + bf0[8];
    bf1[8]  = -bf0[8] + bf0[7];
    bf1[9]  = -bf0[9] + bf0[6];
    bf1[10] = -bf0[10] + bf0[5];
    bf1[11] = -bf0[11] + bf0[4];
    bf1[12] = -bf0[12] + bf0[3];
    bf1[13] = -bf0[13] + bf0[2];
    bf1[14] = -bf0[14] + bf0[1];
    bf1[15] = -bf0[15] + bf0[0];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[24], cospi[32], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[25], cospi[32], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[27], cospi[32], bf0[20], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[23];
    bf1[17] = bf0[17] + bf0[22];
    bf1[18] = bf0[18] + bf0[21];
    bf1[19] = bf0[19] + bf0[20];
    bf1[20] = -bf0[20] + bf0[19];
    bf1[21] = -bf0[21] + bf0[18];
    bf1[22] = -bf0[22] + bf0[17];
    bf1[23] = -bf0[23] + bf0[16];
    bf1[24] = -bf0[24] + bf0[31];
    bf1[25] = -bf0[25] + bf0[30];
    bf1[26] = -bf0[26] + bf0[29];
    bf1[27] = -bf0[27] + bf0[28];
    bf1[28] = bf0[28] + bf0[27];
    bf1[29] = bf0[29] + bf0[26];
    bf1[30] = bf0[30] + bf0[25];
    bf1[31] = bf0[31] + bf0[24];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[2]  = -bf0[2] + bf0[1];
    bf1[3]  = -bf0[3] + bf0[0];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(-cospi[16], bf0[18], cospi[48], bf0[29], cos_bit);
    bf1[19] = half_btf(-cospi[16], bf0[19], cospi[48], bf0[28], cos_bit);
    bf1[20] = half_btf(-cospi[48], bf0[20], -cospi[16], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[48], bf0[21], -cospi[16], bf0[26], cos_bit);
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[48], bf0[26], -cospi[16], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[48], bf0[27], -cospi[16], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[16], bf0[29], cospi[48], bf0[18], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1]  = half_btf(-cospi[32], bf0[1], cospi[32], bf0[0], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[48], bf0[3], -cospi[16], bf0[2], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[5]  = -bf0[5] + bf0[4];
    bf1[6]  = -bf0[6] + bf0[7];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[19];
    bf1[17] = bf0[17] + bf0[18];
    bf1[18] = -bf0[18] + bf0[17];
    bf1[19] = -bf0[19] + bf0[16];
    bf1[20] = -bf0[20] + bf0[23];
    bf1[21] = -bf0[21] + bf0[22];
    bf1[22] = bf0[22] + bf0[21];
    bf1[23] = bf0[23] + bf0[20];
    bf1[24] = bf0[24] + bf0[27];
    bf1[25] = bf0[25] + bf0[26];
    bf1[26] = -bf0[26] + bf0[25];
    bf1[27] = -bf0[27] + bf0[24];
    bf1[28] = -bf0[28] + bf0[31];
    bf1[29] = -bf0[29] + bf0[30];
    bf1[30] = bf0[30] + bf0[29];
    bf1[31] = bf0[31] + bf0[28];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[5]  = half_btf(cospi[24], bf0[5], cospi[40], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[7]  = half_btf(cospi[56], bf0[7], -cospi[8], bf0[4], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[9]  = -bf0[9] + bf0[8];
    bf1[10] = -bf0[10] + bf0[11];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[13] = -bf0[13] + bf0[12];
    bf1[14] = -bf0[14] + bf0[15];
    bf1[15] = bf0[15] + bf0[14];
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(cospi[24], bf0[25], -cospi[40], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[21], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(cospi[56], bf0[29], -cospi[8], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[8], bf0[30], cospi[56], bf0[17], cos_bit);
    bf1[31] = bf0[31];

    // stage 7
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[9]  = half_btf(cospi[28], bf0[9], cospi[36], bf0[14], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], cospi[20], bf0[13], cos_bit);
    bf1[11] = half_btf(cospi[12], bf0[11], cospi[52], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[44], bf0[13], -cospi[20], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[28], bf0[14], -cospi[36], bf0[9], cos_bit);
    bf1[15] = half_btf(cospi[60], bf0[15], -cospi[4], bf0[8], cos_bit);
    bf1[16] = bf0[16] + bf0[17];
    bf1[17] = -bf0[17] + bf0[16];
    bf1[18] = -bf0[18] + bf0[19];
    bf1[19] = bf0[19] + bf0[18];
    bf1[20] = bf0[20] + bf0[21];
    bf1[21] = -bf0[21] + bf0[20];
    bf1[22] = -bf0[22] + bf0[23];
    bf1[23] = bf0[23] + bf0[22];
    bf1[24] = bf0[24] + bf0[25];
    bf1[25] = -bf0[25] + bf0[24];
    bf1[26] = -bf0[26] + bf0[27];
    bf1[27] = bf0[27] + bf0[26];
    bf1[28] = bf0[28] + bf0[29];
    bf1[29] = -bf0[29] + bf0[28];
    bf1[30] = -bf0[30] + bf0[31];
    bf1[31] = bf0[31] + bf0[30];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = half_btf(cospi[62], bf0[16], cospi[2], bf0[31], cos_bit);
    bf1[17] = half_btf(cospi[30], bf0[17], cospi[34], bf0[30], cos_bit);
    bf1[18] = half_btf(cospi[46], bf0[18], cospi[18], bf0[29], cos_bit);
    bf1[19] = half_btf(cospi[14], bf0[19], cospi[50], bf0[28], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], cospi[10], bf0[27], cos_bit);
    bf1[21] = half_btf(cospi[22], bf0[21], cospi[42], bf0[26], cos_bit);
    bf1[22] = half_btf(cospi[38], bf0[22], cospi[26], bf0[25], cos_bit);
    bf1[23] = half_btf(cospi[6], bf0[23], cospi[58], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[6], bf0[24], -cospi[58], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[38], bf0[25], -cospi[26], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[22], bf0[26], -cospi[42], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[54], bf0[27], -cospi[10], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[14], bf0[28], -cospi[50], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[46], bf0[29], -cospi[18], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[30], bf0[30], -cospi[34], bf0[17], cos_bit);
    bf1[31] = half_btf(cospi[62], bf0[31], -cospi[2], bf0[16], cos_bit);

    // stage 9
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[16];
    bf1[2]  = bf0[8];
    bf1[3]  = bf0[24];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[20];
    bf1[6]  = bf0[12];
    bf1[7]  = bf0[28];
    bf1[8]  = bf0[2];
    bf1[9]  = bf0[18];
    bf1[10] = bf0[10];
    bf1[11] = bf0[26];
    bf1[12] = bf0[6];
    bf1[13] = bf0[22];
    bf1[14] = bf0[14];
    bf1[15] = bf0[30];
    bf1[16] = bf0[1];
    bf1[17] = bf0[17];
    bf1[18] = bf0[9];
    bf1[19] = bf0[25];
    bf1[20] = bf0[5];
    bf1[21] = bf0[21];
    bf1[22] = bf0[13];
    bf1[23] = bf0[29];
    bf1[24] = bf0[3];
    bf1[25] = bf0[19];
    bf1[26] = bf0[11];
    bf1[27] = bf0[27];
    bf1[28] = bf0[7];
    bf1[29] = bf0[23];
    bf1[30] = bf0[15];
    bf1[31] = bf0[31];
}

void svt_av1_fdct64_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[64];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[63];
    bf1[1]  = input[1] + input[62];
    bf1[2]  = input[2] + input[61];
    bf1[3]  = input[3] + input[60];
    bf1[4]  = input[4] + input[59];
    bf1[5]  = input[5] + input[58];
    bf1[6]  = input[6] + input[57];
    bf1[7]  = input[7] + input[56];
    bf1[8]  = input[8] + input[55];
    bf1[9]  = input[9] + input[54];
    bf1[10] = input[10] + input[53];
    bf1[11] = input[11] + input[52];
    bf1[12] = input[12] + input[51];
    bf1[13] = input[13] + input[50];
    bf1[14] = input[14] + input[49];
    bf1[15] = input[15] + input[48];
    bf1[16] = input[16] + input[47];
    bf1[17] = input[17] + input[46];
    bf1[18] = input[18] + input[45];
    bf1[19] = input[19] + input[44];
    bf1[20] = input[20] + input[43];
    bf1[21] = input[21] + input[42];
    bf1[22] = input[22] + input[41];
    bf1[23] = input[23] + input[40];
    bf1[24] = input[24] + input[39];
    bf1[25] = input[25] + input[38];
    bf1[26] = input[26] + input[37];
    bf1[27] = input[27] + input[36];
    bf1[28] = input[28] + input[35];
    bf1[29] = input[29] + input[34];
    bf1[30] = input[30] + input[33];
    bf1[31] = input[31] + input[32];
    bf1[32] = -input[32] + input[31];
    bf1[33] = -input[33] + input[30];
    bf1[34] = -input[34] + input[29];
    bf1[35] = -input[35] + input[28];
    bf1[36] = -input[36] + input[27];
    bf1[37] = -input[37] + input[26];
    bf1[38] = -input[38] + input[25];
    bf1[39] = -input[39] + input[24];
    bf1[40] = -input[40] + input[23];
    bf1[41] = -input[41] + input[22];
    bf1[42] = -input[42] + input[21];
    bf1[43] = -input[43] + input[20];
    bf1[44] = -input[44] + input[19];
    bf1[45] = -input[45] + input[18];
    bf1[46] = -input[46] + input[17];
    bf1[47] = -input[47] + input[16];
    bf1[48] = -input[48] + input[15];
    bf1[49] = -input[49] + input[14];
    bf1[50] = -input[50] + input[13];
    bf1[51] = -input[51] + input[12];
    bf1[52] = -input[52] + input[11];
    bf1[53] = -input[53] + input[10];
    bf1[54] = -input[54] + input[9];
    bf1[55] = -input[55] + input[8];
    bf1[56] = -input[56] + input[7];
    bf1[57] = -input[57] + input[6];
    bf1[58] = -input[58] + input[5];
    bf1[59] = -input[59] + input[4];
    bf1[60] = -input[60] + input[3];
    bf1[61] = -input[61] + input[2];
    bf1[62] = -input[62] + input[1];
    bf1[63] = -input[63] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[31];
    bf1[1]  = bf0[1] + bf0[30];
    bf1[2]  = bf0[2] + bf0[29];
    bf1[3]  = bf0[3] + bf0[28];
    bf1[4]  = bf0[4] + bf0[27];
    bf1[5]  = bf0[5] + bf0[26];
    bf1[6]  = bf0[6] + bf0[25];
    bf1[7]  = bf0[7] + bf0[24];
    bf1[8]  = bf0[8] + bf0[23];
    bf1[9]  = bf0[9] + bf0[22];
    bf1[10] = bf0[10] + bf0[21];
    bf1[11] = bf0[11] + bf0[20];
    bf1[12] = bf0[12] + bf0[19];
    bf1[13] = bf0[13] + bf0[18];
    bf1[14] = bf0[14] + bf0[17];
    bf1[15] = bf0[15] + bf0[16];
    bf1[16] = -bf0[16] + bf0[15];
    bf1[17] = -bf0[17] + bf0[14];
    bf1[18] = -bf0[18] + bf0[13];
    bf1[19] = -bf0[19] + bf0[12];
    bf1[20] = -bf0[20] + bf0[11];
    bf1[21] = -bf0[21] + bf0[10];
    bf1[22] = -bf0[22] + bf0[9];
    bf1[23] = -bf0[23] + bf0[8];
    bf1[24] = -bf0[24] + bf0[7];
    bf1[25] = -bf0[25] + bf0[6];
    bf1[26] = -bf0[26] + bf0[5];
    bf1[27] = -bf0[27] + bf0[4];
    bf1[28] = -bf0[28] + bf0[3];
    bf1[29] = -bf0[29] + bf0[2];
    bf1[30] = -bf0[30] + bf0[1];
    bf1[31] = -bf0[31] + bf0[0];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = bf0[34];
    bf1[35] = bf0[35];
    bf1[36] = bf0[36];
    bf1[37] = bf0[37];
    bf1[38] = bf0[38];
    bf1[39] = bf0[39];
    bf1[40] = half_btf(-cospi[32], bf0[40], cospi[32], bf0[55], cos_bit);
    bf1[41] = half_btf(-cospi[32], bf0[41], cospi[32], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[32], bf0[42], cospi[32], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[32], bf0[43], cospi[32], bf0[52], cos_bit);
    bf1[44] = half_btf(-cospi[32], bf0[44], cospi[32], bf0[51], cos_bit);
    bf1[45] = half_btf(-cospi[32], bf0[45], cospi[32], bf0[50], cos_bit);
    bf1[46] = half_btf(-cospi[32], bf0[46], cospi[32], bf0[49], cos_bit);
    bf1[47] = half_btf(-cospi[32], bf0[47], cospi[32], bf0[48], cos_bit);
    bf1[48] = half_btf(cospi[32], bf0[48], cospi[32], bf0[47], cos_bit);
    bf1[49] = half_btf(cospi[32], bf0[49], cospi[32], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[32], bf0[50], cospi[32], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[32], bf0[51], cospi[32], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[32], bf0[52], cospi[32], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[32], bf0[53], cospi[32], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[32], bf0[54], cospi[32], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[32], bf0[55], cospi[32], bf0[40], cos_bit);
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = bf0[58];
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[15];
    bf1[1]  = bf0[1] + bf0[14];
    bf1[2]  = bf0[2] + bf0[13];
    bf1[3]  = bf0[3] + bf0[12];
    bf1[4]  = bf0[4] + bf0[11];
    bf1[5]  = bf0[5] + bf0[10];
    bf1[6]  = bf0[6] + bf0[9];
    bf1[7]  = bf0[7] + bf0[8];
    bf1[8]  = -bf0[8] + bf0[7];
    bf1[9]  = -bf0[9] + bf0[6];
    bf1[10] = -bf0[10] + bf0[5];
    bf1[11] = -bf0[11] + bf0[4];
    bf1[12] = -bf0[12] + bf0[3];
    bf1[13] = -bf0[13] + bf0[2];
    bf1[14] = -bf0[14] + bf0[1];
    bf1[15] = -bf0[15] + bf0[0];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[24], cospi[32], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[25], cospi[32], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[27], cospi[32], bf0[20], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[47];
    bf1[33] = bf0[33] + bf0[46];
    bf1[34] = bf0[34] + bf0[45];
    bf1[35] = bf0[35] + bf0[44];
    bf1[36] = bf0[36] + bf0[43];
    bf1[37] = bf0[37] + bf0[42];
    bf1[38] = bf0[38] + bf0[41];
    bf1[39] = bf0[39] + bf0[40];
    bf1[40] = -bf0[40] + bf0[39];
    bf1[41] = -bf0[41] + bf0[38];
    bf1[42] = -bf0[42] + bf0[37];
    bf1[43] = -bf0[43] + bf0[36];
    bf1[44] = -bf0[44] + bf0[35];
    bf1[45] = -bf0[45] + bf0[34];
    bf1[46] = -bf0[46] + bf0[33];
    bf1[47] = -bf0[47] + bf0[32];
    bf1[48] = -bf0[48] + bf0[63];
    bf1[49] = -bf0[49] + bf0[62];
    bf1[50] = -bf0[50] + bf0[61];
    bf1[51] = -bf0[51] + bf0[60];
    bf1[52] = -bf0[52] + bf0[59];
    bf1[53] = -bf0[53] + bf0[58];
    bf1[54] = -bf0[54] + bf0[57];
    bf1[55] = -bf0[55] + bf0[56];
    bf1[56] = bf0[56] + bf0[55];
    bf1[57] = bf0[57] + bf0[54];
    bf1[58] = bf0[58] + bf0[53];
    bf1[59] = bf0[59] + bf0[52];
    bf1[60] = bf0[60] + bf0[51];
    bf1[61] = bf0[61] + bf0[50];
    bf1[62] = bf0[62] + bf0[49];
    bf1[63] = bf0[63] + bf0[48];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[23];
    bf1[17] = bf0[17] + bf0[22];
    bf1[18] = bf0[18] + bf0[21];
    bf1[19] = bf0[19] + bf0[20];
    bf1[20] = -bf0[20] + bf0[19];
    bf1[21] = -bf0[21] + bf0[18];
    bf1[22] = -bf0[22] + bf0[17];
    bf1[23] = -bf0[23] + bf0[16];
    bf1[24] = -bf0[24] + bf0[31];
    bf1[25] = -bf0[25] + bf0[30];
    bf1[26] = -bf0[26] + bf0[29];
    bf1[27] = -bf0[27] + bf0[28];
    bf1[28] = bf0[28] + bf0[27];
    bf1[29] = bf0[29] + bf0[26];
    bf1[30] = bf0[30] + bf0[25];
    bf1[31] = bf0[31] + bf0[24];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = bf0[34];
    bf1[35] = bf0[35];
    bf1[36] = half_btf(-cospi[16], bf0[36], cospi[48], bf0[59], cos_bit);
    bf1[37] = half_btf(-cospi[16], bf0[37], cospi[48], bf0[58], cos_bit);
    bf1[38] = half_btf(-cospi[16], bf0[38], cospi[48], bf0[57], cos_bit);
    bf1[39] = half_btf(-cospi[16], bf0[39], cospi[48], bf0[56], cos_bit);
    bf1[40] = half_btf(-cospi[48], bf0[40], -cospi[16], bf0[55], cos_bit);
    bf1[41] = half_btf(-cospi[48], bf0[41], -cospi[16], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[48], bf0[42], -cospi[16], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[48], bf0[43], -cospi[16], bf0[52], cos_bit);
    bf1[44] = bf0[44];
    bf1[45] = bf0[45];
    bf1[46] = bf0[46];
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = bf0[49];
    bf1[50] = bf0[50];
    bf1[51] = bf0[51];
    bf1[52] = half_btf(cospi[48], bf0[52], -cospi[16], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[48], bf0[53], -cospi[16], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[48], bf0[54], -cospi[16], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[48], bf0[55], -cospi[16], bf0[40], cos_bit);
    bf1[56] = half_btf(cospi[16], bf0[56], cospi[48], bf0[39], cos_bit);
    bf1[57] = half_btf(cospi[16], bf0[57], cospi[48], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[16], bf0[58], cospi[48], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[16], bf0[59], cospi[48], bf0[36], cos_bit);
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[2]  = -bf0[2] + bf0[1];
    bf1[3]  = -bf0[3] + bf0[0];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(-cospi[16], bf0[18], cospi[48], bf0[29], cos_bit);
    bf1[19] = half_btf(-cospi[16], bf0[19], cospi[48], bf0[28], cos_bit);
    bf1[20] = half_btf(-cospi[48], bf0[20], -cospi[16], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[48], bf0[21], -cospi[16], bf0[26], cos_bit);
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[48], bf0[26], -cospi[16], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[48], bf0[27], -cospi[16], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[16], bf0[29], cospi[48], bf0[18], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[39];
    bf1[33] = bf0[33] + bf0[38];
    bf1[34] = bf0[34] + bf0[37];
    bf1[35] = bf0[35] + bf0[36];
    bf1[36] = -bf0[36] + bf0[35];
    bf1[37] = -bf0[37] + bf0[34];
    bf1[38] = -bf0[38] + bf0[33];
    bf1[39] = -bf0[39] + bf0[32];
    bf1[40] = -bf0[40] + bf0[47];
    bf1[41] = -bf0[41] + bf0[46];
    bf1[42] = -bf0[42] + bf0[45];
    bf1[43] = -bf0[43] + bf0[44];
    bf1[44] = bf0[44] + bf0[43];
    bf1[45] = bf0[45] + bf0[42];
    bf1[46] = bf0[46] + bf0[41];
    bf1[47] = bf0[47] + bf0[40];
    bf1[48] = bf0[48] + bf0[55];
    bf1[49] = bf0[49] + bf0[54];
    bf1[50] = bf0[50] + bf0[53];
    bf1[51] = bf0[51] + bf0[52];
    bf1[52] = -bf0[52] + bf0[51];
    bf1[53] = -bf0[53] + bf0[50];
    bf1[54] = -bf0[54] + bf0[49];
    bf1[55] = -bf0[55] + bf0[48];
    bf1[56] = -bf0[56] + bf0[63];
    bf1[57] = -bf0[57] + bf0[62];
    bf1[58] = -bf0[58] + bf0[61];
    bf1[59] = -bf0[59] + bf0[60];
    bf1[60] = bf0[60] + bf0[59];
    bf1[61] = bf0[61] + bf0[58];
    bf1[62] = bf0[62] + bf0[57];
    bf1[63] = bf0[63] + bf0[56];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1]  = half_btf(-cospi[32], bf0[1], cospi[32], bf0[0], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[48], bf0[3], -cospi[16], bf0[2], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[5]  = -bf0[5] + bf0[4];
    bf1[6]  = -bf0[6] + bf0[7];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[19];
    bf1[17] = bf0[17] + bf0[18];
    bf1[18] = -bf0[18] + bf0[17];
    bf1[19] = -bf0[19] + bf0[16];
    bf1[20] = -bf0[20] + bf0[23];
    bf1[21] = -bf0[21] + bf0[22];
    bf1[22] = bf0[22] + bf0[21];
    bf1[23] = bf0[23] + bf0[20];
    bf1[24] = bf0[24] + bf0[27];
    bf1[25] = bf0[25] + bf0[26];
    bf1[26] = -bf0[26] + bf0[25];
    bf1[27] = -bf0[27] + bf0[24];
    bf1[28] = -bf0[28] + bf0[31];
    bf1[29] = -bf0[29] + bf0[30];
    bf1[30] = bf0[30] + bf0[29];
    bf1[31] = bf0[31] + bf0[28];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = half_btf(-cospi[8], bf0[34], cospi[56], bf0[61], cos_bit);
    bf1[35] = half_btf(-cospi[8], bf0[35], cospi[56], bf0[60], cos_bit);
    bf1[36] = half_btf(-cospi[56], bf0[36], -cospi[8], bf0[59], cos_bit);
    bf1[37] = half_btf(-cospi[56], bf0[37], -cospi[8], bf0[58], cos_bit);
    bf1[38] = bf0[38];
    bf1[39] = bf0[39];
    bf1[40] = bf0[40];
    bf1[41] = bf0[41];
    bf1[42] = half_btf(-cospi[40], bf0[42], cospi[24], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[40], bf0[43], cospi[24], bf0[52], cos_bit);
    bf1[44] = half_btf(-cospi[24], bf0[44], -cospi[40], bf0[51], cos_bit);
    bf1[45] = half_btf(-cospi[24], bf0[45], -cospi[40], bf0[50], cos_bit);
    bf1[46] = bf0[46];
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = bf0[49];
    bf1[50] = half_btf(cospi[24], bf0[50], -cospi[40], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[24], bf0[51], -cospi[40], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[40], bf0[52], cospi[24], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[40], bf0[53], cospi[24], bf0[42], cos_bit);
    bf1[54] = bf0[54];
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = half_btf(cospi[56], bf0[58], -cospi[8], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[56], bf0[59], -cospi[8], bf0[36], cos_bit);
    bf1[60] = half_btf(cospi[8], bf0[60], cospi[56], bf0[35], cos_bit);
    bf1[61] = half_btf(cospi[8], bf0[61], cospi[56], bf0[34], cos_bit);
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 7
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[5]  = half_btf(cospi[24], bf0[5], cospi[40], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[7]  = half_btf(cospi[56], bf0[7], -cospi[8], bf0[4], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[9]  = -bf0[9] + bf0[8];
    bf1[10] = -bf0[10] + bf0[11];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[13] = -bf0[13] + bf0[12];
    bf1[14] = -bf0[14] + bf0[15];
    bf1[15] = bf0[15] + bf0[14];
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(cospi[24], bf0[25], -cospi[40], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[21], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(cospi[56], bf0[29], -cospi[8], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[8], bf0[30], cospi[56], bf0[17], cos_bit);
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[35];
    bf1[33] = bf0[33] + bf0[34];
    bf1[34] = -bf0[34] + bf0[33];
    bf1[35] = -bf0[35] + bf0[32];
    bf1[36] = -bf0[36] + bf0[39];
    bf1[37] = -bf0[37] + bf0[38];
    bf1[38] = bf0[38] + bf0[37];
    bf1[39] = bf0[39] + bf0[36];
    bf1[40] = bf0[40] + bf0[43];
    bf1[41] = bf0[41] + bf0[42];
    bf1[42] = -bf0[42] + bf0[41];
    bf1[43] = -bf0[43] + bf0[40];
    bf1[44] = -bf0[44] + bf0[47];
    bf1[45] = -bf0[45] + bf0[46];
    bf1[46] = bf0[46] + bf0[45];
    bf1[47] = bf0[47] + bf0[44];
    bf1[48] = bf0[48] + bf0[51];
    bf1[49] = bf0[49] + bf0[50];
    bf1[50] = -bf0[50] + bf0[49];
    bf1[51] = -bf0[51] + bf0[48];
    bf1[52] = -bf0[52] + bf0[55];
    bf1[53] = -bf0[53] + bf0[54];
    bf1[54] = bf0[54] + bf0[53];
    bf1[55] = bf0[55] + bf0[52];
    bf1[56] = bf0[56] + bf0[59];
    bf1[57] = bf0[57] + bf0[58];
    bf1[58] = -bf0[58] + bf0[57];
    bf1[59] = -bf0[59] + bf0[56];
    bf1[60] = -bf0[60] + bf0[63];
    bf1[61] = -bf0[61] + bf0[62];
    bf1[62] = bf0[62] + bf0[61];
    bf1[63] = bf0[63] + bf0[60];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[9]  = half_btf(cospi[28], bf0[9], cospi[36], bf0[14], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], cospi[20], bf0[13], cos_bit);
    bf1[11] = half_btf(cospi[12], bf0[11], cospi[52], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[44], bf0[13], -cospi[20], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[28], bf0[14], -cospi[36], bf0[9], cos_bit);
    bf1[15] = half_btf(cospi[60], bf0[15], -cospi[4], bf0[8], cos_bit);
    bf1[16] = bf0[16] + bf0[17];
    bf1[17] = -bf0[17] + bf0[16];
    bf1[18] = -bf0[18] + bf0[19];
    bf1[19] = bf0[19] + bf0[18];
    bf1[20] = bf0[20] + bf0[21];
    bf1[21] = -bf0[21] + bf0[20];
    bf1[22] = -bf0[22] + bf0[23];
    bf1[23] = bf0[23] + bf0[22];
    bf1[24] = bf0[24] + bf0[25];
    bf1[25] = -bf0[25] + bf0[24];
    bf1[26] = -bf0[26] + bf0[27];
    bf1[27] = bf0[27] + bf0[26];
    bf1[28] = bf0[28] + bf0[29];
    bf1[29] = -bf0[29] + bf0[28];
    bf1[30] = -bf0[30] + bf0[31];
    bf1[31] = bf0[31] + bf0[30];
    bf1[32] = bf0[32];
    bf1[33] = half_btf(-cospi[4], bf0[33], cospi[60], bf0[62], cos_bit);
    bf1[34] = half_btf(-cospi[60], bf0[34], -cospi[4], bf0[61], cos_bit);
    bf1[35] = bf0[35];
    bf1[36] = bf0[36];
    bf1[37] = half_btf(-cospi[36], bf0[37], cospi[28], bf0[58], cos_bit);
    bf1[38] = half_btf(-cospi[28], bf0[38], -cospi[36], bf0[57], cos_bit);
    bf1[39] = bf0[39];
    bf1[40] = bf0[40];
    bf1[41] = half_btf(-cospi[20], bf0[41], cospi[44], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[44], bf0[42], -cospi[20], bf0[53], cos_bit);
    bf1[43] = bf0[43];
    bf1[44] = bf0[44];
    bf1[45] = half_btf(-cospi[52], bf0[45], cospi[12], bf0[50], cos_bit);
    bf1[46] = half_btf(-cospi[12], bf0[46], -cospi[52], bf0[49], cos_bit);
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = half_btf(cospi[12], bf0[49], -cospi[52], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[52], bf0[50], cospi[12], bf0[45], cos_bit);
    bf1[51] = bf0[51];
    bf1[52] = bf0[52];
    bf1[53] = half_btf(cospi[44], bf0[53], -cospi[20], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[20], bf0[54], cospi[44], bf0[41], cos_bit);
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = half_btf(cospi[28], bf0[57], -cospi[36], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[36], bf0[58], cospi[28], bf0[37], cos_bit);
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = half_btf(cospi[60], bf0[61], -cospi[4], bf0[34], cos_bit);
    bf1[62] = half_btf(cospi[4], bf0[62], cospi[60], bf0[33], cos_bit);
    bf1[63] = bf0[63];

    // stage 9
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = half_btf(cospi[62], bf0[16], cospi[2], bf0[31], cos_bit);
    bf1[17] = half_btf(cospi[30], bf0[17], cospi[34], bf0[30], cos_bit);
    bf1[18] = half_btf(cospi[46], bf0[18], cospi[18], bf0[29], cos_bit);
    bf1[19] = half_btf(cospi[14], bf0[19], cospi[50], bf0[28], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], cospi[10], bf0[27], cos_bit);
    bf1[21] = half_btf(cospi[22], bf0[21], cospi[42], bf0[26], cos_bit);
    bf1[22] = half_btf(cospi[38], bf0[22], cospi[26], bf0[25], cos_bit);
    bf1[23] = half_btf(cospi[6], bf0[23], cospi[58], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[6], bf0[24], -cospi[58], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[38], bf0[25], -cospi[26], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[22], bf0[26], -cospi[42], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[54], bf0[27], -cospi[10], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[14], bf0[28], -cospi[50], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[46], bf0[29], -cospi[18], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[30], bf0[30], -cospi[34], bf0[17], cos_bit);
    bf1[31] = half_btf(cospi[62], bf0[31], -cospi[2], bf0[16], cos_bit);
    bf1[32] = bf0[32] + bf0[33];
    bf1[33] = -bf0[33] + bf0[32];
    bf1[34] = -bf0[34] + bf0[35];
    bf1[35] = bf0[35] + bf0[34];
    bf1[36] = bf0[36] + bf0[37];
    bf1[37] = -bf0[37] + bf0[36];
    bf1[38] = -bf0[38] + bf0[39];
    bf1[39] = bf0[39] + bf0[38];
    bf1[40] = bf0[40] + bf0[41];
    bf1[41] = -bf0[41] + bf0[40];
    bf1[42] = -bf0[42] + bf0[43];
    bf1[43] = bf0[43] + bf0[42];
    bf1[44] = bf0[44] + bf0[45];
    bf1[45] = -bf0[45] + bf0[44];
    bf1[46] = -bf0[46] + bf0[47];
    bf1[47] = bf0[47] + bf0[46];
    bf1[48] = bf0[48] + bf0[49];
    bf1[49] = -bf0[49] + bf0[48];
    bf1[50] = -bf0[50] + bf0[51];
    bf1[51] = bf0[51] + bf0[50];
    bf1[52] = bf0[52] + bf0[53];
    bf1[53] = -bf0[53] + bf0[52];
    bf1[54] = -bf0[54] + bf0[55];
    bf1[55] = bf0[55] + bf0[54];
    bf1[56] = bf0[56] + bf0[57];
    bf1[57] = -bf0[57] + bf0[56];
    bf1[58] = -bf0[58] + bf0[59];
    bf1[59] = bf0[59] + bf0[58];
    bf1[60] = bf0[60] + bf0[61];
    bf1[61] = -bf0[61] + bf0[60];
    bf1[62] = -bf0[62] + bf0[63];
    bf1[63] = bf0[63] + bf0[62];

    // stage 10
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = bf0[21];
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = bf0[26];
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = half_btf(cospi[63], bf0[32], cospi[1], bf0[63], cos_bit);
    bf1[33] = half_btf(cospi[31], bf0[33], cospi[33], bf0[62], cos_bit);
    bf1[34] = half_btf(cospi[47], bf0[34], cospi[17], bf0[61], cos_bit);
    bf1[35] = half_btf(cospi[15], bf0[35], cospi[49], bf0[60], cos_bit);
    bf1[36] = half_btf(cospi[55], bf0[36], cospi[9], bf0[59], cos_bit);
    bf1[37] = half_btf(cospi[23], bf0[37], cospi[41], bf0[58], cos_bit);
    bf1[38] = half_btf(cospi[39], bf0[38], cospi[25], bf0[57], cos_bit);
    bf1[39] = half_btf(cospi[7], bf0[39], cospi[57], bf0[56], cos_bit);
    bf1[40] = half_btf(cospi[59], bf0[40], cospi[5], bf0[55], cos_bit);
    bf1[41] = half_btf(cospi[27], bf0[41], cospi[37], bf0[54], cos_bit);
    bf1[42] = half_btf(cospi[43], bf0[42], cospi[21], bf0[53], cos_bit);
    bf1[43] = half_btf(cospi[11], bf0[43], cospi[53], bf0[52], cos_bit);
    bf1[44] = half_btf(cospi[51], bf0[44], cospi[13], bf0[51], cos_bit);
    bf1[45] = half_btf(cospi[19], bf0[45], cospi[45], bf0[50], cos_bit);
    bf1[46] = half_btf(cospi[35], bf0[46], cospi[29], bf0[49], cos_bit);
    bf1[47] = half_btf(cospi[3], bf0[47], cospi[61], bf0[48], cos_bit);
    bf1[48] = half_btf(cospi[3], bf0[48], -cospi[61], bf0[47], cos_bit);
    bf1[49] = half_btf(cospi[35], bf0[49], -cospi[29], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[19], bf0[50], -cospi[45], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[51], bf0[51], -cospi[13], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[11], bf0[52], -cospi[53], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[43], bf0[53], -cospi[21], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[27], bf0[54], -cospi[37], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[59], bf0[55], -cospi[5], bf0[40], cos_bit);
    bf1[56] = half_btf(cospi[7], bf0[56], -cospi[57], bf0[39], cos_bit);
    bf1[57] = half_btf(cospi[39], bf0[57], -cospi[25], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[23], bf0[58], -cospi[41], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[55], bf0[59], -cospi[9], bf0[36], cos_bit);
    bf1[60] = half_btf(cospi[15], bf0[60], -cospi[49], bf0[35], cos_bit);
    bf1[61] = half_btf(cospi[47], bf0[61], -cospi[17], bf0[34], cos_bit);
    bf1[62] = half_btf(cospi[31], bf0[62], -cospi[33], bf0[33], cos_bit);
    bf1[63] = half_btf(cospi[63], bf0[63], -cospi[1], bf0[32], cos_bit);

    // stage 11
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[32];
    bf1[2]  = bf0[16];
    bf1[3]  = bf0[48];
    bf1[4]  = bf0[8];
    bf1[5]  = bf0[40];
    bf1[6]  = bf0[24];
    bf1[7]  = bf0[56];
    bf1[8]  = bf0[4];
    bf1[9]  = bf0[36];
    bf1[10] = bf0[20];
    bf1[11] = bf0[52];
    bf1[12] = bf0[12];
    bf1[13] = bf0[44];
    bf1[14] = bf0[28];
    bf1[15] = bf0[60];
    bf1[16] = bf0[2];
    bf1[17] = bf0[34];
    bf1[18] = bf0[18];
    bf1[19] = bf0[50];
    bf1[20] = bf0[10];
    bf1[21] = bf0[42];
    bf1[22] = bf0[26];
    bf1[23] = bf0[58];
    bf1[24] = bf0[6];
    bf1[25] = bf0[38];
    bf1[26] = bf0[22];
    bf1[27] = bf0[54];
    bf1[28] = bf0[14];
    bf1[29] = bf0[46];
    bf1[30] = bf0[30];
    bf1[31] = bf0[62];
    bf1[32] = bf0[1];
    bf1[33] = bf0[33];
    bf1[34] = bf0[17];
    bf1[35] = bf0[49];
    bf1[36] = bf0[9];
    bf1[37] = bf0[41];
    bf1[38] = bf0[25];
    bf1[39] = bf0[57];
    bf1[40] = bf0[5];
    bf1[41] = bf0[37];
    bf1[42] = bf0[21];
    bf1[43] = bf0[53];
    bf1[44] = bf0[13];
    bf1[45] = bf0[45];
    bf1[46] = bf0[29];
    bf1[47] = bf0[61];
    bf1[48] = bf0[3];
    bf1[49] = bf0[35];
    bf1[50] = bf0[19];
    bf1[51] = bf0[51];
    bf1[52] = bf0[11];
    bf1[53] = bf0[43];
    bf1[54] = bf0[27];
    bf1[55] = bf0[59];
    bf1[56] = bf0[7];
    bf1[57] = bf0[39];
    bf1[58] = bf0[23];
    bf1[59] = bf0[55];
    bf1[60] = bf0[15];
    bf1[61] = bf0[47];
    bf1[62] = bf0[31];
    bf1[63] = bf0[63];
}

void svt_av1_fadst4_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    int32_t        bit   = cos_bit;
    const int32_t* sinpi = sinpi_arr(bit);
    int32_t        x0, x1, x2, x3;
    int32_t        s0, s1, s2, s3, s4, s5, s6, s7;

    // stage 0
    x0 = input[0];
    x1 = input[1];
    x2 = input[2];
    x3 = input[3];

    if (!(x0 | x1 | x2 | x3)) {
        output[0] = output[1] = output[2] = output[3] = 0;
        return;
    }

    //// stage 1
    //s0 = range_check_value(sinpi[1] * x0, bit + stage_range[1]);
    //s1 = range_check_value(sinpi[4] * x0, bit + stage_range[1]);
    //s2 = range_check_value(sinpi[2] * x1, bit + stage_range[1]);
    //s3 = range_check_value(sinpi[1] * x1, bit + stage_range[1]);
    //s4 = range_check_value(sinpi[3] * x2, bit + stage_range[1]);
    //s5 = range_check_value(sinpi[4] * x3, bit + stage_range[1]);
    //s6 = range_check_value(sinpi[2] * x3, bit + stage_range[1]);
    //s7 = range_check_value(x0 + x1, stage_range[1]);

    //// stage 2
    //s7 = range_check_value(s7 - x3, stage_range[2]);

    //// stage 3
    //x0 = range_check_value(s0 + s2, bit + stage_range[3]);
    //x1 = range_check_value(sinpi[3] * s7, bit + stage_range[3]);
    //x2 = range_check_value(s1 - s3, bit + stage_range[3]);
    //x3 = range_check_value(s4, bit + stage_range[3]);

    //// stage 4
    //x0 = range_check_value(x0 + s5, bit + stage_range[4]);
    //x2 = range_check_value(x2 + s6, bit + stage_range[4]);

    //// stage 5
    //s0 = range_check_value(x0 + x3, bit + stage_range[5]);
    //s1 = range_check_value(x1, bit + stage_range[5]);
    //s2 = range_check_value(x2 - x3, bit + stage_range[5]);
    //s3 = range_check_value(x2 - x0, bit + stage_range[5]);

    //// stage 6
    //s3 = range_check_value(s3 + x3, bit + stage_range[6]);

    // stage 1
    s0 = sinpi[1] * x0;
    s1 = sinpi[4] * x0;
    s2 = sinpi[2] * x1;
    s3 = sinpi[1] * x1;
    s4 = sinpi[3] * x2;
    s5 = sinpi[4] * x3;
    s6 = sinpi[2] * x3;
    s7 = x0 + x1;

    // stage 2
    s7 = s7 - x3;

    // stage 3
    x0 = s0 + s2;
    x1 = sinpi[3] * s7;
    x2 = s1 - s3;
    x3 = s4;

    // stage 4
    x0 = x0 + s5;
    x2 = x2 + s6;

    // stage 5
    s0 = x0 + x3;
    s1 = x1;
    s2 = x2 - x3;
    s3 = x2 - x0;

    // stage 6
    s3 = s3 + x3;

    // 1-D transform scaling factor is sqrt(2).
    output[0] = round_shift(s0, bit);
    output[1] = round_shift(s1, bit);
    output[2] = round_shift(s2, bit);
    output[3] = round_shift(s3, bit);
}

void svt_av1_fadst8_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    assert(output != input);
    bf1    = output;
    bf1[0] = input[0];
    bf1[1] = -input[7];
    bf1[2] = -input[3];
    bf1[3] = input[4];
    bf1[4] = -input[1];
    bf1[5] = input[6];
    bf1[6] = input[2];
    bf1[7] = -input[5];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[32], bf0[2], -cospi[32], bf0[3], cos_bit);
    bf1[4] = bf0[4];
    bf1[5] = bf0[5];
    bf1[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[32], bf0[6], -cospi[32], bf0[7], cos_bit);

    // stage 3
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0] + bf0[2];
    bf1[1] = bf0[1] + bf0[3];
    bf1[2] = bf0[0] - bf0[2];
    bf1[3] = bf0[1] - bf0[3];
    bf1[4] = bf0[4] + bf0[6];
    bf1[5] = bf0[5] + bf0[7];
    bf1[6] = bf0[4] - bf0[6];
    bf1[7] = bf0[5] - bf0[7];

    // stage 4
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = bf0[2];
    bf1[3] = bf0[3];
    bf1[4] = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5] = half_btf(cospi[48], bf0[4], -cospi[16], bf0[5], cos_bit);
    bf1[6] = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[16], bf0[6], cospi[48], bf0[7], cos_bit);

    // stage 5
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0] + bf0[4];
    bf1[1] = bf0[1] + bf0[5];
    bf1[2] = bf0[2] + bf0[6];
    bf1[3] = bf0[3] + bf0[7];
    bf1[4] = bf0[0] - bf0[4];
    bf1[5] = bf0[1] - bf0[5];
    bf1[6] = bf0[2] - bf0[6];
    bf1[7] = bf0[3] - bf0[7];

    // stage 6
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = half_btf(cospi[4], bf0[0], cospi[60], bf0[1], cos_bit);
    bf1[1] = half_btf(cospi[60], bf0[0], -cospi[4], bf0[1], cos_bit);
    bf1[2] = half_btf(cospi[20], bf0[2], cospi[44], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[44], bf0[2], -cospi[20], bf0[3], cos_bit);
    bf1[4] = half_btf(cospi[36], bf0[4], cospi[28], bf0[5], cos_bit);
    bf1[5] = half_btf(cospi[28], bf0[4], -cospi[36], bf0[5], cos_bit);
    bf1[6] = half_btf(cospi[52], bf0[6], cospi[12], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[12], bf0[6], -cospi[52], bf0[7], cos_bit);

    // stage 7
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[1];
    bf1[1] = bf0[6];
    bf1[2] = bf0[3];
    bf1[3] = bf0[4];
    bf1[4] = bf0[5];
    bf1[5] = bf0[2];
    bf1[6] = bf0[7];
    bf1[7] = bf0[0];
}

void svt_av1_fadst16_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    assert(output != input);
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = -input[15];
    bf1[2]  = -input[7];
    bf1[3]  = input[8];
    bf1[4]  = -input[3];
    bf1[5]  = input[12];
    bf1[6]  = input[4];
    bf1[7]  = -input[11];
    bf1[8]  = -input[1];
    bf1[9]  = input[14];
    bf1[10] = input[6];
    bf1[11] = -input[9];
    bf1[12] = input[2];
    bf1[13] = -input[13];
    bf1[14] = -input[5];
    bf1[15] = input[10];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[32], bf0[2], -cospi[32], bf0[3], cos_bit);
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[32], bf0[6], -cospi[32], bf0[7], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(cospi[32], bf0[10], cospi[32], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[32], bf0[10], -cospi[32], bf0[11], cos_bit);
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = half_btf(cospi[32], bf0[14], cospi[32], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[32], bf0[14], -cospi[32], bf0[15], cos_bit);

    // stage 3
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[2];
    bf1[1]  = bf0[1] + bf0[3];
    bf1[2]  = bf0[0] - bf0[2];
    bf1[3]  = bf0[1] - bf0[3];
    bf1[4]  = bf0[4] + bf0[6];
    bf1[5]  = bf0[5] + bf0[7];
    bf1[6]  = bf0[4] - bf0[6];
    bf1[7]  = bf0[5] - bf0[7];
    bf1[8]  = bf0[8] + bf0[10];
    bf1[9]  = bf0[9] + bf0[11];
    bf1[10] = bf0[8] - bf0[10];
    bf1[11] = bf0[9] - bf0[11];
    bf1[12] = bf0[12] + bf0[14];
    bf1[13] = bf0[13] + bf0[15];
    bf1[14] = bf0[12] - bf0[14];
    bf1[15] = bf0[13] - bf0[15];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5]  = half_btf(cospi[48], bf0[4], -cospi[16], bf0[5], cos_bit);
    bf1[6]  = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[16], bf0[6], cospi[48], bf0[7], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = half_btf(cospi[16], bf0[12], cospi[48], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[48], bf0[12], -cospi[16], bf0[13], cos_bit);
    bf1[14] = half_btf(-cospi[48], bf0[14], cospi[16], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[16], bf0[14], cospi[48], bf0[15], cos_bit);

    // stage 5
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[4];
    bf1[1]  = bf0[1] + bf0[5];
    bf1[2]  = bf0[2] + bf0[6];
    bf1[3]  = bf0[3] + bf0[7];
    bf1[4]  = bf0[0] - bf0[4];
    bf1[5]  = bf0[1] - bf0[5];
    bf1[6]  = bf0[2] - bf0[6];
    bf1[7]  = bf0[3] - bf0[7];
    bf1[8]  = bf0[8] + bf0[12];
    bf1[9]  = bf0[9] + bf0[13];
    bf1[10] = bf0[10] + bf0[14];
    bf1[11] = bf0[11] + bf0[15];
    bf1[12] = bf0[8] - bf0[12];
    bf1[13] = bf0[9] - bf0[13];
    bf1[14] = bf0[10] - bf0[14];
    bf1[15] = bf0[11] - bf0[15];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[8], bf0[8], cospi[56], bf0[9], cos_bit);
    bf1[9]  = half_btf(cospi[56], bf0[8], -cospi[8], bf0[9], cos_bit);
    bf1[10] = half_btf(cospi[40], bf0[10], cospi[24], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[24], bf0[10], -cospi[40], bf0[11], cos_bit);
    bf1[12] = half_btf(-cospi[56], bf0[12], cospi[8], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[8], bf0[12], cospi[56], bf0[13], cos_bit);
    bf1[14] = half_btf(-cospi[24], bf0[14], cospi[40], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[40], bf0[14], cospi[24], bf0[15], cos_bit);

    // stage 7
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[8];
    bf1[1]  = bf0[1] + bf0[9];
    bf1[2]  = bf0[2] + bf0[10];
    bf1[3]  = bf0[3] + bf0[11];
    bf1[4]  = bf0[4] + bf0[12];
    bf1[5]  = bf0[5] + bf0[13];
    bf1[6]  = bf0[6] + bf0[14];
    bf1[7]  = bf0[7] + bf0[15];
    bf1[8]  = bf0[0] - bf0[8];
    bf1[9]  = bf0[1] - bf0[9];
    bf1[10] = bf0[2] - bf0[10];
    bf1[11] = bf0[3] - bf0[11];
    bf1[12] = bf0[4] - bf0[12];
    bf1[13] = bf0[5] - bf0[13];
    bf1[14] = bf0[6] - bf0[14];
    bf1[15] = bf0[7] - bf0[15];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[2], bf0[0], cospi[62], bf0[1], cos_bit);
    bf1[1]  = half_btf(cospi[62], bf0[0], -cospi[2], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[10], bf0[2], cospi[54], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[54], bf0[2], -cospi[10], bf0[3], cos_bit);
    bf1[4]  = half_btf(cospi[18], bf0[4], cospi[46], bf0[5], cos_bit);
    bf1[5]  = half_btf(cospi[46], bf0[4], -cospi[18], bf0[5], cos_bit);
    bf1[6]  = half_btf(cospi[26], bf0[6], cospi[38], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[38], bf0[6], -cospi[26], bf0[7], cos_bit);
    bf1[8]  = half_btf(cospi[34], bf0[8], cospi[30], bf0[9], cos_bit);
    bf1[9]  = half_btf(cospi[30], bf0[8], -cospi[34], bf0[9], cos_bit);
    bf1[10] = half_btf(cospi[42], bf0[10], cospi[22], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[22], bf0[10], -cospi[42], bf0[11], cos_bit);
    bf1[12] = half_btf(cospi[50], bf0[12], cospi[14], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[14], bf0[12], -cospi[50], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[58], bf0[14], cospi[6], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[6], bf0[14], -cospi[58], bf0[15], cos_bit);

    // stage 9
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[1];
    bf1[1]  = bf0[14];
    bf1[2]  = bf0[3];
    bf1[3]  = bf0[12];
    bf1[4]  = bf0[5];
    bf1[5]  = bf0[10];
    bf1[6]  = bf0[7];
    bf1[7]  = bf0[8];
    bf1[8]  = bf0[9];
    bf1[9]  = bf0[6];
    bf1[10] = bf0[11];
    bf1[11] = bf0[4];
    bf1[12] = bf0[13];
    bf1[13] = bf0[2];
    bf1[14] = bf0[15];
    bf1[15] = bf0[0];
}

static void av1_fadst32_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[32];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[31];
    bf1[1]  = input[0];
    bf1[2]  = input[29];
    bf1[3]  = input[2];
    bf1[4]  = input[27];
    bf1[5]  = input[4];
    bf1[6]  = input[25];
    bf1[7]  = input[6];
    bf1[8]  = input[23];
    bf1[9]  = input[8];
    bf1[10] = input[21];
    bf1[11] = input[10];
    bf1[12] = input[19];
    bf1[13] = input[12];
    bf1[14] = input[17];
    bf1[15] = input[14];
    bf1[16] = input[15];
    bf1[17] = input[16];
    bf1[18] = input[13];
    bf1[19] = input[18];
    bf1[20] = input[11];
    bf1[21] = input[20];
    bf1[22] = input[9];
    bf1[23] = input[22];
    bf1[24] = input[7];
    bf1[25] = input[24];
    bf1[26] = input[5];
    bf1[27] = input[26];
    bf1[28] = input[3];
    bf1[29] = input[28];
    bf1[30] = input[1];
    bf1[31] = input[30];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[1], bf0[0], cospi[63], bf0[1], cos_bit);
    bf1[1]  = half_btf(-cospi[1], bf0[1], cospi[63], bf0[0], cos_bit);
    bf1[2]  = half_btf(cospi[5], bf0[2], cospi[59], bf0[3], cos_bit);
    bf1[3]  = half_btf(-cospi[5], bf0[3], cospi[59], bf0[2], cos_bit);
    bf1[4]  = half_btf(cospi[9], bf0[4], cospi[55], bf0[5], cos_bit);
    bf1[5]  = half_btf(-cospi[9], bf0[5], cospi[55], bf0[4], cos_bit);
    bf1[6]  = half_btf(cospi[13], bf0[6], cospi[51], bf0[7], cos_bit);
    bf1[7]  = half_btf(-cospi[13], bf0[7], cospi[51], bf0[6], cos_bit);
    bf1[8]  = half_btf(cospi[17], bf0[8], cospi[47], bf0[9], cos_bit);
    bf1[9]  = half_btf(-cospi[17], bf0[9], cospi[47], bf0[8], cos_bit);
    bf1[10] = half_btf(cospi[21], bf0[10], cospi[43], bf0[11], cos_bit);
    bf1[11] = half_btf(-cospi[21], bf0[11], cospi[43], bf0[10], cos_bit);
    bf1[12] = half_btf(cospi[25], bf0[12], cospi[39], bf0[13], cos_bit);
    bf1[13] = half_btf(-cospi[25], bf0[13], cospi[39], bf0[12], cos_bit);
    bf1[14] = half_btf(cospi[29], bf0[14], cospi[35], bf0[15], cos_bit);
    bf1[15] = half_btf(-cospi[29], bf0[15], cospi[35], bf0[14], cos_bit);
    bf1[16] = half_btf(cospi[33], bf0[16], cospi[31], bf0[17], cos_bit);
    bf1[17] = half_btf(-cospi[33], bf0[17], cospi[31], bf0[16], cos_bit);
    bf1[18] = half_btf(cospi[37], bf0[18], cospi[27], bf0[19], cos_bit);
    bf1[19] = half_btf(-cospi[37], bf0[19], cospi[27], bf0[18], cos_bit);
    bf1[20] = half_btf(cospi[41], bf0[20], cospi[23], bf0[21], cos_bit);
    bf1[21] = half_btf(-cospi[41], bf0[21], cospi[23], bf0[20], cos_bit);
    bf1[22] = half_btf(cospi[45], bf0[22], cospi[19], bf0[23], cos_bit);
    bf1[23] = half_btf(-cospi[45], bf0[23], cospi[19], bf0[22], cos_bit);
    bf1[24] = half_btf(cospi[49], bf0[24], cospi[15], bf0[25], cos_bit);
    bf1[25] = half_btf(-cospi[49], bf0[25], cospi[15], bf0[24], cos_bit);
    bf1[26] = half_btf(cospi[53], bf0[26], cospi[11], bf0[27], cos_bit);
    bf1[27] = half_btf(-cospi[53], bf0[27], cospi[11], bf0[26], cos_bit);
    bf1[28] = half_btf(cospi[57], bf0[28], cospi[7], bf0[29], cos_bit);
    bf1[29] = half_btf(-cospi[57], bf0[29], cospi[7], bf0[28], cos_bit);
    bf1[30] = half_btf(cospi[61], bf0[30], cospi[3], bf0[31], cos_bit);
    bf1[31] = half_btf(-cospi[61], bf0[31], cospi[3], bf0[30], cos_bit);

    // stage 3
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[16];
    bf1[1]  = bf0[1] + bf0[17];
    bf1[2]  = bf0[2] + bf0[18];
    bf1[3]  = bf0[3] + bf0[19];
    bf1[4]  = bf0[4] + bf0[20];
    bf1[5]  = bf0[5] + bf0[21];
    bf1[6]  = bf0[6] + bf0[22];
    bf1[7]  = bf0[7] + bf0[23];
    bf1[8]  = bf0[8] + bf0[24];
    bf1[9]  = bf0[9] + bf0[25];
    bf1[10] = bf0[10] + bf0[26];
    bf1[11] = bf0[11] + bf0[27];
    bf1[12] = bf0[12] + bf0[28];
    bf1[13] = bf0[13] + bf0[29];
    bf1[14] = bf0[14] + bf0[30];
    bf1[15] = bf0[15] + bf0[31];
    bf1[16] = -bf0[16] + bf0[0];
    bf1[17] = -bf0[17] + bf0[1];
    bf1[18] = -bf0[18] + bf0[2];
    bf1[19] = -bf0[19] + bf0[3];
    bf1[20] = -bf0[20] + bf0[4];
    bf1[21] = -bf0[21] + bf0[5];
    bf1[22] = -bf0[22] + bf0[6];
    bf1[23] = -bf0[23] + bf0[7];
    bf1[24] = -bf0[24] + bf0[8];
    bf1[25] = -bf0[25] + bf0[9];
    bf1[26] = -bf0[26] + bf0[10];
    bf1[27] = -bf0[27] + bf0[11];
    bf1[28] = -bf0[28] + bf0[12];
    bf1[29] = -bf0[29] + bf0[13];
    bf1[30] = -bf0[30] + bf0[14];
    bf1[31] = -bf0[31] + bf0[15];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = half_btf(cospi[4], bf0[16], cospi[60], bf0[17], cos_bit);
    bf1[17] = half_btf(-cospi[4], bf0[17], cospi[60], bf0[16], cos_bit);
    bf1[18] = half_btf(cospi[20], bf0[18], cospi[44], bf0[19], cos_bit);
    bf1[19] = half_btf(-cospi[20], bf0[19], cospi[44], bf0[18], cos_bit);
    bf1[20] = half_btf(cospi[36], bf0[20], cospi[28], bf0[21], cos_bit);
    bf1[21] = half_btf(-cospi[36], bf0[21], cospi[28], bf0[20], cos_bit);
    bf1[22] = half_btf(cospi[52], bf0[22], cospi[12], bf0[23], cos_bit);
    bf1[23] = half_btf(-cospi[52], bf0[23], cospi[12], bf0[22], cos_bit);
    bf1[24] = half_btf(-cospi[60], bf0[24], cospi[4], bf0[25], cos_bit);
    bf1[25] = half_btf(cospi[60], bf0[25], cospi[4], bf0[24], cos_bit);
    bf1[26] = half_btf(-cospi[44], bf0[26], cospi[20], bf0[27], cos_bit);
    bf1[27] = half_btf(cospi[44], bf0[27], cospi[20], bf0[26], cos_bit);
    bf1[28] = half_btf(-cospi[28], bf0[28], cospi[36], bf0[29], cos_bit);
    bf1[29] = half_btf(cospi[28], bf0[29], cospi[36], bf0[28], cos_bit);
    bf1[30] = half_btf(-cospi[12], bf0[30], cospi[52], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[12], bf0[31], cospi[52], bf0[30], cos_bit);

    // stage 5
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[8];
    bf1[1]  = bf0[1] + bf0[9];
    bf1[2]  = bf0[2] + bf0[10];
    bf1[3]  = bf0[3] + bf0[11];
    bf1[4]  = bf0[4] + bf0[12];
    bf1[5]  = bf0[5] + bf0[13];
    bf1[6]  = bf0[6] + bf0[14];
    bf1[7]  = bf0[7] + bf0[15];
    bf1[8]  = -bf0[8] + bf0[0];
    bf1[9]  = -bf0[9] + bf0[1];
    bf1[10] = -bf0[10] + bf0[2];
    bf1[11] = -bf0[11] + bf0[3];
    bf1[12] = -bf0[12] + bf0[4];
    bf1[13] = -bf0[13] + bf0[5];
    bf1[14] = -bf0[14] + bf0[6];
    bf1[15] = -bf0[15] + bf0[7];
    bf1[16] = bf0[16] + bf0[24];
    bf1[17] = bf0[17] + bf0[25];
    bf1[18] = bf0[18] + bf0[26];
    bf1[19] = bf0[19] + bf0[27];
    bf1[20] = bf0[20] + bf0[28];
    bf1[21] = bf0[21] + bf0[29];
    bf1[22] = bf0[22] + bf0[30];
    bf1[23] = bf0[23] + bf0[31];
    bf1[24] = -bf0[24] + bf0[16];
    bf1[25] = -bf0[25] + bf0[17];
    bf1[26] = -bf0[26] + bf0[18];
    bf1[27] = -bf0[27] + bf0[19];
    bf1[28] = -bf0[28] + bf0[20];
    bf1[29] = -bf0[29] + bf0[21];
    bf1[30] = -bf0[30] + bf0[22];
    bf1[31] = -bf0[31] + bf0[23];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[8], bf0[8], cospi[56], bf0[9], cos_bit);
    bf1[9]  = half_btf(-cospi[8], bf0[9], cospi[56], bf0[8], cos_bit);
    bf1[10] = half_btf(cospi[40], bf0[10], cospi[24], bf0[11], cos_bit);
    bf1[11] = half_btf(-cospi[40], bf0[11], cospi[24], bf0[10], cos_bit);
    bf1[12] = half_btf(-cospi[56], bf0[12], cospi[8], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[56], bf0[13], cospi[8], bf0[12], cos_bit);
    bf1[14] = half_btf(-cospi[24], bf0[14], cospi[40], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[24], bf0[15], cospi[40], bf0[14], cos_bit);
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = bf0[21];
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = half_btf(cospi[8], bf0[24], cospi[56], bf0[25], cos_bit);
    bf1[25] = half_btf(-cospi[8], bf0[25], cospi[56], bf0[24], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[27], cos_bit);
    bf1[27] = half_btf(-cospi[40], bf0[27], cospi[24], bf0[26], cos_bit);
    bf1[28] = half_btf(-cospi[56], bf0[28], cospi[8], bf0[29], cos_bit);
    bf1[29] = half_btf(cospi[56], bf0[29], cospi[8], bf0[28], cos_bit);
    bf1[30] = half_btf(-cospi[24], bf0[30], cospi[40], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[24], bf0[31], cospi[40], bf0[30], cos_bit);

    // stage 7
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[4];
    bf1[1]  = bf0[1] + bf0[5];
    bf1[2]  = bf0[2] + bf0[6];
    bf1[3]  = bf0[3] + bf0[7];
    bf1[4]  = -bf0[4] + bf0[0];
    bf1[5]  = -bf0[5] + bf0[1];
    bf1[6]  = -bf0[6] + bf0[2];
    bf1[7]  = -bf0[7] + bf0[3];
    bf1[8]  = bf0[8] + bf0[12];
    bf1[9]  = bf0[9] + bf0[13];
    bf1[10] = bf0[10] + bf0[14];
    bf1[11] = bf0[11] + bf0[15];
    bf1[12] = -bf0[12] + bf0[8];
    bf1[13] = -bf0[13] + bf0[9];
    bf1[14] = -bf0[14] + bf0[10];
    bf1[15] = -bf0[15] + bf0[11];
    bf1[16] = bf0[16] + bf0[20];
    bf1[17] = bf0[17] + bf0[21];
    bf1[18] = bf0[18] + bf0[22];
    bf1[19] = bf0[19] + bf0[23];
    bf1[20] = -bf0[20] + bf0[16];
    bf1[21] = -bf0[21] + bf0[17];
    bf1[22] = -bf0[22] + bf0[18];
    bf1[23] = -bf0[23] + bf0[19];
    bf1[24] = bf0[24] + bf0[28];
    bf1[25] = bf0[25] + bf0[29];
    bf1[26] = bf0[26] + bf0[30];
    bf1[27] = bf0[27] + bf0[31];
    bf1[28] = -bf0[28] + bf0[24];
    bf1[29] = -bf0[29] + bf0[25];
    bf1[30] = -bf0[30] + bf0[26];
    bf1[31] = -bf0[31] + bf0[27];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5]  = half_btf(-cospi[16], bf0[5], cospi[48], bf0[4], cos_bit);
    bf1[6]  = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[48], bf0[7], cospi[16], bf0[6], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = half_btf(cospi[16], bf0[12], cospi[48], bf0[13], cos_bit);
    bf1[13] = half_btf(-cospi[16], bf0[13], cospi[48], bf0[12], cos_bit);
    bf1[14] = half_btf(-cospi[48], bf0[14], cospi[16], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[48], bf0[15], cospi[16], bf0[14], cos_bit);
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(cospi[16], bf0[20], cospi[48], bf0[21], cos_bit);
    bf1[21] = half_btf(-cospi[16], bf0[21], cospi[48], bf0[20], cos_bit);
    bf1[22] = half_btf(-cospi[48], bf0[22], cospi[16], bf0[23], cos_bit);
    bf1[23] = half_btf(cospi[48], bf0[23], cospi[16], bf0[22], cos_bit);
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = bf0[26];
    bf1[27] = bf0[27];
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[29], cos_bit);
    bf1[29] = half_btf(-cospi[16], bf0[29], cospi[48], bf0[28], cos_bit);
    bf1[30] = half_btf(-cospi[48], bf0[30], cospi[16], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[48], bf0[31], cospi[16], bf0[30], cos_bit);

    // stage 9
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[2];
    bf1[1]  = bf0[1] + bf0[3];
    bf1[2]  = -bf0[2] + bf0[0];
    bf1[3]  = -bf0[3] + bf0[1];
    bf1[4]  = bf0[4] + bf0[6];
    bf1[5]  = bf0[5] + bf0[7];
    bf1[6]  = -bf0[6] + bf0[4];
    bf1[7]  = -bf0[7] + bf0[5];
    bf1[8]  = bf0[8] + bf0[10];
    bf1[9]  = bf0[9] + bf0[11];
    bf1[10] = -bf0[10] + bf0[8];
    bf1[11] = -bf0[11] + bf0[9];
    bf1[12] = bf0[12] + bf0[14];
    bf1[13] = bf0[13] + bf0[15];
    bf1[14] = -bf0[14] + bf0[12];
    bf1[15] = -bf0[15] + bf0[13];
    bf1[16] = bf0[16] + bf0[18];
    bf1[17] = bf0[17] + bf0[19];
    bf1[18] = -bf0[18] + bf0[16];
    bf1[19] = -bf0[19] + bf0[17];
    bf1[20] = bf0[20] + bf0[22];
    bf1[21] = bf0[21] + bf0[23];
    bf1[22] = -bf0[22] + bf0[20];
    bf1[23] = -bf0[23] + bf0[21];
    bf1[24] = bf0[24] + bf0[26];
    bf1[25] = bf0[25] + bf0[27];
    bf1[26] = -bf0[26] + bf0[24];
    bf1[27] = -bf0[27] + bf0[25];
    bf1[28] = bf0[28] + bf0[30];
    bf1[29] = bf0[29] + bf0[31];
    bf1[30] = -bf0[30] + bf0[28];
    bf1[31] = -bf0[31] + bf0[29];

    // stage 10
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3]  = half_btf(-cospi[32], bf0[3], cospi[32], bf0[2], cos_bit);
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7]  = half_btf(-cospi[32], bf0[7], cospi[32], bf0[6], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(cospi[32], bf0[10], cospi[32], bf0[11], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[10], cos_bit);
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = half_btf(cospi[32], bf0[14], cospi[32], bf0[15], cos_bit);
    bf1[15] = half_btf(-cospi[32], bf0[15], cospi[32], bf0[14], cos_bit);
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(cospi[32], bf0[18], cospi[32], bf0[19], cos_bit);
    bf1[19] = half_btf(-cospi[32], bf0[19], cospi[32], bf0[18], cos_bit);
    bf1[20] = bf0[20];
    bf1[21] = bf0[21];
    bf1[22] = half_btf(cospi[32], bf0[22], cospi[32], bf0[23], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[22], cos_bit);
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[27], cos_bit);
    bf1[27] = half_btf(-cospi[32], bf0[27], cospi[32], bf0[26], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = half_btf(cospi[32], bf0[30], cospi[32], bf0[31], cos_bit);
    bf1[31] = half_btf(-cospi[32], bf0[31], cospi[32], bf0[30], cos_bit);

    // stage 11
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = -bf0[16];
    bf1[2]  = bf0[24];
    bf1[3]  = -bf0[8];
    bf1[4]  = bf0[12];
    bf1[5]  = -bf0[28];
    bf1[6]  = bf0[20];
    bf1[7]  = -bf0[4];
    bf1[8]  = bf0[6];
    bf1[9]  = -bf0[22];
    bf1[10] = bf0[30];
    bf1[11] = -bf0[14];
    bf1[12] = bf0[10];
    bf1[13] = -bf0[26];
    bf1[14] = bf0[18];
    bf1[15] = -bf0[2];
    bf1[16] = bf0[3];
    bf1[17] = -bf0[19];
    bf1[18] = bf0[27];
    bf1[19] = -bf0[11];
    bf1[20] = bf0[15];
    bf1[21] = -bf0[31];
    bf1[22] = bf0[23];
    bf1[23] = -bf0[7];
    bf1[24] = bf0[5];
    bf1[25] = -bf0[21];
    bf1[26] = bf0[29];
    bf1[27] = -bf0[13];
    bf1[28] = bf0[9];
    bf1[29] = -bf0[25];
    bf1[30] = bf0[17];
    bf1[31] = -bf0[1];
}

void svt_av1_fidentity4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 4; ++i) {
        output[i] = round_shift((int64_t)input[i] * new_sqrt2, new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_fidentity8_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 8; ++i) {
        output[i] = input[i] * 2;
    }
}

void svt_av1_fidentity16_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 16; ++i) {
        output[i] = round_shift((int64_t)input[i] * 2 * new_sqrt2, new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_fidentity32_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 32; ++i) {
        output[i] = input[i] * 4;
    }
}

static void av1_fidentity64_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 64; ++i) {
        output[i] = round_shift((int64_t)input[i] * 4 * new_sqrt2, new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

TxfmFunc svt_aom_fwd_txfm_type_to_func(TxfmType txfmtype) {
    switch (txfmtype) {
    case TXFM_TYPE_DCT4:
        return svt_av1_fdct4_new;
    case TXFM_TYPE_DCT8:
        return svt_av1_fdct8_new;
    case TXFM_TYPE_DCT16:
        return svt_av1_fdct16_new;
    case TXFM_TYPE_DCT32:
        return svt_av1_fdct32_new;
    case TXFM_TYPE_DCT64:
        return svt_av1_fdct64_new;
    case TXFM_TYPE_ADST4:
        return svt_av1_fadst4_new;
    case TXFM_TYPE_ADST8:
        return svt_av1_fadst8_new;
    case TXFM_TYPE_ADST16:
        return svt_av1_fadst16_new;
    case TXFM_TYPE_ADST32:
        return av1_fadst32_new;
    case TXFM_TYPE_IDENTITY4:
        return svt_av1_fidentity4_c;
    case TXFM_TYPE_IDENTITY8:
        return svt_av1_fidentity8_c;
    case TXFM_TYPE_IDENTITY16:
        return svt_av1_fidentity16_c;
    case TXFM_TYPE_IDENTITY32:
        return svt_av1_fidentity32_c;
    case TXFM_TYPE_IDENTITY64:
        return av1_fidentity64_c;
    default:
        assert(0);
        return NULL;
    }
}

//fwd_txfm2d_c
static INLINE void av1_tranform_two_d_core_c(int16_t* input, uint32_t input_stride, int32_t* output,
                                             const Txfm2dFlipCfg* cfg, int32_t* buf, uint8_t bit_depth) {
    int32_t c, r;
    // Note when assigning txfm_size_col, we use the txfm_size from the
    // row configuration and vice versa. This is intentionally done to
    // accurately perform rectangular transforms. When the transform is
    // rectangular, the number of columns will be the same as the
    // txfm_size stored in the row cfg struct. It will make no difference
    // for square transforms.
    const int32_t txfm_size_col = tx_size_wide[cfg->tx_size];
    const int32_t txfm_size_row = tx_size_high[cfg->tx_size];
    // Take the shift from the larger dimension in the rectangular case.
    const int8_t* shift     = cfg->shift;
    const int32_t rect_type = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);
    int8_t        stage_range_col[MAX_TXFM_STAGE_NUM];
    int8_t        stage_range_row[MAX_TXFM_STAGE_NUM];
    assert(cfg->stage_num_col <= MAX_TXFM_STAGE_NUM);
    assert(cfg->stage_num_row <= MAX_TXFM_STAGE_NUM);
    svt_av1_gen_fwd_stage_range(stage_range_col, stage_range_row, cfg, bit_depth);

    const int8_t   cos_bit_col   = cfg->cos_bit_col;
    const int8_t   cos_bit_row   = cfg->cos_bit_row;
    const TxfmFunc txfm_func_col = svt_aom_fwd_txfm_type_to_func(cfg->txfm_type_col);
    const TxfmFunc txfm_func_row = svt_aom_fwd_txfm_type_to_func(cfg->txfm_type_row);
    ASSERT(txfm_func_col != NULL);
    ASSERT(txfm_func_row != NULL);
    // use output buffer as temp buffer
    int32_t* temp_in  = output;
    int32_t* temp_out = output + txfm_size_row;

    // Columns
    for (c = 0; c < txfm_size_col; ++c) {
        if (cfg->ud_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                temp_in[r] = input[r * input_stride + c];
            }
        } else {
            for (r = 0; r < txfm_size_row; ++r) {
                // flip upside down
                temp_in[r] = input[(txfm_size_row - r - 1) * input_stride + c];
            }
        }
        svt_av1_round_shift_array_c(temp_in, txfm_size_row, -shift[0]); // NM svt_av1_round_shift_array_c
        txfm_func_col(temp_in, temp_out, cos_bit_col, stage_range_col);
        svt_av1_round_shift_array_c(temp_out, txfm_size_row, -shift[1]); // NM svt_av1_round_shift_array_c
        if (cfg->lr_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                buf[r * txfm_size_col + c] = temp_out[r];
            }
        } else {
            for (r = 0; r < txfm_size_row; ++r) {
                // flip from left to right
                buf[r * txfm_size_col + (txfm_size_col - c - 1)] = temp_out[r];
            }
        }
    }

    // Rows
    for (r = 0; r < txfm_size_row; ++r) {
        txfm_func_row(buf + r * txfm_size_col, output + r * txfm_size_col, cos_bit_row, stage_range_row);
        svt_av1_round_shift_array_c(output + r * txfm_size_col, txfm_size_col, -shift[2]);

        if (abs(rect_type) == 1) {
            // Multiply everything by Sqrt2 if the transform is rectangular and the
            // size difference is a factor of 2.
            for (c = 0; c < txfm_size_col; ++c) {
                output[r * txfm_size_col + c] = round_shift((int64_t)output[r * txfm_size_col + c] * new_sqrt2,
                                                            new_sqrt2_bits);
            }
        }
    }
}

static INLINE void set_fwd_txfm_non_scale_range(Txfm2dFlipCfg* cfg) {
    av1_zero(cfg->stage_range_col);
    av1_zero(cfg->stage_range_row);

    if (cfg->txfm_type_col == TXFM_TYPE_INVALID) {
        return;
    }

    const int8_t* range_mult2_col = fwd_txfm_range_mult2_list[cfg->txfm_type_col];
    const int32_t stage_num_col   = MIN(cfg->stage_num_col, MAX_TXFM_STAGE_NUM);
    for (int32_t i = 0; i < stage_num_col; ++i) {
        cfg->stage_range_col[i] = (range_mult2_col[i] + 1) >> 1;
    }

    if (cfg->txfm_type_row != TXFM_TYPE_INVALID) {
        const int8_t* range_mult2_row = fwd_txfm_range_mult2_list[cfg->txfm_type_row];
        const int32_t stage_num_row   = MIN(cfg->stage_num_row, MAX_TXFM_STAGE_NUM);
        for (int32_t i = 0; i < stage_num_row; ++i) {
            cfg->stage_range_row[i] = (range_mult2_col[cfg->stage_num_col - 1] + range_mult2_row[i] + 1) >> 1;
        }
    }
}

void svt_aom_transform_config(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg) {
    assert(cfg != NULL);
    cfg->tx_size = tx_size;
    set_flip_cfg(tx_type, cfg);
    const TxType1D tx_type_1d_col = vtx_tab[tx_type];
    const TxType1D tx_type_1d_row = htx_tab[tx_type];
    const int32_t  txw_idx        = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t  txh_idx        = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    cfg->shift                    = fwd_txfm_shift_ls[tx_size];
    cfg->cos_bit_col              = fwd_cos_bit_col[txw_idx][txh_idx];
    cfg->cos_bit_row              = fwd_cos_bit_row[txw_idx][txh_idx];
    cfg->txfm_type_col            = av1_txfm_type_ls[txh_idx][tx_type_1d_col];
    cfg->txfm_type_row            = av1_txfm_type_ls[txw_idx][tx_type_1d_row];
    cfg->stage_num_col            = av1_txfm_stage_num_list[cfg->txfm_type_col];
    cfg->stage_num_row            = av1_txfm_stage_num_list[cfg->txfm_type_row];
    set_fwd_txfm_non_scale_range(cfg);
}

static uint64_t energy_computation(int32_t* coeff, uint32_t coeff_stride, uint32_t area_width, uint32_t area_height) {
    uint64_t prediction_distortion = 0;

    for (uint32_t row_index = 0; row_index < area_height; ++row_index) {
        for (uint32_t column_index = 0; column_index < area_width; ++column_index) {
            prediction_distortion += (int64_t)SQR((int64_t)(coeff[column_index]));
        }
        coeff += coeff_stride;
    }

    return prediction_distortion;
}

uint64_t svt_handle_transform64x64_c(int32_t* output) {
    uint64_t three_quad_energy;

    // top - right 32x32 area.
    three_quad_energy = energy_computation(output + 32, 64, 32, 32);
    //bottom 64x32 area.
    three_quad_energy += energy_computation(output + 32 * 64, 64, 64, 32);

    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        svt_memcpy_c(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return three_quad_energy;
}

void svt_av1_transform_two_d_64x64_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                     uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 64];
    Txfm2dFlipCfg cfg;
    //av1_get_fwd_txfm_cfg
    svt_aom_transform_config(transform_type, TX_64X64, &cfg);
    //fwd_txfm2d_c
    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_transform_two_d_32x32_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                     uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 32];
    Txfm2dFlipCfg cfg;

    svt_aom_transform_config(transform_type, TX_32X32, &cfg);

    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_transform_two_d_16x16_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                     uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 16];
    Txfm2dFlipCfg cfg;

    svt_aom_transform_config(transform_type, TX_16X16, &cfg);

    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_transform_two_d_8x8_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 8];
    Txfm2dFlipCfg cfg;

    svt_aom_transform_config(transform_type, TX_8X8, &cfg);

    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_transform_two_d_4x4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 4];
    Txfm2dFlipCfg cfg;

    svt_aom_transform_config(transform_type, TX_4X4, &cfg);

    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

/*********************************************************************
* Calculate CBF
*********************************************************************/
void svt_av1_fwd_txfm2d_64x32_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 32];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/
    svt_aom_transform_config(transform_type, TX_64X32, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

uint64_t svt_handle_transform64x32_c(int32_t* output) {
    // top - right 32x32 area.
    const uint64_t three_quad_energy = energy_computation(output + 32, 64, 32, 32);

    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        svt_memcpy_c(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return three_quad_energy;
}

void svt_av1_fwd_txfm2d_32x64_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                uint8_t bit_depth) {
    int32_t intermediate_transform_buffer[32 * 64];

    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/
    svt_aom_transform_config(transform_type, TX_32X64, &cfg);
    /*fwd_txfm2d_c*/
    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

uint64_t svt_handle_transform32x64_c(int32_t* output) {
    //bottom 32x32 area.
    const uint64_t three_quad_energy = energy_computation(output + 32 * 32, 32, 32, 32);
    return three_quad_energy;
}

void svt_av1_fwd_txfm2d_64x16_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 16];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/
    svt_aom_transform_config(transform_type, TX_64X16, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

uint64_t svt_handle_transform64x16_c(int32_t* output) {
    // top - right 32x16 area.
    const uint64_t three_quad_energy = energy_computation(output + 32, 64, 32, 16);

    // Re-pack non-zero coeffs in the first 32x16 indices.
    for (int32_t row = 1; row < 16; ++row) {
        svt_memcpy_c(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return three_quad_energy;
}

void svt_av1_fwd_txfm2d_16x64_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                uint8_t bit_depth) {
    int32_t intermediate_transform_buffer[16 * 64];

    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/
    svt_aom_transform_config(transform_type, TX_16X64, &cfg);
    /*fwd_txfm2d_c*/
    av1_tranform_two_d_core_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

uint64_t svt_handle_transform16x64_c(int32_t* output) {
    //bottom 16x32 area.
    const uint64_t three_quad_energy = energy_computation(output + 16 * 32, 16, 16, 32);
    return three_quad_energy;
}

uint64_t svt_handle_transform16x64_N2_N4_c(int32_t* output) {
    (void)output;
    return 0;
}

uint64_t svt_handle_transform32x64_N2_N4_c(int32_t* output) {
    (void)output;
    return 0;
}

uint64_t svt_handle_transform64x16_N2_N4_c(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x16 indices.
    for (int32_t row = 1; row < 16; ++row) {
        svt_memcpy_c(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return 0;
}

uint64_t svt_handle_transform64x32_N2_N4_c(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        svt_memcpy_c(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return 0;
}

uint64_t svt_handle_transform64x64_N2_N4_c(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        svt_memcpy_c(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return 0;
}

void svt_av1_fwd_txfm2d_32x16_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 16];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_32X16, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x32_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 32];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_16X32, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x8_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                               uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 8];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_16X8, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x16_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                               uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 16];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_8X16, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x8_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                               uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 8];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_32X8, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x32_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                               uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 32];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_8X32, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                               uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 4];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_16X4, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_4x16_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                               uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 16];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_4X16, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                              uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 4];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_8X4, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_4x8_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                              uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 8];
    Txfm2dFlipCfg cfg;
    /*av1_get_fwd_txfm_cfg*/ svt_aom_transform_config(transform_type, TX_4X8, &cfg);
    /*fwd_txfm2d_c*/ av1_tranform_two_d_core_c(
        input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

static EbErrorType av1_estimate_transform_N2(int16_t* residual_buffer, uint32_t residual_stride, int32_t* coeff_buffer,
                                             uint32_t coeff_stride, TxSize transform_size, uint64_t* three_quad_energy,
                                             uint32_t bit_depth, TxType transform_type, PlaneType component_type)

{
    EbErrorType return_error = EB_ErrorNone;

    (void)coeff_stride;
    (void)component_type;

    switch (transform_size) {
    case TX_64X32:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_64x32_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_64x32_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform64x32_N2_N4(coeff_buffer);

        break;

    case TX_32X64:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_32x64_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x64_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform32x64_N2_N4(coeff_buffer);

        break;

    case TX_64X16:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_64x16_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_64x16_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform64x16_N2_N4(coeff_buffer);

        break;

    case TX_16X64:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_16x64_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_16x64_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform16x64_N2_N4(coeff_buffer);

        break;

    case TX_32X16:
        // TTK
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_32x16_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x16_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_16X32:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_16x32_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_16x32_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_16X8:
        svt_av1_fwd_txfm2d_16x8_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;

    case TX_8X16:
        svt_av1_fwd_txfm2d_8x16_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;

    case TX_32X8:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_32x8_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x8_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_8X32:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_8x32_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_8x32_N2_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;
    case TX_16X4:
        svt_av1_fwd_txfm2d_16x4_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;
    case TX_4X16:
        svt_av1_fwd_txfm2d_4x16_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;
    case TX_8X4:

        svt_av1_fwd_txfm2d_8x4_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_4X8:

        svt_av1_fwd_txfm2d_4x8_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;

    case TX_64X64:

        svt_av1_fwd_txfm2d_64x64_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        *three_quad_energy = svt_handle_transform64x64_N2_N4(coeff_buffer);

        break;

    case TX_32X32:
        if (transform_type == V_DCT || transform_type == H_DCT || transform_type == V_ADST ||
            transform_type == H_ADST || transform_type == V_FLIPADST || transform_type == H_FLIPADST) {
            // Tahani: I believe those cases are never hit
            svt_aom_transform_two_d_32x32_N2_c(
                residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        else {
            svt_av1_fwd_txfm2d_32x32_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        break;

    case TX_16X16:

        svt_av1_fwd_txfm2d_16x16_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_8X8:

        svt_av1_fwd_txfm2d_8x8_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_4X4:

        svt_av1_fwd_txfm2d_4x4_N2(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    default:
        assert(0);
        break;
    }

    return return_error;
}

static EbErrorType av1_estimate_transform_N4(int16_t* residual_buffer, uint32_t residual_stride, int32_t* coeff_buffer,
                                             uint32_t coeff_stride, TxSize transform_size, uint64_t* three_quad_energy,
                                             uint32_t bit_depth, TxType transform_type, PlaneType component_type)

{
    EbErrorType return_error = EB_ErrorNone;

    (void)coeff_stride;
    (void)component_type;

    switch (transform_size) {
    case TX_64X32:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_64x32_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_64x32_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform64x32_N2_N4(coeff_buffer);

        break;

    case TX_32X64:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_32x64_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x64_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform32x64_N2_N4(coeff_buffer);

        break;

    case TX_64X16:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_64x16_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_64x16_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform64x16_N2_N4(coeff_buffer);

        break;

    case TX_16X64:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_16x64_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_16x64_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform16x64_N2_N4(coeff_buffer);

        break;

    case TX_32X16:
        // TTK
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_32x16_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x16_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_16X32:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_16x32_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_16x32_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_16X8:
        svt_av1_fwd_txfm2d_16x8_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;

    case TX_8X16:
        svt_av1_fwd_txfm2d_8x16_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;

    case TX_32X8:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_32x8_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x8_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_8X32:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_8x32_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_8x32_N4_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;
    case TX_16X4:
        svt_av1_fwd_txfm2d_16x4_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;
    case TX_4X16:
        svt_av1_fwd_txfm2d_4x16_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;
    case TX_8X4:

        svt_av1_fwd_txfm2d_8x4_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_4X8:

        svt_av1_fwd_txfm2d_4x8_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;

    case TX_64X64:

        svt_av1_fwd_txfm2d_64x64_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        *three_quad_energy = svt_handle_transform64x64_N2_N4(coeff_buffer);

        break;

    case TX_32X32:
        if (transform_type == V_DCT || transform_type == H_DCT || transform_type == V_ADST ||
            transform_type == H_ADST || transform_type == V_FLIPADST || transform_type == H_FLIPADST) {
            // Tahani: I believe those cases are never hit
            svt_aom_transform_two_d_32x32_N4_c(
                residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        else {
            svt_av1_fwd_txfm2d_32x32_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        break;

    case TX_16X16:

        svt_av1_fwd_txfm2d_16x16_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_8X8:

        svt_av1_fwd_txfm2d_8x8_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_4X4:

        svt_av1_fwd_txfm2d_4x4_N4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    default:
        assert(0);
        break;
    }

    return return_error;
}

static EbErrorType av1_estimate_transform_ONLY_DC(int16_t* residual_buffer, uint32_t residual_stride,
                                                  int32_t* coeff_buffer, uint32_t coeff_stride, TxSize transform_size,
                                                  uint64_t* three_quad_energy, uint32_t bit_depth,
                                                  TxType transform_type, PlaneType component_type)

{
    EbErrorType return_error = av1_estimate_transform_N4(residual_buffer,
                                                         residual_stride,
                                                         coeff_buffer,
                                                         coeff_stride,
                                                         transform_size,
                                                         three_quad_energy,
                                                         bit_depth,
                                                         transform_type,
                                                         component_type);

    for (int i = 1; i < (tx_size_wide[transform_size] * tx_size_high[transform_size]); i++) {
        if (i % tx_size_wide[transform_size] < (tx_size_wide[transform_size] >> 2) ||
            i / tx_size_wide[transform_size] < (tx_size_high[transform_size] >> 2)) {
            coeff_buffer[i] = 0;
        }
    }
    return return_error;
}

static EbErrorType av1_estimate_transform_default(int16_t* residual_buffer, uint32_t residual_stride,
                                                  int32_t* coeff_buffer, uint32_t coeff_stride, TxSize transform_size,
                                                  uint64_t* three_quad_energy, uint32_t bit_depth,
                                                  TxType transform_type, PlaneType component_type)

{
    EbErrorType return_error = EB_ErrorNone;

    (void)coeff_stride;
    (void)component_type;

    switch (transform_size) {
    case TX_64X32:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_64x32(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_64x32_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform64x32(coeff_buffer);

        break;

    case TX_32X64:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_32x64(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x64_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform32x64(coeff_buffer);

        break;

    case TX_64X16:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_64x16(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_64x16_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform64x16(coeff_buffer);

        break;

    case TX_16X64:
        if (transform_type == DCT_DCT) {
            svt_av1_fwd_txfm2d_16x64(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_16x64_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        *three_quad_energy = svt_handle_transform16x64(coeff_buffer);

        break;

    case TX_32X16:
        // TTK
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_32x16(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x16_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_16X32:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_16x32(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_16x32_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_16X8:
        svt_av1_fwd_txfm2d_16x8(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;

    case TX_8X16:
        svt_av1_fwd_txfm2d_8x16(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;

    case TX_32X8:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_32x8(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_32x8_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;

    case TX_8X32:
        if ((transform_type == DCT_DCT) || (transform_type == IDTX)) {
            svt_av1_fwd_txfm2d_8x32(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        } else {
            svt_av1_fwd_txfm2d_8x32_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }
        break;
    case TX_16X4:
        svt_av1_fwd_txfm2d_16x4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;
    case TX_4X16:
        svt_av1_fwd_txfm2d_4x16(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        break;
    case TX_8X4:

        svt_av1_fwd_txfm2d_8x4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_4X8:

        svt_av1_fwd_txfm2d_4x8(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;

    case TX_64X64:

        svt_av1_fwd_txfm2d_64x64(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        *three_quad_energy = svt_handle_transform64x64(coeff_buffer);

        break;

    case TX_32X32:
        if (transform_type == V_DCT || transform_type == H_DCT || transform_type == V_ADST ||
            transform_type == H_ADST || transform_type == V_FLIPADST || transform_type == H_FLIPADST) {
            // Tahani: I believe those cases are never hit
            svt_av1_transform_two_d_32x32_c(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        else {
            svt_av1_fwd_txfm2d_32x32(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);
        }

        break;

    case TX_16X16:

        svt_av1_fwd_txfm2d_16x16(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_8X8:

        svt_av1_fwd_txfm2d_8x8(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    case TX_4X4:

        svt_av1_fwd_txfm2d_4x4(residual_buffer, coeff_buffer, residual_stride, transform_type, bit_depth);

        break;
    default:
        assert(0);
        break;
    }

    return return_error;
}

/* 4-point reversible, orthonormal Walsh-Hadamard in 3.5 adds, 0.5 shifts per
   pixel.
   Shared for both high and low bit depth.
 */
void svt_av1_fwht4x4_c(int16_t* input, int32_t* output, uint32_t stride) {
    int            i;
    int64_t        a1, b1, c1, d1, e1;
    const int16_t* ip_pass0 = input;
    const int32_t* ip       = NULL;
    int32_t*       op       = output;

    for (i = 0; i < 4; i++) {
        a1 = ip_pass0[0 * stride];
        b1 = ip_pass0[1 * stride];
        c1 = ip_pass0[2 * stride];
        d1 = ip_pass0[3 * stride];

        a1 += b1;
        d1 = d1 - c1;
        e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;
        op[0] = (int32_t)a1;
        op[1] = (int32_t)c1;
        op[2] = (int32_t)d1;
        op[3] = (int32_t)b1;

        ip_pass0++;
        op += 4;
    }
    ip = output;
    op = output;

    for (i = 0; i < 4; i++) {
        a1 = ip[4 * 0];
        b1 = ip[4 * 1];
        c1 = ip[4 * 2];
        d1 = ip[4 * 3];

        a1 += b1;
        d1 -= c1;
        e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;
        op[4 * 0] = (int32_t)(a1 * UNIT_QUANT_FACTOR);
        op[4 * 1] = (int32_t)(c1 * UNIT_QUANT_FACTOR);
        op[4 * 2] = (int32_t)(d1 * UNIT_QUANT_FACTOR);
        op[4 * 3] = (int32_t)(b1 * UNIT_QUANT_FACTOR);

        ip++;
        op++;
    }
}

/*********************************************************************
* Transform
*   Note there is an implicit assumption that TU Size <= PU Size,
*   which is different than the HEVC requirements.
*********************************************************************/
EbErrorType svt_aom_estimate_transform(PictureControlSet* pcs, ModeDecisionContext* ctx, int16_t* residual_buffer,
                                       uint32_t residual_stride, int32_t* coeff_buffer, uint32_t coeff_stride,
                                       TxSize transform_size, uint64_t* three_quad_energy, uint32_t bit_depth,
                                       TxType transform_type, PlaneType component_type,
                                       EB_TRANS_COEFF_SHAPE trans_coeff_shape)

{
    (void)trans_coeff_shape;
    (void)coeff_stride;
    (void)component_type;

    if (svt_av1_is_lossless_segment(pcs, ctx->blk_ptr->segment_id)) {
        assert(transform_type == DCT_DCT);
        int32_t dst[16];

        svt_av1_fwht4x4(residual_buffer, dst, residual_stride);
        for (int i = 0; i < 4; i++) {
            for (int j = 0; j < 4; j++) {
                coeff_buffer[(j << 2) + i] = dst[(i << 2) + j];
            }
        }
        return EB_ErrorNone;
    }

    switch (trans_coeff_shape) {
    case DEFAULT_SHAPE:
        return av1_estimate_transform_default(residual_buffer,
                                              residual_stride,
                                              coeff_buffer,
                                              coeff_stride,
                                              transform_size,
                                              three_quad_energy,
                                              bit_depth,
                                              transform_type,
                                              component_type);
    case N2_SHAPE:
        return av1_estimate_transform_N2(residual_buffer,
                                         residual_stride,
                                         coeff_buffer,
                                         coeff_stride,
                                         transform_size,
                                         three_quad_energy,
                                         bit_depth,
                                         transform_type,
                                         component_type);
    case N4_SHAPE:
        return av1_estimate_transform_N4(residual_buffer,
                                         residual_stride,
                                         coeff_buffer,
                                         coeff_stride,
                                         transform_size,
                                         three_quad_energy,
                                         bit_depth,
                                         transform_type,
                                         component_type);
    case ONLY_DC_SHAPE:
        return av1_estimate_transform_ONLY_DC(residual_buffer,
                                              residual_stride,
                                              coeff_buffer,
                                              coeff_stride,
                                              transform_size,
                                              three_quad_energy,
                                              bit_depth,
                                              transform_type,
                                              component_type);
    }

    assert(0);
    return EB_ErrorBadParameter;
}

// PF_N4
static void highbd_fwd_txfm_64x64_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x64_N4(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_32x64_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_32x64_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, bd);
}

static void highbd_fwd_txfm_64x32_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x32_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, bd);
}

static void highbd_fwd_txfm_16x64_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x64_N4(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_64x16_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x16_N4(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_32x32_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_32x32_N4(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x16_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x16_N4(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_8x8_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_8x8_N4(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_4x8_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x8_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x4_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_8x4_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x16_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_8x16_N4(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x8_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x8_N4(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x32_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_16x32_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_32x16_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_32x16_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_4x16_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x16(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_16x4_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_16x4_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x32_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_8x32_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_32x8_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_32x8_N4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

//PF_N2
static void highbd_fwd_txfm_64x64_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x64_N2(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_32x64_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_32x64_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, bd);
}

static void highbd_fwd_txfm_64x32_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x32_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, bd);
}

static void highbd_fwd_txfm_16x64_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x64_N2(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_64x16_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x16_N2(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_32x32_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_32x32_N2(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x16_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x16_N2(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_8x8_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_8x8_N2(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_4x8_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x8_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x4_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_8x4_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x16_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_8x16_N2(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x8_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x8_N2(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x32_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_16x32_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_32x16_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_32x16_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_4x16_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x16(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_16x4_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_16x4_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x32_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_8x32_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_32x8_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_32x8_N2(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_64x64(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x64(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_32x64(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_32x64(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, bd);
}

static void highbd_fwd_txfm_64x32(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x32(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, bd);
}

static void highbd_fwd_txfm_16x64(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x64(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_64x16(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(txfm_param->tx_type == DCT_DCT);
    int32_t*  dst_coeff = (int32_t*)coeff;
    const int bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_64x16(src_diff, dst_coeff, diff_stride, DCT_DCT, bd);
}

static void highbd_fwd_txfm_32x32(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_32x32(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x16(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x16(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_8x8(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_8x8(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_4x8(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x8(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_8x4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x16(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_8x16(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x8(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t*     dst_coeff = (int32_t*)coeff;
    const TxType tx_type   = txfm_param->tx_type;
    const int    bd        = txfm_param->bd;
    svt_av1_fwd_txfm2d_16x8(src_diff, dst_coeff, diff_stride, tx_type, bd);
}

static void highbd_fwd_txfm_16x32(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_16x32(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_32x16(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_32x16(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_4x16(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_4x16(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_16x4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_16x4(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_8x32(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_8x32(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

static void highbd_fwd_txfm_32x8(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    int32_t* dst_coeff = (int32_t*)coeff;
    svt_av1_fwd_txfm2d_32x8(src_diff, dst_coeff, diff_stride, txfm_param->tx_type, txfm_param->bd);
}

void svt_av1_highbd_fwd_txfm_n4(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    const TxSize tx_size = txfm_param->tx_size;
    switch (tx_size) {
    case TX_64X64:
        highbd_fwd_txfm_64x64_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X64:
        highbd_fwd_txfm_32x64_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_64X32:
        highbd_fwd_txfm_64x32_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X64:
        highbd_fwd_txfm_16x64_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_64X16:
        highbd_fwd_txfm_64x16_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X32:
        highbd_fwd_txfm_32x32_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X16:
        highbd_fwd_txfm_16x16_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X8:
        highbd_fwd_txfm_8x8_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X8:
        highbd_fwd_txfm_4x8_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X4:
        highbd_fwd_txfm_8x4_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X16:
        highbd_fwd_txfm_8x16_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X8:
        highbd_fwd_txfm_16x8_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X32:
        highbd_fwd_txfm_16x32_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X16:
        highbd_fwd_txfm_32x16_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X4:
        //hack highbd_fwd_txfm_4x4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X16:
        highbd_fwd_txfm_4x16_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X4:
        highbd_fwd_txfm_16x4_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X32:
        highbd_fwd_txfm_8x32_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X8:
        highbd_fwd_txfm_32x8_n4(src_diff, coeff, diff_stride, txfm_param);
        break;
    default:
        assert(0);
        break;
    }
}

void svt_av1_highbd_fwd_txfm_n2(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    const TxSize tx_size = txfm_param->tx_size;
    switch (tx_size) {
    case TX_64X64:
        highbd_fwd_txfm_64x64_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X64:
        highbd_fwd_txfm_32x64_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_64X32:
        highbd_fwd_txfm_64x32_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X64:
        highbd_fwd_txfm_16x64_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_64X16:
        highbd_fwd_txfm_64x16_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X32:
        highbd_fwd_txfm_32x32_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X16:
        highbd_fwd_txfm_16x16_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X8:
        highbd_fwd_txfm_8x8_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X8:
        highbd_fwd_txfm_4x8_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X4:
        highbd_fwd_txfm_8x4_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X16:
        highbd_fwd_txfm_8x16_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X8:
        highbd_fwd_txfm_16x8_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X32:
        highbd_fwd_txfm_16x32_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X16:
        highbd_fwd_txfm_32x16_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X4:
        //hack highbd_fwd_txfm_4x4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X16:
        highbd_fwd_txfm_4x16_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X4:
        highbd_fwd_txfm_16x4_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X32:
        highbd_fwd_txfm_8x32_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X8:
        highbd_fwd_txfm_32x8_n2(src_diff, coeff, diff_stride, txfm_param);
        break;
    default:
        assert(0);
        break;
    }
}

void svt_av1_highbd_fwd_txfm(int16_t* src_diff, TranLow* coeff, int diff_stride, TxfmParam* txfm_param) {
    assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    const TxSize tx_size = txfm_param->tx_size;
    switch (tx_size) {
    case TX_64X64:
        highbd_fwd_txfm_64x64(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X64:
        highbd_fwd_txfm_32x64(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_64X32:
        highbd_fwd_txfm_64x32(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X64:
        highbd_fwd_txfm_16x64(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_64X16:
        highbd_fwd_txfm_64x16(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X32:
        highbd_fwd_txfm_32x32(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X16:
        highbd_fwd_txfm_16x16(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X8:
        highbd_fwd_txfm_8x8(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X8:
        highbd_fwd_txfm_4x8(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X4:
        highbd_fwd_txfm_8x4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X16:
        highbd_fwd_txfm_8x16(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X8:
        highbd_fwd_txfm_16x8(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X32:
        highbd_fwd_txfm_16x32(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X16:
        highbd_fwd_txfm_32x16(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X4:
        //hack highbd_fwd_txfm_4x4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_4X16:
        highbd_fwd_txfm_4x16(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_16X4:
        highbd_fwd_txfm_16x4(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_8X32:
        highbd_fwd_txfm_8x32(src_diff, coeff, diff_stride, txfm_param);
        break;
    case TX_32X8:
        highbd_fwd_txfm_32x8(src_diff, coeff, diff_stride, txfm_param);
        break;
    default:
        assert(0);
        break;
    }
}

void svt_av1_wht_fwd_txfm(int16_t* src_diff, int bw, int32_t* coeff, TxSize tx_size, EB_TRANS_COEFF_SHAPE pf_shape,
                          int bit_depth, int is_hbd) {
    TxfmParam txfm_param;
    txfm_param.tx_type     = DCT_DCT;
    txfm_param.tx_size     = tx_size;
    txfm_param.lossless    = 0;
    txfm_param.tx_set_type = EXT_TX_SET_ALL16;

    txfm_param.bd     = bit_depth;
    txfm_param.is_hbd = is_hbd;
    switch (pf_shape) {
    case N4_SHAPE:
        svt_av1_highbd_fwd_txfm_n4(src_diff, coeff, bw, &txfm_param);
        break;
    case N2_SHAPE:
        svt_av1_highbd_fwd_txfm_n2(src_diff, coeff, bw, &txfm_param);
        break;
    default:
        svt_av1_highbd_fwd_txfm(src_diff, coeff, bw, &txfm_param);
    }
}

void svt_av1_fidentity16_N2_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 8; ++i) {
        output[i] = round_shift((int64_t)input[i] * 2 * new_sqrt2, new_sqrt2_bits);
    }

    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_fadst16_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    assert(output != input);
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = -input[15];
    bf1[2]  = -input[7];
    bf1[3]  = input[8];
    bf1[4]  = -input[3];
    bf1[5]  = input[12];
    bf1[6]  = input[4];
    bf1[7]  = -input[11];
    bf1[8]  = -input[1];
    bf1[9]  = input[14];
    bf1[10] = input[6];
    bf1[11] = -input[9];
    bf1[12] = input[2];
    bf1[13] = -input[13];
    bf1[14] = -input[5];
    bf1[15] = input[10];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[32], bf0[2], -cospi[32], bf0[3], cos_bit);
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[32], bf0[6], -cospi[32], bf0[7], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(cospi[32], bf0[10], cospi[32], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[32], bf0[10], -cospi[32], bf0[11], cos_bit);
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = half_btf(cospi[32], bf0[14], cospi[32], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[32], bf0[14], -cospi[32], bf0[15], cos_bit);

    // stage 3
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[2];
    bf1[1]  = bf0[1] + bf0[3];
    bf1[2]  = bf0[0] - bf0[2];
    bf1[3]  = bf0[1] - bf0[3];
    bf1[4]  = bf0[4] + bf0[6];
    bf1[5]  = bf0[5] + bf0[7];
    bf1[6]  = bf0[4] - bf0[6];
    bf1[7]  = bf0[5] - bf0[7];
    bf1[8]  = bf0[8] + bf0[10];
    bf1[9]  = bf0[9] + bf0[11];
    bf1[10] = bf0[8] - bf0[10];
    bf1[11] = bf0[9] - bf0[11];
    bf1[12] = bf0[12] + bf0[14];
    bf1[13] = bf0[13] + bf0[15];
    bf1[14] = bf0[12] - bf0[14];
    bf1[15] = bf0[13] - bf0[15];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5]  = half_btf(cospi[48], bf0[4], -cospi[16], bf0[5], cos_bit);
    bf1[6]  = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[16], bf0[6], cospi[48], bf0[7], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = half_btf(cospi[16], bf0[12], cospi[48], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[48], bf0[12], -cospi[16], bf0[13], cos_bit);
    bf1[14] = half_btf(-cospi[48], bf0[14], cospi[16], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[16], bf0[14], cospi[48], bf0[15], cos_bit);

    // stage 5
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[4];
    bf1[1]  = bf0[1] + bf0[5];
    bf1[2]  = bf0[2] + bf0[6];
    bf1[3]  = bf0[3] + bf0[7];
    bf1[4]  = bf0[0] - bf0[4];
    bf1[5]  = bf0[1] - bf0[5];
    bf1[6]  = bf0[2] - bf0[6];
    bf1[7]  = bf0[3] - bf0[7];
    bf1[8]  = bf0[8] + bf0[12];
    bf1[9]  = bf0[9] + bf0[13];
    bf1[10] = bf0[10] + bf0[14];
    bf1[11] = bf0[11] + bf0[15];
    bf1[12] = bf0[8] - bf0[12];
    bf1[13] = bf0[9] - bf0[13];
    bf1[14] = bf0[10] - bf0[14];
    bf1[15] = bf0[11] - bf0[15];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[8], bf0[8], cospi[56], bf0[9], cos_bit);
    bf1[9]  = half_btf(cospi[56], bf0[8], -cospi[8], bf0[9], cos_bit);
    bf1[10] = half_btf(cospi[40], bf0[10], cospi[24], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[24], bf0[10], -cospi[40], bf0[11], cos_bit);
    bf1[12] = half_btf(-cospi[56], bf0[12], cospi[8], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[8], bf0[12], cospi[56], bf0[13], cos_bit);
    bf1[14] = half_btf(-cospi[24], bf0[14], cospi[40], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[40], bf0[14], cospi[24], bf0[15], cos_bit);

    // stage 7
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[8];
    bf1[1]  = bf0[1] + bf0[9];
    bf1[2]  = bf0[2] + bf0[10];
    bf1[3]  = bf0[3] + bf0[11];
    bf1[4]  = bf0[4] + bf0[12];
    bf1[5]  = bf0[5] + bf0[13];
    bf1[6]  = bf0[6] + bf0[14];
    bf1[7]  = bf0[7] + bf0[15];
    bf1[8]  = bf0[0] - bf0[8];
    bf1[9]  = bf0[1] - bf0[9];
    bf1[10] = bf0[2] - bf0[10];
    bf1[11] = bf0[3] - bf0[11];
    bf1[12] = bf0[4] - bf0[12];
    bf1[13] = bf0[5] - bf0[13];
    bf1[14] = bf0[6] - bf0[14];
    bf1[15] = bf0[7] - bf0[15];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[1]  = half_btf(cospi[62], bf0[0], -cospi[2], bf0[1], cos_bit);
    bf1[3]  = half_btf(cospi[54], bf0[2], -cospi[10], bf0[3], cos_bit);
    bf1[5]  = half_btf(cospi[46], bf0[4], -cospi[18], bf0[5], cos_bit);
    bf1[7]  = half_btf(cospi[38], bf0[6], -cospi[26], bf0[7], cos_bit);
    bf1[8]  = half_btf(cospi[34], bf0[8], cospi[30], bf0[9], cos_bit);
    bf1[10] = half_btf(cospi[42], bf0[10], cospi[22], bf0[11], cos_bit);
    bf1[12] = half_btf(cospi[50], bf0[12], cospi[14], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[58], bf0[14], cospi[6], bf0[15], cos_bit);

    // stage 9
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[1];
    bf1[1] = bf0[14];
    bf1[2] = bf0[3];
    bf1[3] = bf0[12];
    bf1[4] = bf0[5];
    bf1[5] = bf0[10];
    bf1[6] = bf0[7];
    bf1[7] = bf0[8];
}

void svt_av1_fdct16_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[15];
    bf1[1]  = input[1] + input[14];
    bf1[2]  = input[2] + input[13];
    bf1[3]  = input[3] + input[12];
    bf1[4]  = input[4] + input[11];
    bf1[5]  = input[5] + input[10];
    bf1[6]  = input[6] + input[9];
    bf1[7]  = input[7] + input[8];
    bf1[8]  = -input[8] + input[7];
    bf1[9]  = -input[9] + input[6];
    bf1[10] = -input[10] + input[5];
    bf1[11] = -input[11] + input[4];
    bf1[12] = -input[12] + input[3];
    bf1[13] = -input[13] + input[2];
    bf1[14] = -input[14] + input[1];
    bf1[15] = -input[15] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[2]  = -bf0[2] + bf0[1];
    bf1[3]  = -bf0[3] + bf0[0];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[5]  = -bf0[5] + bf0[4];
    bf1[6]  = -bf0[6] + bf0[7];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[6]  = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[9]  = -bf0[9] + bf0[8];
    bf1[10] = -bf0[10] + bf0[11];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[13] = -bf0[13] + bf0[12];
    bf1[14] = -bf0[14] + bf0[15];
    bf1[15] = bf0[15] + bf0[14];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = bf0[4];
    bf1[6]  = bf0[6];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], cospi[20], bf0[13], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[14] = half_btf(cospi[28], bf0[14], -cospi[36], bf0[9], cos_bit);

    // stage 7
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[8];
    bf1[2] = bf0[4];
    bf1[3] = bf0[12];
    bf1[4] = bf0[2];
    bf1[5] = bf0[10];
    bf1[6] = bf0[6];
    bf1[7] = bf0[14];
}

void svt_av1_fidentity8_N2_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 4; ++i) {
        output[i] = input[i] * 2;
    }
}

void svt_av1_fadst8_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    assert(output != input);
    bf1    = output;
    bf1[0] = input[0];
    bf1[1] = -input[7];
    bf1[2] = -input[3];
    bf1[3] = input[4];
    bf1[4] = -input[1];
    bf1[5] = input[6];
    bf1[6] = input[2];
    bf1[7] = -input[5];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[32], bf0[2], -cospi[32], bf0[3], cos_bit);
    bf1[4] = bf0[4];
    bf1[5] = bf0[5];
    bf1[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[32], bf0[6], -cospi[32], bf0[7], cos_bit);

    // stage 3
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0] + bf0[2];
    bf1[1] = bf0[1] + bf0[3];
    bf1[2] = bf0[0] - bf0[2];
    bf1[3] = bf0[1] - bf0[3];
    bf1[4] = bf0[4] + bf0[6];
    bf1[5] = bf0[5] + bf0[7];
    bf1[6] = bf0[4] - bf0[6];
    bf1[7] = bf0[5] - bf0[7];

    // stage 4
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = bf0[2];
    bf1[3] = bf0[3];
    bf1[4] = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5] = half_btf(cospi[48], bf0[4], -cospi[16], bf0[5], cos_bit);
    bf1[6] = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[16], bf0[6], cospi[48], bf0[7], cos_bit);

    // stage 5
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0] + bf0[4];
    bf1[1] = bf0[1] + bf0[5];
    bf1[2] = bf0[2] + bf0[6];
    bf1[3] = bf0[3] + bf0[7];
    bf1[4] = bf0[0] - bf0[4];
    bf1[5] = bf0[1] - bf0[5];
    bf1[6] = bf0[2] - bf0[6];
    bf1[7] = bf0[3] - bf0[7];

    // stage 6
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[1] = half_btf(cospi[60], bf0[0], -cospi[4], bf0[1], cos_bit);
    bf1[3] = half_btf(cospi[44], bf0[2], -cospi[20], bf0[3], cos_bit);
    bf1[4] = half_btf(cospi[36], bf0[4], cospi[28], bf0[5], cos_bit);
    bf1[6] = half_btf(cospi[52], bf0[6], cospi[12], bf0[7], cos_bit);

    // stage 7
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[1];
    bf1[1] = bf0[6];
    bf1[2] = bf0[3];
    bf1[3] = bf0[4];
}

void svt_av1_fdct8_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    bf1    = output;
    bf1[0] = input[0] + input[7];
    bf1[1] = input[1] + input[6];
    bf1[2] = input[2] + input[5];
    bf1[3] = input[3] + input[4];
    bf1[4] = -input[4] + input[3];
    bf1[5] = -input[5] + input[2];
    bf1[6] = -input[6] + input[1];
    bf1[7] = -input[7] + input[0];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0] + bf0[3];
    bf1[1] = bf0[1] + bf0[2];
    bf1[2] = -bf0[2] + bf0[1];
    bf1[3] = -bf0[3] + bf0[0];
    bf1[4] = bf0[4];
    bf1[5] = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7] = bf0[7];

    // stage 3
    cospi  = cospi_arr(cos_bit);
    bf0    = step;
    bf1    = output;
    bf1[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[2] = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[4] = bf0[4] + bf0[5];
    bf1[5] = -bf0[5] + bf0[4];
    bf1[6] = -bf0[6] + bf0[7];
    bf1[7] = bf0[7] + bf0[6];

    // stage 4
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[2] = bf0[2];
    bf1[4] = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[6] = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);

    // stage 5
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[4];
    bf1[2] = bf0[2];
    bf1[3] = bf0[6];
}

void svt_av1_fidentity4_N2_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    output[0] = round_shift((int64_t)input[0] * new_sqrt2, new_sqrt2_bits);
    output[1] = round_shift((int64_t)input[1] * new_sqrt2, new_sqrt2_bits);
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_fadst4_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    int32_t        bit   = cos_bit;
    const int32_t* sinpi = sinpi_arr(bit);
    int32_t        x0, x1, x2, x3;
    int32_t        s0, s2, s4, s5, s7;

    // stage 0
    x0 = input[0];
    x1 = input[1];
    x2 = input[2];
    x3 = input[3];

    if (!(x0 | x1 | x2 | x3)) {
        output[0] = output[1] = output[2] = output[3] = 0;
        return;
    }

    // stage 1
    s0 = sinpi[1] * x0;
    s2 = sinpi[2] * x1;
    s4 = sinpi[3] * x2;
    s5 = sinpi[4] * x3;
    s7 = x0 + x1;

    // stage 2
    s7 = s7 - x3;

    // stage 3
    x0 = s0 + s2;
    x1 = sinpi[3] * s7;

    // stage 4
    x0 = x0 + s5;

    // stage 5
    s0 = x0 + s4;

    // 1-D transform scaling factor is sqrt(2).
    output[0] = round_shift(s0, bit);
    output[1] = round_shift(x1, bit);
}

void svt_av1_fdct4_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t* bf0;
    int32_t  step[4];

    // stage 1;
    bf0    = step;
    bf0[0] = input[0] + input[3];
    bf0[1] = input[1] + input[2];
    bf0[2] = -input[2] + input[1];
    bf0[3] = -input[3] + input[0];

    // stage 2
    cospi = cospi_arr(cos_bit);

    output[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    output[1] = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
}

void svt_av1_fdct32_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[32];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[31];
    bf1[1]  = input[1] + input[30];
    bf1[2]  = input[2] + input[29];
    bf1[3]  = input[3] + input[28];
    bf1[4]  = input[4] + input[27];
    bf1[5]  = input[5] + input[26];
    bf1[6]  = input[6] + input[25];
    bf1[7]  = input[7] + input[24];
    bf1[8]  = input[8] + input[23];
    bf1[9]  = input[9] + input[22];
    bf1[10] = input[10] + input[21];
    bf1[11] = input[11] + input[20];
    bf1[12] = input[12] + input[19];
    bf1[13] = input[13] + input[18];
    bf1[14] = input[14] + input[17];
    bf1[15] = input[15] + input[16];
    bf1[16] = -input[16] + input[15];
    bf1[17] = -input[17] + input[14];
    bf1[18] = -input[18] + input[13];
    bf1[19] = -input[19] + input[12];
    bf1[20] = -input[20] + input[11];
    bf1[21] = -input[21] + input[10];
    bf1[22] = -input[22] + input[9];
    bf1[23] = -input[23] + input[8];
    bf1[24] = -input[24] + input[7];
    bf1[25] = -input[25] + input[6];
    bf1[26] = -input[26] + input[5];
    bf1[27] = -input[27] + input[4];
    bf1[28] = -input[28] + input[3];
    bf1[29] = -input[29] + input[2];
    bf1[30] = -input[30] + input[1];
    bf1[31] = -input[31] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[15];
    bf1[1]  = bf0[1] + bf0[14];
    bf1[2]  = bf0[2] + bf0[13];
    bf1[3]  = bf0[3] + bf0[12];
    bf1[4]  = bf0[4] + bf0[11];
    bf1[5]  = bf0[5] + bf0[10];
    bf1[6]  = bf0[6] + bf0[9];
    bf1[7]  = bf0[7] + bf0[8];
    bf1[8]  = -bf0[8] + bf0[7];
    bf1[9]  = -bf0[9] + bf0[6];
    bf1[10] = -bf0[10] + bf0[5];
    bf1[11] = -bf0[11] + bf0[4];
    bf1[12] = -bf0[12] + bf0[3];
    bf1[13] = -bf0[13] + bf0[2];
    bf1[14] = -bf0[14] + bf0[1];
    bf1[15] = -bf0[15] + bf0[0];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[24], cospi[32], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[25], cospi[32], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[27], cospi[32], bf0[20], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[23];
    bf1[17] = bf0[17] + bf0[22];
    bf1[18] = bf0[18] + bf0[21];
    bf1[19] = bf0[19] + bf0[20];
    bf1[20] = -bf0[20] + bf0[19];
    bf1[21] = -bf0[21] + bf0[18];
    bf1[22] = -bf0[22] + bf0[17];
    bf1[23] = -bf0[23] + bf0[16];
    bf1[24] = -bf0[24] + bf0[31];
    bf1[25] = -bf0[25] + bf0[30];
    bf1[26] = -bf0[26] + bf0[29];
    bf1[27] = -bf0[27] + bf0[28];
    bf1[28] = bf0[28] + bf0[27];
    bf1[29] = bf0[29] + bf0[26];
    bf1[30] = bf0[30] + bf0[25];
    bf1[31] = bf0[31] + bf0[24];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[2]  = -bf0[2] + bf0[1];
    bf1[3]  = -bf0[3] + bf0[0];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(-cospi[16], bf0[18], cospi[48], bf0[29], cos_bit);
    bf1[19] = half_btf(-cospi[16], bf0[19], cospi[48], bf0[28], cos_bit);
    bf1[20] = half_btf(-cospi[48], bf0[20], -cospi[16], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[48], bf0[21], -cospi[16], bf0[26], cos_bit);
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[48], bf0[26], -cospi[16], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[48], bf0[27], -cospi[16], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[16], bf0[29], cospi[48], bf0[18], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[5]  = -bf0[5] + bf0[4];
    bf1[6]  = -bf0[6] + bf0[7];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[19];
    bf1[17] = bf0[17] + bf0[18];
    bf1[18] = -bf0[18] + bf0[17];
    bf1[19] = -bf0[19] + bf0[16];
    bf1[20] = -bf0[20] + bf0[23];
    bf1[21] = -bf0[21] + bf0[22];
    bf1[22] = bf0[22] + bf0[21];
    bf1[23] = bf0[23] + bf0[20];
    bf1[24] = bf0[24] + bf0[27];
    bf1[25] = bf0[25] + bf0[26];
    bf1[26] = -bf0[26] + bf0[25];
    bf1[27] = -bf0[27] + bf0[24];
    bf1[28] = -bf0[28] + bf0[31];
    bf1[29] = -bf0[29] + bf0[30];
    bf1[30] = bf0[30] + bf0[29];
    bf1[31] = bf0[31] + bf0[28];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[6]  = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[9]  = -bf0[9] + bf0[8];
    bf1[10] = -bf0[10] + bf0[11];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[13] = -bf0[13] + bf0[12];
    bf1[14] = -bf0[14] + bf0[15];
    bf1[15] = bf0[15] + bf0[14];
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(cospi[24], bf0[25], -cospi[40], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[21], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(cospi[56], bf0[29], -cospi[8], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[8], bf0[30], cospi[56], bf0[17], cos_bit);
    bf1[31] = bf0[31];

    // stage 7
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = bf0[4];
    bf1[6]  = bf0[6];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], cospi[20], bf0[13], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[14] = half_btf(cospi[28], bf0[14], -cospi[36], bf0[9], cos_bit);
    bf1[16] = bf0[16] + bf0[17];
    bf1[17] = -bf0[17] + bf0[16];
    bf1[18] = -bf0[18] + bf0[19];
    bf1[19] = bf0[19] + bf0[18];
    bf1[20] = bf0[20] + bf0[21];
    bf1[21] = -bf0[21] + bf0[20];
    bf1[22] = -bf0[22] + bf0[23];
    bf1[23] = bf0[23] + bf0[22];
    bf1[24] = bf0[24] + bf0[25];
    bf1[25] = -bf0[25] + bf0[24];
    bf1[26] = -bf0[26] + bf0[27];
    bf1[27] = bf0[27] + bf0[26];
    bf1[28] = bf0[28] + bf0[29];
    bf1[29] = -bf0[29] + bf0[28];
    bf1[30] = -bf0[30] + bf0[31];
    bf1[31] = bf0[31] + bf0[30];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = bf0[4];
    bf1[6]  = bf0[6];
    bf1[8]  = bf0[8];
    bf1[10] = bf0[10];
    bf1[12] = bf0[12];
    bf1[14] = bf0[14];
    bf1[16] = half_btf(cospi[62], bf0[16], cospi[2], bf0[31], cos_bit);
    bf1[18] = half_btf(cospi[46], bf0[18], cospi[18], bf0[29], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], cospi[10], bf0[27], cos_bit);
    bf1[22] = half_btf(cospi[38], bf0[22], cospi[26], bf0[25], cos_bit);
    bf1[24] = half_btf(cospi[6], bf0[24], -cospi[58], bf0[23], cos_bit);
    bf1[26] = half_btf(cospi[22], bf0[26], -cospi[42], bf0[21], cos_bit);
    bf1[28] = half_btf(cospi[14], bf0[28], -cospi[50], bf0[19], cos_bit);
    bf1[30] = half_btf(cospi[30], bf0[30], -cospi[34], bf0[17], cos_bit);

    // stage 9
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[16];
    bf1[2]  = bf0[8];
    bf1[3]  = bf0[24];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[20];
    bf1[6]  = bf0[12];
    bf1[7]  = bf0[28];
    bf1[8]  = bf0[2];
    bf1[9]  = bf0[18];
    bf1[10] = bf0[10];
    bf1[11] = bf0[26];
    bf1[12] = bf0[6];
    bf1[13] = bf0[22];
    bf1[14] = bf0[14];
    bf1[15] = bf0[30];
}

void svt_av1_fidentity32_N2_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 16; ++i) {
        output[i] = input[i] * 4;
    }
}

void svt_av1_fdct64_new_N2(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[64];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[63];
    bf1[1]  = input[1] + input[62];
    bf1[2]  = input[2] + input[61];
    bf1[3]  = input[3] + input[60];
    bf1[4]  = input[4] + input[59];
    bf1[5]  = input[5] + input[58];
    bf1[6]  = input[6] + input[57];
    bf1[7]  = input[7] + input[56];
    bf1[8]  = input[8] + input[55];
    bf1[9]  = input[9] + input[54];
    bf1[10] = input[10] + input[53];
    bf1[11] = input[11] + input[52];
    bf1[12] = input[12] + input[51];
    bf1[13] = input[13] + input[50];
    bf1[14] = input[14] + input[49];
    bf1[15] = input[15] + input[48];
    bf1[16] = input[16] + input[47];
    bf1[17] = input[17] + input[46];
    bf1[18] = input[18] + input[45];
    bf1[19] = input[19] + input[44];
    bf1[20] = input[20] + input[43];
    bf1[21] = input[21] + input[42];
    bf1[22] = input[22] + input[41];
    bf1[23] = input[23] + input[40];
    bf1[24] = input[24] + input[39];
    bf1[25] = input[25] + input[38];
    bf1[26] = input[26] + input[37];
    bf1[27] = input[27] + input[36];
    bf1[28] = input[28] + input[35];
    bf1[29] = input[29] + input[34];
    bf1[30] = input[30] + input[33];
    bf1[31] = input[31] + input[32];
    bf1[32] = -input[32] + input[31];
    bf1[33] = -input[33] + input[30];
    bf1[34] = -input[34] + input[29];
    bf1[35] = -input[35] + input[28];
    bf1[36] = -input[36] + input[27];
    bf1[37] = -input[37] + input[26];
    bf1[38] = -input[38] + input[25];
    bf1[39] = -input[39] + input[24];
    bf1[40] = -input[40] + input[23];
    bf1[41] = -input[41] + input[22];
    bf1[42] = -input[42] + input[21];
    bf1[43] = -input[43] + input[20];
    bf1[44] = -input[44] + input[19];
    bf1[45] = -input[45] + input[18];
    bf1[46] = -input[46] + input[17];
    bf1[47] = -input[47] + input[16];
    bf1[48] = -input[48] + input[15];
    bf1[49] = -input[49] + input[14];
    bf1[50] = -input[50] + input[13];
    bf1[51] = -input[51] + input[12];
    bf1[52] = -input[52] + input[11];
    bf1[53] = -input[53] + input[10];
    bf1[54] = -input[54] + input[9];
    bf1[55] = -input[55] + input[8];
    bf1[56] = -input[56] + input[7];
    bf1[57] = -input[57] + input[6];
    bf1[58] = -input[58] + input[5];
    bf1[59] = -input[59] + input[4];
    bf1[60] = -input[60] + input[3];
    bf1[61] = -input[61] + input[2];
    bf1[62] = -input[62] + input[1];
    bf1[63] = -input[63] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[31];
    bf1[1]  = bf0[1] + bf0[30];
    bf1[2]  = bf0[2] + bf0[29];
    bf1[3]  = bf0[3] + bf0[28];
    bf1[4]  = bf0[4] + bf0[27];
    bf1[5]  = bf0[5] + bf0[26];
    bf1[6]  = bf0[6] + bf0[25];
    bf1[7]  = bf0[7] + bf0[24];
    bf1[8]  = bf0[8] + bf0[23];
    bf1[9]  = bf0[9] + bf0[22];
    bf1[10] = bf0[10] + bf0[21];
    bf1[11] = bf0[11] + bf0[20];
    bf1[12] = bf0[12] + bf0[19];
    bf1[13] = bf0[13] + bf0[18];
    bf1[14] = bf0[14] + bf0[17];
    bf1[15] = bf0[15] + bf0[16];
    bf1[16] = -bf0[16] + bf0[15];
    bf1[17] = -bf0[17] + bf0[14];
    bf1[18] = -bf0[18] + bf0[13];
    bf1[19] = -bf0[19] + bf0[12];
    bf1[20] = -bf0[20] + bf0[11];
    bf1[21] = -bf0[21] + bf0[10];
    bf1[22] = -bf0[22] + bf0[9];
    bf1[23] = -bf0[23] + bf0[8];
    bf1[24] = -bf0[24] + bf0[7];
    bf1[25] = -bf0[25] + bf0[6];
    bf1[26] = -bf0[26] + bf0[5];
    bf1[27] = -bf0[27] + bf0[4];
    bf1[28] = -bf0[28] + bf0[3];
    bf1[29] = -bf0[29] + bf0[2];
    bf1[30] = -bf0[30] + bf0[1];
    bf1[31] = -bf0[31] + bf0[0];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = bf0[34];
    bf1[35] = bf0[35];
    bf1[36] = bf0[36];
    bf1[37] = bf0[37];
    bf1[38] = bf0[38];
    bf1[39] = bf0[39];
    bf1[40] = half_btf(-cospi[32], bf0[40], cospi[32], bf0[55], cos_bit);
    bf1[41] = half_btf(-cospi[32], bf0[41], cospi[32], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[32], bf0[42], cospi[32], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[32], bf0[43], cospi[32], bf0[52], cos_bit);
    bf1[44] = half_btf(-cospi[32], bf0[44], cospi[32], bf0[51], cos_bit);
    bf1[45] = half_btf(-cospi[32], bf0[45], cospi[32], bf0[50], cos_bit);
    bf1[46] = half_btf(-cospi[32], bf0[46], cospi[32], bf0[49], cos_bit);
    bf1[47] = half_btf(-cospi[32], bf0[47], cospi[32], bf0[48], cos_bit);
    bf1[48] = half_btf(cospi[32], bf0[48], cospi[32], bf0[47], cos_bit);
    bf1[49] = half_btf(cospi[32], bf0[49], cospi[32], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[32], bf0[50], cospi[32], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[32], bf0[51], cospi[32], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[32], bf0[52], cospi[32], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[32], bf0[53], cospi[32], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[32], bf0[54], cospi[32], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[32], bf0[55], cospi[32], bf0[40], cos_bit);
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = bf0[58];
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[15];
    bf1[1]  = bf0[1] + bf0[14];
    bf1[2]  = bf0[2] + bf0[13];
    bf1[3]  = bf0[3] + bf0[12];
    bf1[4]  = bf0[4] + bf0[11];
    bf1[5]  = bf0[5] + bf0[10];
    bf1[6]  = bf0[6] + bf0[9];
    bf1[7]  = bf0[7] + bf0[8];
    bf1[8]  = -bf0[8] + bf0[7];
    bf1[9]  = -bf0[9] + bf0[6];
    bf1[10] = -bf0[10] + bf0[5];
    bf1[11] = -bf0[11] + bf0[4];
    bf1[12] = -bf0[12] + bf0[3];
    bf1[13] = -bf0[13] + bf0[2];
    bf1[14] = -bf0[14] + bf0[1];
    bf1[15] = -bf0[15] + bf0[0];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[24], cospi[32], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[25], cospi[32], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[27], cospi[32], bf0[20], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[47];
    bf1[33] = bf0[33] + bf0[46];
    bf1[34] = bf0[34] + bf0[45];
    bf1[35] = bf0[35] + bf0[44];
    bf1[36] = bf0[36] + bf0[43];
    bf1[37] = bf0[37] + bf0[42];
    bf1[38] = bf0[38] + bf0[41];
    bf1[39] = bf0[39] + bf0[40];
    bf1[40] = -bf0[40] + bf0[39];
    bf1[41] = -bf0[41] + bf0[38];
    bf1[42] = -bf0[42] + bf0[37];
    bf1[43] = -bf0[43] + bf0[36];
    bf1[44] = -bf0[44] + bf0[35];
    bf1[45] = -bf0[45] + bf0[34];
    bf1[46] = -bf0[46] + bf0[33];
    bf1[47] = -bf0[47] + bf0[32];
    bf1[48] = -bf0[48] + bf0[63];
    bf1[49] = -bf0[49] + bf0[62];
    bf1[50] = -bf0[50] + bf0[61];
    bf1[51] = -bf0[51] + bf0[60];
    bf1[52] = -bf0[52] + bf0[59];
    bf1[53] = -bf0[53] + bf0[58];
    bf1[54] = -bf0[54] + bf0[57];
    bf1[55] = -bf0[55] + bf0[56];
    bf1[56] = bf0[56] + bf0[55];
    bf1[57] = bf0[57] + bf0[54];
    bf1[58] = bf0[58] + bf0[53];
    bf1[59] = bf0[59] + bf0[52];
    bf1[60] = bf0[60] + bf0[51];
    bf1[61] = bf0[61] + bf0[50];
    bf1[62] = bf0[62] + bf0[49];
    bf1[63] = bf0[63] + bf0[48];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[23];
    bf1[17] = bf0[17] + bf0[22];
    bf1[18] = bf0[18] + bf0[21];
    bf1[19] = bf0[19] + bf0[20];
    bf1[20] = -bf0[20] + bf0[19];
    bf1[21] = -bf0[21] + bf0[18];
    bf1[22] = -bf0[22] + bf0[17];
    bf1[23] = -bf0[23] + bf0[16];
    bf1[24] = -bf0[24] + bf0[31];
    bf1[25] = -bf0[25] + bf0[30];
    bf1[26] = -bf0[26] + bf0[29];
    bf1[27] = -bf0[27] + bf0[28];
    bf1[28] = bf0[28] + bf0[27];
    bf1[29] = bf0[29] + bf0[26];
    bf1[30] = bf0[30] + bf0[25];
    bf1[31] = bf0[31] + bf0[24];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = bf0[34];
    bf1[35] = bf0[35];
    bf1[36] = half_btf(-cospi[16], bf0[36], cospi[48], bf0[59], cos_bit);
    bf1[37] = half_btf(-cospi[16], bf0[37], cospi[48], bf0[58], cos_bit);
    bf1[38] = half_btf(-cospi[16], bf0[38], cospi[48], bf0[57], cos_bit);
    bf1[39] = half_btf(-cospi[16], bf0[39], cospi[48], bf0[56], cos_bit);
    bf1[40] = half_btf(-cospi[48], bf0[40], -cospi[16], bf0[55], cos_bit);
    bf1[41] = half_btf(-cospi[48], bf0[41], -cospi[16], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[48], bf0[42], -cospi[16], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[48], bf0[43], -cospi[16], bf0[52], cos_bit);
    bf1[44] = bf0[44];
    bf1[45] = bf0[45];
    bf1[46] = bf0[46];
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = bf0[49];
    bf1[50] = bf0[50];
    bf1[51] = bf0[51];
    bf1[52] = half_btf(cospi[48], bf0[52], -cospi[16], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[48], bf0[53], -cospi[16], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[48], bf0[54], -cospi[16], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[48], bf0[55], -cospi[16], bf0[40], cos_bit);
    bf1[56] = half_btf(cospi[16], bf0[56], cospi[48], bf0[39], cos_bit);
    bf1[57] = half_btf(cospi[16], bf0[57], cospi[48], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[16], bf0[58], cospi[48], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[16], bf0[59], cospi[48], bf0[36], cos_bit);
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[2]  = -bf0[2] + bf0[1];
    bf1[3]  = -bf0[3] + bf0[0];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(-cospi[16], bf0[18], cospi[48], bf0[29], cos_bit);
    bf1[19] = half_btf(-cospi[16], bf0[19], cospi[48], bf0[28], cos_bit);
    bf1[20] = half_btf(-cospi[48], bf0[20], -cospi[16], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[48], bf0[21], -cospi[16], bf0[26], cos_bit);
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[48], bf0[26], -cospi[16], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[48], bf0[27], -cospi[16], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[16], bf0[29], cospi[48], bf0[18], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[39];
    bf1[33] = bf0[33] + bf0[38];
    bf1[34] = bf0[34] + bf0[37];
    bf1[35] = bf0[35] + bf0[36];
    bf1[36] = -bf0[36] + bf0[35];
    bf1[37] = -bf0[37] + bf0[34];
    bf1[38] = -bf0[38] + bf0[33];
    bf1[39] = -bf0[39] + bf0[32];
    bf1[40] = -bf0[40] + bf0[47];
    bf1[41] = -bf0[41] + bf0[46];
    bf1[42] = -bf0[42] + bf0[45];
    bf1[43] = -bf0[43] + bf0[44];
    bf1[44] = bf0[44] + bf0[43];
    bf1[45] = bf0[45] + bf0[42];
    bf1[46] = bf0[46] + bf0[41];
    bf1[47] = bf0[47] + bf0[40];
    bf1[48] = bf0[48] + bf0[55];
    bf1[49] = bf0[49] + bf0[54];
    bf1[50] = bf0[50] + bf0[53];
    bf1[51] = bf0[51] + bf0[52];
    bf1[52] = -bf0[52] + bf0[51];
    bf1[53] = -bf0[53] + bf0[50];
    bf1[54] = -bf0[54] + bf0[49];
    bf1[55] = -bf0[55] + bf0[48];
    bf1[56] = -bf0[56] + bf0[63];
    bf1[57] = -bf0[57] + bf0[62];
    bf1[58] = -bf0[58] + bf0[61];
    bf1[59] = -bf0[59] + bf0[60];
    bf1[60] = bf0[60] + bf0[59];
    bf1[61] = bf0[61] + bf0[58];
    bf1[62] = bf0[62] + bf0[57];
    bf1[63] = bf0[63] + bf0[56];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], cospi[16], bf0[3], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[5]  = -bf0[5] + bf0[4];
    bf1[6]  = -bf0[6] + bf0[7];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[19];
    bf1[17] = bf0[17] + bf0[18];
    bf1[18] = -bf0[18] + bf0[17];
    bf1[19] = -bf0[19] + bf0[16];
    bf1[20] = -bf0[20] + bf0[23];
    bf1[21] = -bf0[21] + bf0[22];
    bf1[22] = bf0[22] + bf0[21];
    bf1[23] = bf0[23] + bf0[20];
    bf1[24] = bf0[24] + bf0[27];
    bf1[25] = bf0[25] + bf0[26];
    bf1[26] = -bf0[26] + bf0[25];
    bf1[27] = -bf0[27] + bf0[24];
    bf1[28] = -bf0[28] + bf0[31];
    bf1[29] = -bf0[29] + bf0[30];
    bf1[30] = bf0[30] + bf0[29];
    bf1[31] = bf0[31] + bf0[28];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = half_btf(-cospi[8], bf0[34], cospi[56], bf0[61], cos_bit);
    bf1[35] = half_btf(-cospi[8], bf0[35], cospi[56], bf0[60], cos_bit);
    bf1[36] = half_btf(-cospi[56], bf0[36], -cospi[8], bf0[59], cos_bit);
    bf1[37] = half_btf(-cospi[56], bf0[37], -cospi[8], bf0[58], cos_bit);
    bf1[38] = bf0[38];
    bf1[39] = bf0[39];
    bf1[40] = bf0[40];
    bf1[41] = bf0[41];
    bf1[42] = half_btf(-cospi[40], bf0[42], cospi[24], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[40], bf0[43], cospi[24], bf0[52], cos_bit);
    bf1[44] = half_btf(-cospi[24], bf0[44], -cospi[40], bf0[51], cos_bit);
    bf1[45] = half_btf(-cospi[24], bf0[45], -cospi[40], bf0[50], cos_bit);
    bf1[46] = bf0[46];
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = bf0[49];
    bf1[50] = half_btf(cospi[24], bf0[50], -cospi[40], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[24], bf0[51], -cospi[40], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[40], bf0[52], cospi[24], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[40], bf0[53], cospi[24], bf0[42], cos_bit);
    bf1[54] = bf0[54];
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = half_btf(cospi[56], bf0[58], -cospi[8], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[56], bf0[59], -cospi[8], bf0[36], cos_bit);
    bf1[60] = half_btf(cospi[8], bf0[60], cospi[56], bf0[35], cos_bit);
    bf1[61] = half_btf(cospi[8], bf0[61], cospi[56], bf0[34], cos_bit);
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 7
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[6]  = half_btf(cospi[24], bf0[6], -cospi[40], bf0[5], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[9]  = -bf0[9] + bf0[8];
    bf1[10] = -bf0[10] + bf0[11];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[13] = -bf0[13] + bf0[12];
    bf1[14] = -bf0[14] + bf0[15];
    bf1[15] = bf0[15] + bf0[14];
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(cospi[24], bf0[25], -cospi[40], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[21], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(cospi[56], bf0[29], -cospi[8], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[8], bf0[30], cospi[56], bf0[17], cos_bit);
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[35];
    bf1[33] = bf0[33] + bf0[34];
    bf1[34] = -bf0[34] + bf0[33];
    bf1[35] = -bf0[35] + bf0[32];
    bf1[36] = -bf0[36] + bf0[39];
    bf1[37] = -bf0[37] + bf0[38];
    bf1[38] = bf0[38] + bf0[37];
    bf1[39] = bf0[39] + bf0[36];
    bf1[40] = bf0[40] + bf0[43];
    bf1[41] = bf0[41] + bf0[42];
    bf1[42] = -bf0[42] + bf0[41];
    bf1[43] = -bf0[43] + bf0[40];
    bf1[44] = -bf0[44] + bf0[47];
    bf1[45] = -bf0[45] + bf0[46];
    bf1[46] = bf0[46] + bf0[45];
    bf1[47] = bf0[47] + bf0[44];
    bf1[48] = bf0[48] + bf0[51];
    bf1[49] = bf0[49] + bf0[50];
    bf1[50] = -bf0[50] + bf0[49];
    bf1[51] = -bf0[51] + bf0[48];
    bf1[52] = -bf0[52] + bf0[55];
    bf1[53] = -bf0[53] + bf0[54];
    bf1[54] = bf0[54] + bf0[53];
    bf1[55] = bf0[55] + bf0[52];
    bf1[56] = bf0[56] + bf0[59];
    bf1[57] = bf0[57] + bf0[58];
    bf1[58] = -bf0[58] + bf0[57];
    bf1[59] = -bf0[59] + bf0[56];
    bf1[60] = -bf0[60] + bf0[63];
    bf1[61] = -bf0[61] + bf0[62];
    bf1[62] = bf0[62] + bf0[61];
    bf1[63] = bf0[63] + bf0[60];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = bf0[4];
    bf1[6]  = bf0[6];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], cospi[20], bf0[13], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[14] = half_btf(cospi[28], bf0[14], -cospi[36], bf0[9], cos_bit);
    bf1[16] = bf0[16] + bf0[17];
    bf1[17] = -bf0[17] + bf0[16];
    bf1[18] = -bf0[18] + bf0[19];
    bf1[19] = bf0[19] + bf0[18];
    bf1[20] = bf0[20] + bf0[21];
    bf1[21] = -bf0[21] + bf0[20];
    bf1[22] = -bf0[22] + bf0[23];
    bf1[23] = bf0[23] + bf0[22];
    bf1[24] = bf0[24] + bf0[25];
    bf1[25] = -bf0[25] + bf0[24];
    bf1[26] = -bf0[26] + bf0[27];
    bf1[27] = bf0[27] + bf0[26];
    bf1[28] = bf0[28] + bf0[29];
    bf1[29] = -bf0[29] + bf0[28];
    bf1[30] = -bf0[30] + bf0[31];
    bf1[31] = bf0[31] + bf0[30];
    bf1[32] = bf0[32];
    bf1[33] = half_btf(-cospi[4], bf0[33], cospi[60], bf0[62], cos_bit);
    bf1[34] = half_btf(-cospi[60], bf0[34], -cospi[4], bf0[61], cos_bit);
    bf1[35] = bf0[35];
    bf1[36] = bf0[36];
    bf1[37] = half_btf(-cospi[36], bf0[37], cospi[28], bf0[58], cos_bit);
    bf1[38] = half_btf(-cospi[28], bf0[38], -cospi[36], bf0[57], cos_bit);
    bf1[39] = bf0[39];
    bf1[40] = bf0[40];
    bf1[41] = half_btf(-cospi[20], bf0[41], cospi[44], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[44], bf0[42], -cospi[20], bf0[53], cos_bit);
    bf1[43] = bf0[43];
    bf1[44] = bf0[44];
    bf1[45] = half_btf(-cospi[52], bf0[45], cospi[12], bf0[50], cos_bit);
    bf1[46] = half_btf(-cospi[12], bf0[46], -cospi[52], bf0[49], cos_bit);
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = half_btf(cospi[12], bf0[49], -cospi[52], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[52], bf0[50], cospi[12], bf0[45], cos_bit);
    bf1[51] = bf0[51];
    bf1[52] = bf0[52];
    bf1[53] = half_btf(cospi[44], bf0[53], -cospi[20], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[20], bf0[54], cospi[44], bf0[41], cos_bit);
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = half_btf(cospi[28], bf0[57], -cospi[36], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[36], bf0[58], cospi[28], bf0[37], cos_bit);
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = half_btf(cospi[60], bf0[61], -cospi[4], bf0[34], cos_bit);
    bf1[62] = half_btf(cospi[4], bf0[62], cospi[60], bf0[33], cos_bit);
    bf1[63] = bf0[63];

    // stage 9
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = bf0[4];
    bf1[6]  = bf0[6];
    bf1[8]  = bf0[8];
    bf1[10] = bf0[10];
    bf1[12] = bf0[12];
    bf1[14] = bf0[14];
    bf1[16] = half_btf(cospi[62], bf0[16], cospi[2], bf0[31], cos_bit);
    bf1[18] = half_btf(cospi[46], bf0[18], cospi[18], bf0[29], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], cospi[10], bf0[27], cos_bit);
    bf1[22] = half_btf(cospi[38], bf0[22], cospi[26], bf0[25], cos_bit);
    bf1[24] = half_btf(cospi[6], bf0[24], -cospi[58], bf0[23], cos_bit);
    bf1[26] = half_btf(cospi[22], bf0[26], -cospi[42], bf0[21], cos_bit);
    bf1[28] = half_btf(cospi[14], bf0[28], -cospi[50], bf0[19], cos_bit);
    bf1[30] = half_btf(cospi[30], bf0[30], -cospi[34], bf0[17], cos_bit);
    bf1[32] = bf0[32] + bf0[33];
    bf1[33] = -bf0[33] + bf0[32];
    bf1[34] = -bf0[34] + bf0[35];
    bf1[35] = bf0[35] + bf0[34];
    bf1[36] = bf0[36] + bf0[37];
    bf1[37] = -bf0[37] + bf0[36];
    bf1[38] = -bf0[38] + bf0[39];
    bf1[39] = bf0[39] + bf0[38];
    bf1[40] = bf0[40] + bf0[41];
    bf1[41] = -bf0[41] + bf0[40];
    bf1[42] = -bf0[42] + bf0[43];
    bf1[43] = bf0[43] + bf0[42];
    bf1[44] = bf0[44] + bf0[45];
    bf1[45] = -bf0[45] + bf0[44];
    bf1[46] = -bf0[46] + bf0[47];
    bf1[47] = bf0[47] + bf0[46];
    bf1[48] = bf0[48] + bf0[49];
    bf1[49] = -bf0[49] + bf0[48];
    bf1[50] = -bf0[50] + bf0[51];
    bf1[51] = bf0[51] + bf0[50];
    bf1[52] = bf0[52] + bf0[53];
    bf1[53] = -bf0[53] + bf0[52];
    bf1[54] = -bf0[54] + bf0[55];
    bf1[55] = bf0[55] + bf0[54];
    bf1[56] = bf0[56] + bf0[57];
    bf1[57] = -bf0[57] + bf0[56];
    bf1[58] = -bf0[58] + bf0[59];
    bf1[59] = bf0[59] + bf0[58];
    bf1[60] = bf0[60] + bf0[61];
    bf1[61] = -bf0[61] + bf0[60];
    bf1[62] = -bf0[62] + bf0[63];
    bf1[63] = bf0[63] + bf0[62];

    // stage 10
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[2]  = bf0[2];
    bf1[4]  = bf0[4];
    bf1[6]  = bf0[6];
    bf1[8]  = bf0[8];
    bf1[10] = bf0[10];
    bf1[12] = bf0[12];
    bf1[14] = bf0[14];
    bf1[16] = bf0[16];
    bf1[18] = bf0[18];
    bf1[20] = bf0[20];
    bf1[22] = bf0[22];
    bf1[24] = bf0[24];
    bf1[26] = bf0[26];
    bf1[28] = bf0[28];
    bf1[30] = bf0[30];
    bf1[32] = half_btf(cospi[63], bf0[32], cospi[1], bf0[63], cos_bit);
    bf1[34] = half_btf(cospi[47], bf0[34], cospi[17], bf0[61], cos_bit);
    bf1[36] = half_btf(cospi[55], bf0[36], cospi[9], bf0[59], cos_bit);
    bf1[38] = half_btf(cospi[39], bf0[38], cospi[25], bf0[57], cos_bit);
    bf1[40] = half_btf(cospi[59], bf0[40], cospi[5], bf0[55], cos_bit);
    bf1[42] = half_btf(cospi[43], bf0[42], cospi[21], bf0[53], cos_bit);
    bf1[44] = half_btf(cospi[51], bf0[44], cospi[13], bf0[51], cos_bit);
    bf1[46] = half_btf(cospi[35], bf0[46], cospi[29], bf0[49], cos_bit);
    bf1[48] = half_btf(cospi[3], bf0[48], -cospi[61], bf0[47], cos_bit);
    bf1[50] = half_btf(cospi[19], bf0[50], -cospi[45], bf0[45], cos_bit);
    bf1[52] = half_btf(cospi[11], bf0[52], -cospi[53], bf0[43], cos_bit);
    bf1[54] = half_btf(cospi[27], bf0[54], -cospi[37], bf0[41], cos_bit);
    bf1[56] = half_btf(cospi[7], bf0[56], -cospi[57], bf0[39], cos_bit);
    bf1[58] = half_btf(cospi[23], bf0[58], -cospi[41], bf0[37], cos_bit);
    bf1[60] = half_btf(cospi[15], bf0[60], -cospi[49], bf0[35], cos_bit);
    bf1[62] = half_btf(cospi[31], bf0[62], -cospi[33], bf0[33], cos_bit);

    // stage 11
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[32];
    bf1[2]  = bf0[16];
    bf1[3]  = bf0[48];
    bf1[4]  = bf0[8];
    bf1[5]  = bf0[40];
    bf1[6]  = bf0[24];
    bf1[7]  = bf0[56];
    bf1[8]  = bf0[4];
    bf1[9]  = bf0[36];
    bf1[10] = bf0[20];
    bf1[11] = bf0[52];
    bf1[12] = bf0[12];
    bf1[13] = bf0[44];
    bf1[14] = bf0[28];
    bf1[15] = bf0[60];
    bf1[16] = bf0[2];
    bf1[17] = bf0[34];
    bf1[18] = bf0[18];
    bf1[19] = bf0[50];
    bf1[20] = bf0[10];
    bf1[21] = bf0[42];
    bf1[22] = bf0[26];
    bf1[23] = bf0[58];
    bf1[24] = bf0[6];
    bf1[25] = bf0[38];
    bf1[26] = bf0[22];
    bf1[27] = bf0[54];
    bf1[28] = bf0[14];
    bf1[29] = bf0[46];
    bf1[30] = bf0[30];
    bf1[31] = bf0[62];
}

static void av1_fidentity64_N2_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 32; ++i) {
        output[i] = round_shift((int64_t)input[i] * 4 * new_sqrt2, new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

static INLINE TxfmFunc fwd_txfm_type_to_func_N2(TxfmType txfmtype) {
    switch (txfmtype) {
    case TXFM_TYPE_DCT4:
        return svt_av1_fdct4_new_N2;
    case TXFM_TYPE_DCT8:
        return svt_av1_fdct8_new_N2;
    case TXFM_TYPE_DCT16:
        return svt_av1_fdct16_new_N2;
    case TXFM_TYPE_DCT32:
        return svt_av1_fdct32_new_N2;
    case TXFM_TYPE_DCT64:
        return svt_av1_fdct64_new_N2;
    case TXFM_TYPE_ADST4:
        return svt_av1_fadst4_new_N2;
    case TXFM_TYPE_ADST8:
        return svt_av1_fadst8_new_N2;
    case TXFM_TYPE_ADST16:
        return svt_av1_fadst16_new_N2;
    case TXFM_TYPE_ADST32:
        return av1_fadst32_new;
    case TXFM_TYPE_IDENTITY4:
        return svt_av1_fidentity4_N2_c;
    case TXFM_TYPE_IDENTITY8:
        return svt_av1_fidentity8_N2_c;
    case TXFM_TYPE_IDENTITY16:
        return svt_av1_fidentity16_N2_c;
    case TXFM_TYPE_IDENTITY32:
        return svt_av1_fidentity32_N2_c;
    case TXFM_TYPE_IDENTITY64:
        return av1_fidentity64_N2_c;
    default:
        assert(0);
        return NULL;
    }
}

static INLINE void av1_tranform_two_d_core_N2_c(int16_t* input, uint32_t input_stride, int32_t* output,
                                                const Txfm2dFlipCfg* cfg, int32_t* buf, uint8_t bit_depth) {
    int32_t c, r;
    // Note when assigning txfm_size_col, we use the txfm_size from the
    // row configuration and vice versa. This is intentionally done to
    // accurately perform rectangular transforms. When the transform is
    // rectangular, the number of columns will be the same as the
    // txfm_size stored in the row cfg struct. It will make no difference
    // for square transforms.
    const int32_t txfm_size_col = tx_size_wide[cfg->tx_size];
    const int32_t txfm_size_row = tx_size_high[cfg->tx_size];
    // Take the shift from the larger dimension in the rectangular case.
    const int8_t* shift     = cfg->shift;
    const int32_t rect_type = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);
    int8_t        stage_range_col[MAX_TXFM_STAGE_NUM];
    int8_t        stage_range_row[MAX_TXFM_STAGE_NUM];
    assert(cfg->stage_num_col <= MAX_TXFM_STAGE_NUM);
    assert(cfg->stage_num_row <= MAX_TXFM_STAGE_NUM);
    svt_av1_gen_fwd_stage_range(stage_range_col, stage_range_row, cfg, bit_depth);

    const int8_t   cos_bit_col   = cfg->cos_bit_col;
    const int8_t   cos_bit_row   = cfg->cos_bit_row;
    const TxfmFunc txfm_func_col = fwd_txfm_type_to_func_N2(cfg->txfm_type_col);
    const TxfmFunc txfm_func_row = fwd_txfm_type_to_func_N2(cfg->txfm_type_row);
    ASSERT(txfm_func_col != NULL);
    ASSERT(txfm_func_row != NULL);
    // use output buffer as temp buffer
    int32_t* temp_in  = output;
    int32_t* temp_out = output + txfm_size_row;

    // Columns
    for (c = 0; c < txfm_size_col; ++c) {
        if (cfg->ud_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                temp_in[r] = input[r * input_stride + c];
            }
        } else {
            for (r = 0; r < txfm_size_row; ++r) {
                // flip upside down
                temp_in[r] = input[(txfm_size_row - r - 1) * input_stride + c];
            }
        }
        svt_av1_round_shift_array_c(temp_in, txfm_size_row, -shift[0]); // NM svt_av1_round_shift_array_c
        txfm_func_col(temp_in, temp_out, cos_bit_col, stage_range_col);
        svt_av1_round_shift_array_c(temp_out, txfm_size_row / 2, -shift[1]); // NM svt_av1_round_shift_array_c
        if (cfg->lr_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                buf[r * txfm_size_col + c] = temp_out[r];
            }
        } else {
            for (r = 0; r < txfm_size_row; ++r) {
                // flip from left to right
                buf[r * txfm_size_col + (txfm_size_col - c - 1)] = temp_out[r];
            }
        }
    }

    // Rows
    for (r = 0; r < txfm_size_row / 2; ++r) {
        txfm_func_row(buf + r * txfm_size_col, output + r * txfm_size_col, cos_bit_row, stage_range_row);
        svt_av1_round_shift_array_c(output + r * txfm_size_col, txfm_size_col / 2, -shift[2]);

        if (abs(rect_type) == 1) {
            // Multiply everything by Sqrt2 if the transform is rectangular and the
            // size difference is a factor of 2.
            for (c = 0; c < txfm_size_col / 2; ++c) {
                output[r * txfm_size_col + c] = round_shift((int64_t)output[r * txfm_size_col + c] * new_sqrt2,
                                                            new_sqrt2_bits);
            }
        }
    }

    for (int i = 0; i < (txfm_size_col * txfm_size_row); i++) {
        if (i % txfm_size_col >= (txfm_size_col >> 1) || i / txfm_size_col >= (txfm_size_row >> 1)) {
            output[i] = 0;
        }
    }
}

void svt_aom_transform_two_d_64x64_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                        uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 64];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_64X64, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_32x32_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                        uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X32, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_16x16_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                        uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X16, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_8x8_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                      uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X8, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_4x4_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                      uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 4];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_4X4, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_64x32_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_64X32, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x64_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 64];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X64, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_64x16_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_64X16, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x64_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 64];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X64, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x16_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X16, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x32_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X32, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x8_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X8, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x16_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X16, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x8_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X8, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x32_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X32, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x4_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 4];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X4, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_4x16_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_4X16, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x4_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                 uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 4];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X4, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_4x8_N2_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                 uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_4X8, &cfg);
    av1_tranform_two_d_core_N2_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fdct4_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi = cospi_arr(cos_bit);
    int32_t        step[2];

    // stage 1;
    step[0] = input[0] + input[3];
    step[1] = input[1] + input[2];

    output[0] = half_btf(cospi[32], step[0], cospi[32], step[1], cos_bit);
}

void svt_av1_fadst4_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    int32_t        bit   = cos_bit;
    const int32_t* sinpi = sinpi_arr(bit);
    int32_t        x0, x1, x2, x3;
    int32_t        s0, s2, s4, s5;

    // stage 0
    x0 = input[0];
    x1 = input[1];
    x2 = input[2];
    x3 = input[3];

    if (!(x0 | x1 | x2 | x3)) {
        output[0] = output[1] = output[2] = output[3] = 0;
        return;
    }

    // stage 1
    s0 = sinpi[1] * x0;
    s2 = sinpi[2] * x1;
    s4 = sinpi[3] * x2;
    s5 = sinpi[4] * x3;

    // stage 3
    x0 = s0 + s2;

    // stage 4
    x0 = x0 + s5;

    // stage 5
    s0 = x0 + s4;

    // 1-D transform scaling factor is sqrt(2).
    output[0] = round_shift(s0, bit);
}

void svt_av1_fidentity4_N4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    output[0] = round_shift((int64_t)input[0] * new_sqrt2, new_sqrt2_bits);
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_fdct8_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    bf1    = output;
    bf1[0] = input[0] + input[7];
    bf1[1] = input[1] + input[6];
    bf1[2] = input[2] + input[5];
    bf1[3] = input[3] + input[4];
    bf1[4] = -input[4] + input[3];
    bf1[5] = -input[5] + input[2];
    bf1[6] = -input[6] + input[1];
    bf1[7] = -input[7] + input[0];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0] + bf0[3];
    bf1[1] = bf0[1] + bf0[2];
    bf1[4] = bf0[4];
    bf1[5] = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7] = bf0[7];

    // stage 3
    bf0    = step;
    bf1    = output;
    bf1[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[4] = bf0[4] + bf0[5];
    bf1[7] = bf0[7] + bf0[6];

    // stage 4
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[4] = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);

    // stage 5
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[4];
}

void svt_av1_fadst8_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    assert(output != input);
    bf1    = output;
    bf1[0] = input[0];
    bf1[1] = -input[7];
    bf1[2] = -input[3];
    bf1[3] = input[4];
    bf1[4] = -input[1];
    bf1[5] = input[6];
    bf1[6] = input[2];
    bf1[7] = -input[5];

    // stage 2
    cospi  = cospi_arr(cos_bit);
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[32], bf0[2], -cospi[32], bf0[3], cos_bit);
    bf1[4] = bf0[4];
    bf1[5] = bf0[5];
    bf1[6] = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[32], bf0[6], -cospi[32], bf0[7], cos_bit);

    // stage 3
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0] + bf0[2];
    bf1[1] = bf0[1] + bf0[3];
    bf1[2] = bf0[0] - bf0[2];
    bf1[3] = bf0[1] - bf0[3];
    bf1[4] = bf0[4] + bf0[6];
    bf1[5] = bf0[5] + bf0[7];
    bf1[6] = bf0[4] - bf0[6];
    bf1[7] = bf0[5] - bf0[7];

    // stage 4
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = bf0[2];
    bf1[3] = bf0[3];
    bf1[4] = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5] = half_btf(cospi[48], bf0[4], -cospi[16], bf0[5], cos_bit);
    bf1[6] = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7] = half_btf(cospi[16], bf0[6], cospi[48], bf0[7], cos_bit);

    // stage 5
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0] + bf0[4];
    bf1[1] = bf0[1] + bf0[5];
    bf1[6] = bf0[2] - bf0[6];
    bf1[7] = bf0[3] - bf0[7];

    // stage 6
    bf0    = output;
    bf1    = step;
    bf1[1] = half_btf(cospi[60], bf0[0], -cospi[4], bf0[1], cos_bit);
    bf1[6] = half_btf(cospi[52], bf0[6], cospi[12], bf0[7], cos_bit);

    // stage 7
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[1];
    bf1[1] = bf0[6];
}

void svt_av1_fidentity8_N4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 2; ++i) {
        output[i] = input[i] * 2;
    }
}

void svt_av1_fdct16_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[15];
    bf1[1]  = input[1] + input[14];
    bf1[2]  = input[2] + input[13];
    bf1[3]  = input[3] + input[12];
    bf1[4]  = input[4] + input[11];
    bf1[5]  = input[5] + input[10];
    bf1[6]  = input[6] + input[9];
    bf1[7]  = input[7] + input[8];
    bf1[8]  = -input[8] + input[7];
    bf1[9]  = -input[9] + input[6];
    bf1[10] = -input[10] + input[5];
    bf1[11] = -input[11] + input[4];
    bf1[12] = -input[12] + input[3];
    bf1[13] = -input[13] + input[2];
    bf1[14] = -input[14] + input[1];
    bf1[15] = -input[15] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];

    // stage 3
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];

    // stage 4
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];

    // stage 5
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[15] = bf0[15] + bf0[14];

    // stage 6
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[4]  = bf0[4];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);

    // stage 7
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[8];
    bf1[2] = bf0[4];
    bf1[3] = bf0[12];
}

void svt_av1_fadst16_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    assert(output != input);
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = -input[15];
    bf1[2]  = -input[7];
    bf1[3]  = input[8];
    bf1[4]  = -input[3];
    bf1[5]  = input[12];
    bf1[6]  = input[4];
    bf1[7]  = -input[11];
    bf1[8]  = -input[1];
    bf1[9]  = input[14];
    bf1[10] = input[6];
    bf1[11] = -input[9];
    bf1[12] = input[2];
    bf1[13] = -input[13];
    bf1[14] = -input[5];
    bf1[15] = input[10];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = half_btf(cospi[32], bf0[2], cospi[32], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[32], bf0[2], -cospi[32], bf0[3], cos_bit);
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[32], bf0[6], -cospi[32], bf0[7], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(cospi[32], bf0[10], cospi[32], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[32], bf0[10], -cospi[32], bf0[11], cos_bit);
    bf1[12] = bf0[12];
    bf1[13] = bf0[13];
    bf1[14] = half_btf(cospi[32], bf0[14], cospi[32], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[32], bf0[14], -cospi[32], bf0[15], cos_bit);

    // stage 3
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[2];
    bf1[1]  = bf0[1] + bf0[3];
    bf1[2]  = bf0[0] - bf0[2];
    bf1[3]  = bf0[1] - bf0[3];
    bf1[4]  = bf0[4] + bf0[6];
    bf1[5]  = bf0[5] + bf0[7];
    bf1[6]  = bf0[4] - bf0[6];
    bf1[7]  = bf0[5] - bf0[7];
    bf1[8]  = bf0[8] + bf0[10];
    bf1[9]  = bf0[9] + bf0[11];
    bf1[10] = bf0[8] - bf0[10];
    bf1[11] = bf0[9] - bf0[11];
    bf1[12] = bf0[12] + bf0[14];
    bf1[13] = bf0[13] + bf0[15];
    bf1[14] = bf0[12] - bf0[14];
    bf1[15] = bf0[13] - bf0[15];

    // stage 4
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[16], bf0[4], cospi[48], bf0[5], cos_bit);
    bf1[5]  = half_btf(cospi[48], bf0[4], -cospi[16], bf0[5], cos_bit);
    bf1[6]  = half_btf(-cospi[48], bf0[6], cospi[16], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[16], bf0[6], cospi[48], bf0[7], cos_bit);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = bf0[10];
    bf1[11] = bf0[11];
    bf1[12] = half_btf(cospi[16], bf0[12], cospi[48], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[48], bf0[12], -cospi[16], bf0[13], cos_bit);
    bf1[14] = half_btf(-cospi[48], bf0[14], cospi[16], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[16], bf0[14], cospi[48], bf0[15], cos_bit);

    // stage 5
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[4];
    bf1[1]  = bf0[1] + bf0[5];
    bf1[2]  = bf0[2] + bf0[6];
    bf1[3]  = bf0[3] + bf0[7];
    bf1[4]  = bf0[0] - bf0[4];
    bf1[5]  = bf0[1] - bf0[5];
    bf1[6]  = bf0[2] - bf0[6];
    bf1[7]  = bf0[3] - bf0[7];
    bf1[8]  = bf0[8] + bf0[12];
    bf1[9]  = bf0[9] + bf0[13];
    bf1[10] = bf0[10] + bf0[14];
    bf1[11] = bf0[11] + bf0[15];
    bf1[12] = bf0[8] - bf0[12];
    bf1[13] = bf0[9] - bf0[13];
    bf1[14] = bf0[10] - bf0[14];
    bf1[15] = bf0[11] - bf0[15];

    // stage 6
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = bf0[4];
    bf1[5]  = bf0[5];
    bf1[6]  = bf0[6];
    bf1[7]  = bf0[7];
    bf1[8]  = half_btf(cospi[8], bf0[8], cospi[56], bf0[9], cos_bit);
    bf1[9]  = half_btf(cospi[56], bf0[8], -cospi[8], bf0[9], cos_bit);
    bf1[10] = half_btf(cospi[40], bf0[10], cospi[24], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[24], bf0[10], -cospi[40], bf0[11], cos_bit);
    bf1[12] = half_btf(-cospi[56], bf0[12], cospi[8], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[8], bf0[12], cospi[56], bf0[13], cos_bit);
    bf1[14] = half_btf(-cospi[24], bf0[14], cospi[40], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[40], bf0[14], cospi[24], bf0[15], cos_bit);

    // stage 7
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[8];
    bf1[1]  = bf0[1] + bf0[9];
    bf1[2]  = bf0[2] + bf0[10];
    bf1[3]  = bf0[3] + bf0[11];
    bf1[12] = bf0[4] - bf0[12];
    bf1[13] = bf0[5] - bf0[13];
    bf1[14] = bf0[6] - bf0[14];
    bf1[15] = bf0[7] - bf0[15];

    // stage 8
    bf0     = output;
    bf1     = step;
    bf1[1]  = half_btf(cospi[62], bf0[0], -cospi[2], bf0[1], cos_bit);
    bf1[3]  = half_btf(cospi[54], bf0[2], -cospi[10], bf0[3], cos_bit);
    bf1[12] = half_btf(cospi[50], bf0[12], cospi[14], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[58], bf0[14], cospi[6], bf0[15], cos_bit);

    // stage 9
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[1];
    bf1[1] = bf0[14];
    bf1[2] = bf0[3];
    bf1[3] = bf0[12];
}

void svt_av1_fidentity16_N4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 4; ++i) {
        output[i] = round_shift((int64_t)input[i] * 2 * new_sqrt2, new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_fdct32_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[32];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[31];
    bf1[1]  = input[1] + input[30];
    bf1[2]  = input[2] + input[29];
    bf1[3]  = input[3] + input[28];
    bf1[4]  = input[4] + input[27];
    bf1[5]  = input[5] + input[26];
    bf1[6]  = input[6] + input[25];
    bf1[7]  = input[7] + input[24];
    bf1[8]  = input[8] + input[23];
    bf1[9]  = input[9] + input[22];
    bf1[10] = input[10] + input[21];
    bf1[11] = input[11] + input[20];
    bf1[12] = input[12] + input[19];
    bf1[13] = input[13] + input[18];
    bf1[14] = input[14] + input[17];
    bf1[15] = input[15] + input[16];
    bf1[16] = -input[16] + input[15];
    bf1[17] = -input[17] + input[14];
    bf1[18] = -input[18] + input[13];
    bf1[19] = -input[19] + input[12];
    bf1[20] = -input[20] + input[11];
    bf1[21] = -input[21] + input[10];
    bf1[22] = -input[22] + input[9];
    bf1[23] = -input[23] + input[8];
    bf1[24] = -input[24] + input[7];
    bf1[25] = -input[25] + input[6];
    bf1[26] = -input[26] + input[5];
    bf1[27] = -input[27] + input[4];
    bf1[28] = -input[28] + input[3];
    bf1[29] = -input[29] + input[2];
    bf1[30] = -input[30] + input[1];
    bf1[31] = -input[31] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[15];
    bf1[1]  = bf0[1] + bf0[14];
    bf1[2]  = bf0[2] + bf0[13];
    bf1[3]  = bf0[3] + bf0[12];
    bf1[4]  = bf0[4] + bf0[11];
    bf1[5]  = bf0[5] + bf0[10];
    bf1[6]  = bf0[6] + bf0[9];
    bf1[7]  = bf0[7] + bf0[8];
    bf1[8]  = -bf0[8] + bf0[7];
    bf1[9]  = -bf0[9] + bf0[6];
    bf1[10] = -bf0[10] + bf0[5];
    bf1[11] = -bf0[11] + bf0[4];
    bf1[12] = -bf0[12] + bf0[3];
    bf1[13] = -bf0[13] + bf0[2];
    bf1[14] = -bf0[14] + bf0[1];
    bf1[15] = -bf0[15] + bf0[0];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[24], cospi[32], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[25], cospi[32], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[27], cospi[32], bf0[20], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];

    // stage 3
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[23];
    bf1[17] = bf0[17] + bf0[22];
    bf1[18] = bf0[18] + bf0[21];
    bf1[19] = bf0[19] + bf0[20];
    bf1[20] = -bf0[20] + bf0[19];
    bf1[21] = -bf0[21] + bf0[18];
    bf1[22] = -bf0[22] + bf0[17];
    bf1[23] = -bf0[23] + bf0[16];
    bf1[24] = -bf0[24] + bf0[31];
    bf1[25] = -bf0[25] + bf0[30];
    bf1[26] = -bf0[26] + bf0[29];
    bf1[27] = -bf0[27] + bf0[28];
    bf1[28] = bf0[28] + bf0[27];
    bf1[29] = bf0[29] + bf0[26];
    bf1[30] = bf0[30] + bf0[25];
    bf1[31] = bf0[31] + bf0[24];

    // stage 4
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(-cospi[16], bf0[18], cospi[48], bf0[29], cos_bit);
    bf1[19] = half_btf(-cospi[16], bf0[19], cospi[48], bf0[28], cos_bit);
    bf1[20] = half_btf(-cospi[48], bf0[20], -cospi[16], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[48], bf0[21], -cospi[16], bf0[26], cos_bit);
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[48], bf0[26], -cospi[16], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[48], bf0[27], -cospi[16], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[16], bf0[29], cospi[48], bf0[18], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];

    // stage 5
    bf0     = step;
    bf1     = output;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[19];
    bf1[17] = bf0[17] + bf0[18];
    bf1[18] = -bf0[18] + bf0[17];
    bf1[19] = -bf0[19] + bf0[16];
    bf1[20] = -bf0[20] + bf0[23];
    bf1[21] = -bf0[21] + bf0[22];
    bf1[22] = bf0[22] + bf0[21];
    bf1[23] = bf0[23] + bf0[20];
    bf1[24] = bf0[24] + bf0[27];
    bf1[25] = bf0[25] + bf0[26];
    bf1[26] = -bf0[26] + bf0[25];
    bf1[27] = -bf0[27] + bf0[24];
    bf1[28] = -bf0[28] + bf0[31];
    bf1[29] = -bf0[29] + bf0[30];
    bf1[30] = bf0[30] + bf0[29];
    bf1[31] = bf0[31] + bf0[28];

    // stage 6
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[15] = bf0[15] + bf0[14];
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(cospi[24], bf0[25], -cospi[40], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[21], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(cospi[56], bf0[29], -cospi[8], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[8], bf0[30], cospi[56], bf0[17], cos_bit);
    bf1[31] = bf0[31];

    // stage 7
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[4]  = bf0[4];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[16] = bf0[16] + bf0[17];
    bf1[19] = bf0[19] + bf0[18];
    bf1[20] = bf0[20] + bf0[21];
    bf1[23] = bf0[23] + bf0[22];
    bf1[24] = bf0[24] + bf0[25];
    bf1[27] = bf0[27] + bf0[26];
    bf1[28] = bf0[28] + bf0[29];
    bf1[31] = bf0[31] + bf0[30];

    // stage 8
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[4]  = bf0[4];
    bf1[8]  = bf0[8];
    bf1[12] = bf0[12];
    bf1[16] = half_btf(cospi[62], bf0[16], cospi[2], bf0[31], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], cospi[10], bf0[27], cos_bit);
    bf1[24] = half_btf(cospi[6], bf0[24], -cospi[58], bf0[23], cos_bit);
    bf1[28] = half_btf(cospi[14], bf0[28], -cospi[50], bf0[19], cos_bit);

    // stage 9
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = bf0[16];
    bf1[2] = bf0[8];
    bf1[3] = bf0[24];
    bf1[4] = bf0[4];
    bf1[5] = bf0[20];
    bf1[6] = bf0[12];
    bf1[7] = bf0[28];
}

void svt_av1_fidentity32_N4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 8; ++i) {
        output[i] = input[i] * 4;
    }
}

void svt_av1_fdct64_new_N4(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    const int32_t* cospi;

    int32_t *bf0, *bf1;
    int32_t  step[64];

    // stage 0;

    // stage 1;
    bf1     = output;
    bf1[0]  = input[0] + input[63];
    bf1[1]  = input[1] + input[62];
    bf1[2]  = input[2] + input[61];
    bf1[3]  = input[3] + input[60];
    bf1[4]  = input[4] + input[59];
    bf1[5]  = input[5] + input[58];
    bf1[6]  = input[6] + input[57];
    bf1[7]  = input[7] + input[56];
    bf1[8]  = input[8] + input[55];
    bf1[9]  = input[9] + input[54];
    bf1[10] = input[10] + input[53];
    bf1[11] = input[11] + input[52];
    bf1[12] = input[12] + input[51];
    bf1[13] = input[13] + input[50];
    bf1[14] = input[14] + input[49];
    bf1[15] = input[15] + input[48];
    bf1[16] = input[16] + input[47];
    bf1[17] = input[17] + input[46];
    bf1[18] = input[18] + input[45];
    bf1[19] = input[19] + input[44];
    bf1[20] = input[20] + input[43];
    bf1[21] = input[21] + input[42];
    bf1[22] = input[22] + input[41];
    bf1[23] = input[23] + input[40];
    bf1[24] = input[24] + input[39];
    bf1[25] = input[25] + input[38];
    bf1[26] = input[26] + input[37];
    bf1[27] = input[27] + input[36];
    bf1[28] = input[28] + input[35];
    bf1[29] = input[29] + input[34];
    bf1[30] = input[30] + input[33];
    bf1[31] = input[31] + input[32];
    bf1[32] = -input[32] + input[31];
    bf1[33] = -input[33] + input[30];
    bf1[34] = -input[34] + input[29];
    bf1[35] = -input[35] + input[28];
    bf1[36] = -input[36] + input[27];
    bf1[37] = -input[37] + input[26];
    bf1[38] = -input[38] + input[25];
    bf1[39] = -input[39] + input[24];
    bf1[40] = -input[40] + input[23];
    bf1[41] = -input[41] + input[22];
    bf1[42] = -input[42] + input[21];
    bf1[43] = -input[43] + input[20];
    bf1[44] = -input[44] + input[19];
    bf1[45] = -input[45] + input[18];
    bf1[46] = -input[46] + input[17];
    bf1[47] = -input[47] + input[16];
    bf1[48] = -input[48] + input[15];
    bf1[49] = -input[49] + input[14];
    bf1[50] = -input[50] + input[13];
    bf1[51] = -input[51] + input[12];
    bf1[52] = -input[52] + input[11];
    bf1[53] = -input[53] + input[10];
    bf1[54] = -input[54] + input[9];
    bf1[55] = -input[55] + input[8];
    bf1[56] = -input[56] + input[7];
    bf1[57] = -input[57] + input[6];
    bf1[58] = -input[58] + input[5];
    bf1[59] = -input[59] + input[4];
    bf1[60] = -input[60] + input[3];
    bf1[61] = -input[61] + input[2];
    bf1[62] = -input[62] + input[1];
    bf1[63] = -input[63] + input[0];

    // stage 2
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[31];
    bf1[1]  = bf0[1] + bf0[30];
    bf1[2]  = bf0[2] + bf0[29];
    bf1[3]  = bf0[3] + bf0[28];
    bf1[4]  = bf0[4] + bf0[27];
    bf1[5]  = bf0[5] + bf0[26];
    bf1[6]  = bf0[6] + bf0[25];
    bf1[7]  = bf0[7] + bf0[24];
    bf1[8]  = bf0[8] + bf0[23];
    bf1[9]  = bf0[9] + bf0[22];
    bf1[10] = bf0[10] + bf0[21];
    bf1[11] = bf0[11] + bf0[20];
    bf1[12] = bf0[12] + bf0[19];
    bf1[13] = bf0[13] + bf0[18];
    bf1[14] = bf0[14] + bf0[17];
    bf1[15] = bf0[15] + bf0[16];
    bf1[16] = -bf0[16] + bf0[15];
    bf1[17] = -bf0[17] + bf0[14];
    bf1[18] = -bf0[18] + bf0[13];
    bf1[19] = -bf0[19] + bf0[12];
    bf1[20] = -bf0[20] + bf0[11];
    bf1[21] = -bf0[21] + bf0[10];
    bf1[22] = -bf0[22] + bf0[9];
    bf1[23] = -bf0[23] + bf0[8];
    bf1[24] = -bf0[24] + bf0[7];
    bf1[25] = -bf0[25] + bf0[6];
    bf1[26] = -bf0[26] + bf0[5];
    bf1[27] = -bf0[27] + bf0[4];
    bf1[28] = -bf0[28] + bf0[3];
    bf1[29] = -bf0[29] + bf0[2];
    bf1[30] = -bf0[30] + bf0[1];
    bf1[31] = -bf0[31] + bf0[0];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = bf0[34];
    bf1[35] = bf0[35];
    bf1[36] = bf0[36];
    bf1[37] = bf0[37];
    bf1[38] = bf0[38];
    bf1[39] = bf0[39];
    bf1[40] = half_btf(-cospi[32], bf0[40], cospi[32], bf0[55], cos_bit);
    bf1[41] = half_btf(-cospi[32], bf0[41], cospi[32], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[32], bf0[42], cospi[32], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[32], bf0[43], cospi[32], bf0[52], cos_bit);
    bf1[44] = half_btf(-cospi[32], bf0[44], cospi[32], bf0[51], cos_bit);
    bf1[45] = half_btf(-cospi[32], bf0[45], cospi[32], bf0[50], cos_bit);
    bf1[46] = half_btf(-cospi[32], bf0[46], cospi[32], bf0[49], cos_bit);
    bf1[47] = half_btf(-cospi[32], bf0[47], cospi[32], bf0[48], cos_bit);
    bf1[48] = half_btf(cospi[32], bf0[48], cospi[32], bf0[47], cos_bit);
    bf1[49] = half_btf(cospi[32], bf0[49], cospi[32], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[32], bf0[50], cospi[32], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[32], bf0[51], cospi[32], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[32], bf0[52], cospi[32], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[32], bf0[53], cospi[32], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[32], bf0[54], cospi[32], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[32], bf0[55], cospi[32], bf0[40], cos_bit);
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = bf0[58];
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 3
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[15];
    bf1[1]  = bf0[1] + bf0[14];
    bf1[2]  = bf0[2] + bf0[13];
    bf1[3]  = bf0[3] + bf0[12];
    bf1[4]  = bf0[4] + bf0[11];
    bf1[5]  = bf0[5] + bf0[10];
    bf1[6]  = bf0[6] + bf0[9];
    bf1[7]  = bf0[7] + bf0[8];
    bf1[8]  = -bf0[8] + bf0[7];
    bf1[9]  = -bf0[9] + bf0[6];
    bf1[10] = -bf0[10] + bf0[5];
    bf1[11] = -bf0[11] + bf0[4];
    bf1[12] = -bf0[12] + bf0[3];
    bf1[13] = -bf0[13] + bf0[2];
    bf1[14] = -bf0[14] + bf0[1];
    bf1[15] = -bf0[15] + bf0[0];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[24], cospi[32], bf0[23], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[25], cospi[32], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[27], cospi[32], bf0[20], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[47];
    bf1[33] = bf0[33] + bf0[46];
    bf1[34] = bf0[34] + bf0[45];
    bf1[35] = bf0[35] + bf0[44];
    bf1[36] = bf0[36] + bf0[43];
    bf1[37] = bf0[37] + bf0[42];
    bf1[38] = bf0[38] + bf0[41];
    bf1[39] = bf0[39] + bf0[40];
    bf1[40] = -bf0[40] + bf0[39];
    bf1[41] = -bf0[41] + bf0[38];
    bf1[42] = -bf0[42] + bf0[37];
    bf1[43] = -bf0[43] + bf0[36];
    bf1[44] = -bf0[44] + bf0[35];
    bf1[45] = -bf0[45] + bf0[34];
    bf1[46] = -bf0[46] + bf0[33];
    bf1[47] = -bf0[47] + bf0[32];
    bf1[48] = -bf0[48] + bf0[63];
    bf1[49] = -bf0[49] + bf0[62];
    bf1[50] = -bf0[50] + bf0[61];
    bf1[51] = -bf0[51] + bf0[60];
    bf1[52] = -bf0[52] + bf0[59];
    bf1[53] = -bf0[53] + bf0[58];
    bf1[54] = -bf0[54] + bf0[57];
    bf1[55] = -bf0[55] + bf0[56];
    bf1[56] = bf0[56] + bf0[55];
    bf1[57] = bf0[57] + bf0[54];
    bf1[58] = bf0[58] + bf0[53];
    bf1[59] = bf0[59] + bf0[52];
    bf1[60] = bf0[60] + bf0[51];
    bf1[61] = bf0[61] + bf0[50];
    bf1[62] = bf0[62] + bf0[49];
    bf1[63] = bf0[63] + bf0[48];

    // stage 4
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0] + bf0[7];
    bf1[1]  = bf0[1] + bf0[6];
    bf1[2]  = bf0[2] + bf0[5];
    bf1[3]  = bf0[3] + bf0[4];
    bf1[4]  = -bf0[4] + bf0[3];
    bf1[5]  = -bf0[5] + bf0[2];
    bf1[6]  = -bf0[6] + bf0[1];
    bf1[7]  = -bf0[7] + bf0[0];
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[12], cospi[32], bf0[11], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[13], cospi[32], bf0[10], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[23];
    bf1[17] = bf0[17] + bf0[22];
    bf1[18] = bf0[18] + bf0[21];
    bf1[19] = bf0[19] + bf0[20];
    bf1[20] = -bf0[20] + bf0[19];
    bf1[21] = -bf0[21] + bf0[18];
    bf1[22] = -bf0[22] + bf0[17];
    bf1[23] = -bf0[23] + bf0[16];
    bf1[24] = -bf0[24] + bf0[31];
    bf1[25] = -bf0[25] + bf0[30];
    bf1[26] = -bf0[26] + bf0[29];
    bf1[27] = -bf0[27] + bf0[28];
    bf1[28] = bf0[28] + bf0[27];
    bf1[29] = bf0[29] + bf0[26];
    bf1[30] = bf0[30] + bf0[25];
    bf1[31] = bf0[31] + bf0[24];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = bf0[34];
    bf1[35] = bf0[35];
    bf1[36] = half_btf(-cospi[16], bf0[36], cospi[48], bf0[59], cos_bit);
    bf1[37] = half_btf(-cospi[16], bf0[37], cospi[48], bf0[58], cos_bit);
    bf1[38] = half_btf(-cospi[16], bf0[38], cospi[48], bf0[57], cos_bit);
    bf1[39] = half_btf(-cospi[16], bf0[39], cospi[48], bf0[56], cos_bit);
    bf1[40] = half_btf(-cospi[48], bf0[40], -cospi[16], bf0[55], cos_bit);
    bf1[41] = half_btf(-cospi[48], bf0[41], -cospi[16], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[48], bf0[42], -cospi[16], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[48], bf0[43], -cospi[16], bf0[52], cos_bit);
    bf1[44] = bf0[44];
    bf1[45] = bf0[45];
    bf1[46] = bf0[46];
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = bf0[49];
    bf1[50] = bf0[50];
    bf1[51] = bf0[51];
    bf1[52] = half_btf(cospi[48], bf0[52], -cospi[16], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[48], bf0[53], -cospi[16], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[48], bf0[54], -cospi[16], bf0[41], cos_bit);
    bf1[55] = half_btf(cospi[48], bf0[55], -cospi[16], bf0[40], cos_bit);
    bf1[56] = half_btf(cospi[16], bf0[56], cospi[48], bf0[39], cos_bit);
    bf1[57] = half_btf(cospi[16], bf0[57], cospi[48], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[16], bf0[58], cospi[48], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[16], bf0[59], cospi[48], bf0[36], cos_bit);
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 5
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0] + bf0[3];
    bf1[1]  = bf0[1] + bf0[2];
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[6], cospi[32], bf0[5], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = bf0[8] + bf0[11];
    bf1[9]  = bf0[9] + bf0[10];
    bf1[10] = -bf0[10] + bf0[9];
    bf1[11] = -bf0[11] + bf0[8];
    bf1[12] = -bf0[12] + bf0[15];
    bf1[13] = -bf0[13] + bf0[14];
    bf1[14] = bf0[14] + bf0[13];
    bf1[15] = bf0[15] + bf0[12];
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(-cospi[16], bf0[18], cospi[48], bf0[29], cos_bit);
    bf1[19] = half_btf(-cospi[16], bf0[19], cospi[48], bf0[28], cos_bit);
    bf1[20] = half_btf(-cospi[48], bf0[20], -cospi[16], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[48], bf0[21], -cospi[16], bf0[26], cos_bit);
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[48], bf0[26], -cospi[16], bf0[21], cos_bit);
    bf1[27] = half_btf(cospi[48], bf0[27], -cospi[16], bf0[20], cos_bit);
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[19], cos_bit);
    bf1[29] = half_btf(cospi[16], bf0[29], cospi[48], bf0[18], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[39];
    bf1[33] = bf0[33] + bf0[38];
    bf1[34] = bf0[34] + bf0[37];
    bf1[35] = bf0[35] + bf0[36];
    bf1[36] = -bf0[36] + bf0[35];
    bf1[37] = -bf0[37] + bf0[34];
    bf1[38] = -bf0[38] + bf0[33];
    bf1[39] = -bf0[39] + bf0[32];
    bf1[40] = -bf0[40] + bf0[47];
    bf1[41] = -bf0[41] + bf0[46];
    bf1[42] = -bf0[42] + bf0[45];
    bf1[43] = -bf0[43] + bf0[44];
    bf1[44] = bf0[44] + bf0[43];
    bf1[45] = bf0[45] + bf0[42];
    bf1[46] = bf0[46] + bf0[41];
    bf1[47] = bf0[47] + bf0[40];
    bf1[48] = bf0[48] + bf0[55];
    bf1[49] = bf0[49] + bf0[54];
    bf1[50] = bf0[50] + bf0[53];
    bf1[51] = bf0[51] + bf0[52];
    bf1[52] = -bf0[52] + bf0[51];
    bf1[53] = -bf0[53] + bf0[50];
    bf1[54] = -bf0[54] + bf0[49];
    bf1[55] = -bf0[55] + bf0[48];
    bf1[56] = -bf0[56] + bf0[63];
    bf1[57] = -bf0[57] + bf0[62];
    bf1[58] = -bf0[58] + bf0[61];
    bf1[59] = -bf0[59] + bf0[60];
    bf1[60] = bf0[60] + bf0[59];
    bf1[61] = bf0[61] + bf0[58];
    bf1[62] = bf0[62] + bf0[57];
    bf1[63] = bf0[63] + bf0[56];

    // stage 6
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[4]  = bf0[4] + bf0[5];
    bf1[7]  = bf0[7] + bf0[6];
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(cospi[48], bf0[13], -cospi[16], bf0[10], cos_bit);
    bf1[14] = half_btf(cospi[16], bf0[14], cospi[48], bf0[9], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = bf0[16] + bf0[19];
    bf1[17] = bf0[17] + bf0[18];
    bf1[18] = -bf0[18] + bf0[17];
    bf1[19] = -bf0[19] + bf0[16];
    bf1[20] = -bf0[20] + bf0[23];
    bf1[21] = -bf0[21] + bf0[22];
    bf1[22] = bf0[22] + bf0[21];
    bf1[23] = bf0[23] + bf0[20];
    bf1[24] = bf0[24] + bf0[27];
    bf1[25] = bf0[25] + bf0[26];
    bf1[26] = -bf0[26] + bf0[25];
    bf1[27] = -bf0[27] + bf0[24];
    bf1[28] = -bf0[28] + bf0[31];
    bf1[29] = -bf0[29] + bf0[30];
    bf1[30] = bf0[30] + bf0[29];
    bf1[31] = bf0[31] + bf0[28];
    bf1[32] = bf0[32];
    bf1[33] = bf0[33];
    bf1[34] = half_btf(-cospi[8], bf0[34], cospi[56], bf0[61], cos_bit);
    bf1[35] = half_btf(-cospi[8], bf0[35], cospi[56], bf0[60], cos_bit);
    bf1[36] = half_btf(-cospi[56], bf0[36], -cospi[8], bf0[59], cos_bit);
    bf1[37] = half_btf(-cospi[56], bf0[37], -cospi[8], bf0[58], cos_bit);
    bf1[38] = bf0[38];
    bf1[39] = bf0[39];
    bf1[40] = bf0[40];
    bf1[41] = bf0[41];
    bf1[42] = half_btf(-cospi[40], bf0[42], cospi[24], bf0[53], cos_bit);
    bf1[43] = half_btf(-cospi[40], bf0[43], cospi[24], bf0[52], cos_bit);
    bf1[44] = half_btf(-cospi[24], bf0[44], -cospi[40], bf0[51], cos_bit);
    bf1[45] = half_btf(-cospi[24], bf0[45], -cospi[40], bf0[50], cos_bit);
    bf1[46] = bf0[46];
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = bf0[49];
    bf1[50] = half_btf(cospi[24], bf0[50], -cospi[40], bf0[45], cos_bit);
    bf1[51] = half_btf(cospi[24], bf0[51], -cospi[40], bf0[44], cos_bit);
    bf1[52] = half_btf(cospi[40], bf0[52], cospi[24], bf0[43], cos_bit);
    bf1[53] = half_btf(cospi[40], bf0[53], cospi[24], bf0[42], cos_bit);
    bf1[54] = bf0[54];
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = half_btf(cospi[56], bf0[58], -cospi[8], bf0[37], cos_bit);
    bf1[59] = half_btf(cospi[56], bf0[59], -cospi[8], bf0[36], cos_bit);
    bf1[60] = half_btf(cospi[8], bf0[60], cospi[56], bf0[35], cos_bit);
    bf1[61] = half_btf(cospi[8], bf0[61], cospi[56], bf0[34], cos_bit);
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];

    // stage 7
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[4]  = half_btf(cospi[56], bf0[4], cospi[8], bf0[7], cos_bit);
    bf1[8]  = bf0[8] + bf0[9];
    bf1[11] = bf0[11] + bf0[10];
    bf1[12] = bf0[12] + bf0[13];
    bf1[15] = bf0[15] + bf0[14];
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(cospi[24], bf0[25], -cospi[40], bf0[22], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[21], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(cospi[56], bf0[29], -cospi[8], bf0[18], cos_bit);
    bf1[30] = half_btf(cospi[8], bf0[30], cospi[56], bf0[17], cos_bit);
    bf1[31] = bf0[31];
    bf1[32] = bf0[32] + bf0[35];
    bf1[33] = bf0[33] + bf0[34];
    bf1[34] = -bf0[34] + bf0[33];
    bf1[35] = -bf0[35] + bf0[32];
    bf1[36] = -bf0[36] + bf0[39];
    bf1[37] = -bf0[37] + bf0[38];
    bf1[38] = bf0[38] + bf0[37];
    bf1[39] = bf0[39] + bf0[36];
    bf1[40] = bf0[40] + bf0[43];
    bf1[41] = bf0[41] + bf0[42];
    bf1[42] = -bf0[42] + bf0[41];
    bf1[43] = -bf0[43] + bf0[40];
    bf1[44] = -bf0[44] + bf0[47];
    bf1[45] = -bf0[45] + bf0[46];
    bf1[46] = bf0[46] + bf0[45];
    bf1[47] = bf0[47] + bf0[44];
    bf1[48] = bf0[48] + bf0[51];
    bf1[49] = bf0[49] + bf0[50];
    bf1[50] = -bf0[50] + bf0[49];
    bf1[51] = -bf0[51] + bf0[48];
    bf1[52] = -bf0[52] + bf0[55];
    bf1[53] = -bf0[53] + bf0[54];
    bf1[54] = bf0[54] + bf0[53];
    bf1[55] = bf0[55] + bf0[52];
    bf1[56] = bf0[56] + bf0[59];
    bf1[57] = bf0[57] + bf0[58];
    bf1[58] = -bf0[58] + bf0[57];
    bf1[59] = -bf0[59] + bf0[56];
    bf1[60] = -bf0[60] + bf0[63];
    bf1[61] = -bf0[61] + bf0[62];
    bf1[62] = bf0[62] + bf0[61];
    bf1[63] = bf0[63] + bf0[60];

    // stage 8
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[4]  = bf0[4];
    bf1[8]  = half_btf(cospi[60], bf0[8], cospi[4], bf0[15], cos_bit);
    bf1[12] = half_btf(cospi[12], bf0[12], -cospi[52], bf0[11], cos_bit);
    bf1[16] = bf0[16] + bf0[17];
    bf1[19] = bf0[19] + bf0[18];
    bf1[20] = bf0[20] + bf0[21];
    bf1[23] = bf0[23] + bf0[22];
    bf1[24] = bf0[24] + bf0[25];
    bf1[27] = bf0[27] + bf0[26];
    bf1[28] = bf0[28] + bf0[29];
    bf1[31] = bf0[31] + bf0[30];
    bf1[32] = bf0[32];
    bf1[33] = half_btf(-cospi[4], bf0[33], cospi[60], bf0[62], cos_bit);
    bf1[34] = half_btf(-cospi[60], bf0[34], -cospi[4], bf0[61], cos_bit);
    bf1[35] = bf0[35];
    bf1[36] = bf0[36];
    bf1[37] = half_btf(-cospi[36], bf0[37], cospi[28], bf0[58], cos_bit);
    bf1[38] = half_btf(-cospi[28], bf0[38], -cospi[36], bf0[57], cos_bit);
    bf1[39] = bf0[39];
    bf1[40] = bf0[40];
    bf1[41] = half_btf(-cospi[20], bf0[41], cospi[44], bf0[54], cos_bit);
    bf1[42] = half_btf(-cospi[44], bf0[42], -cospi[20], bf0[53], cos_bit);
    bf1[43] = bf0[43];
    bf1[44] = bf0[44];
    bf1[45] = half_btf(-cospi[52], bf0[45], cospi[12], bf0[50], cos_bit);
    bf1[46] = half_btf(-cospi[12], bf0[46], -cospi[52], bf0[49], cos_bit);
    bf1[47] = bf0[47];
    bf1[48] = bf0[48];
    bf1[49] = half_btf(cospi[12], bf0[49], -cospi[52], bf0[46], cos_bit);
    bf1[50] = half_btf(cospi[52], bf0[50], cospi[12], bf0[45], cos_bit);
    bf1[51] = bf0[51];
    bf1[52] = bf0[52];
    bf1[53] = half_btf(cospi[44], bf0[53], -cospi[20], bf0[42], cos_bit);
    bf1[54] = half_btf(cospi[20], bf0[54], cospi[44], bf0[41], cos_bit);
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = half_btf(cospi[28], bf0[57], -cospi[36], bf0[38], cos_bit);
    bf1[58] = half_btf(cospi[36], bf0[58], cospi[28], bf0[37], cos_bit);
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = half_btf(cospi[60], bf0[61], -cospi[4], bf0[34], cos_bit);
    bf1[62] = half_btf(cospi[4], bf0[62], cospi[60], bf0[33], cos_bit);
    bf1[63] = bf0[63];

    // stage 9
    cospi   = cospi_arr(cos_bit);
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[4]  = bf0[4];
    bf1[8]  = bf0[8];
    bf1[12] = bf0[12];
    bf1[16] = half_btf(cospi[62], bf0[16], cospi[2], bf0[31], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], cospi[10], bf0[27], cos_bit);
    bf1[24] = half_btf(cospi[6], bf0[24], -cospi[58], bf0[23], cos_bit);
    bf1[28] = half_btf(cospi[14], bf0[28], -cospi[50], bf0[19], cos_bit);
    bf1[32] = bf0[32] + bf0[33];
    bf1[35] = bf0[35] + bf0[34];
    bf1[36] = bf0[36] + bf0[37];
    bf1[39] = bf0[39] + bf0[38];
    bf1[40] = bf0[40] + bf0[41];
    bf1[43] = bf0[43] + bf0[42];
    bf1[44] = bf0[44] + bf0[45];
    bf1[47] = bf0[47] + bf0[46];
    bf1[48] = bf0[48] + bf0[49];
    bf1[51] = bf0[51] + bf0[50];
    bf1[52] = bf0[52] + bf0[53];
    bf1[55] = bf0[55] + bf0[54];
    bf1[56] = bf0[56] + bf0[57];
    bf1[59] = bf0[59] + bf0[58];
    bf1[60] = bf0[60] + bf0[61];
    bf1[63] = bf0[63] + bf0[62];

    // stage 10
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[4]  = bf0[4];
    bf1[8]  = bf0[8];
    bf1[12] = bf0[12];
    bf1[16] = bf0[16];
    bf1[20] = bf0[20];
    bf1[24] = bf0[24];
    bf1[28] = bf0[28];
    bf1[32] = half_btf(cospi[63], bf0[32], cospi[1], bf0[63], cos_bit);
    bf1[36] = half_btf(cospi[55], bf0[36], cospi[9], bf0[59], cos_bit);
    bf1[40] = half_btf(cospi[59], bf0[40], cospi[5], bf0[55], cos_bit);
    bf1[44] = half_btf(cospi[51], bf0[44], cospi[13], bf0[51], cos_bit);
    bf1[48] = half_btf(cospi[3], bf0[48], -cospi[61], bf0[47], cos_bit);
    bf1[52] = half_btf(cospi[11], bf0[52], -cospi[53], bf0[43], cos_bit);
    bf1[56] = half_btf(cospi[7], bf0[56], -cospi[57], bf0[39], cos_bit);
    bf1[60] = half_btf(cospi[15], bf0[60], -cospi[49], bf0[35], cos_bit);

    // stage 11
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[32];
    bf1[2]  = bf0[16];
    bf1[3]  = bf0[48];
    bf1[4]  = bf0[8];
    bf1[5]  = bf0[40];
    bf1[6]  = bf0[24];
    bf1[7]  = bf0[56];
    bf1[8]  = bf0[4];
    bf1[9]  = bf0[36];
    bf1[10] = bf0[20];
    bf1[11] = bf0[52];
    bf1[12] = bf0[12];
    bf1[13] = bf0[44];
    bf1[14] = bf0[28];
    bf1[15] = bf0[60];
}

static void av1_fidentity64_N4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    (void)cos_bit;
    for (int32_t i = 0; i < 16; ++i) {
        output[i] = round_shift((int64_t)input[i] * 4 * new_sqrt2, new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

static INLINE TxfmFunc fwd_txfm_type_to_func_N4(TxfmType txfmtype) {
    switch (txfmtype) {
    case TXFM_TYPE_DCT4:
        return svt_av1_fdct4_new_N4;
    case TXFM_TYPE_DCT8:
        return svt_av1_fdct8_new_N4;
    case TXFM_TYPE_DCT16:
        return svt_av1_fdct16_new_N4;
    case TXFM_TYPE_DCT32:
        return svt_av1_fdct32_new_N4;
    case TXFM_TYPE_DCT64:
        return svt_av1_fdct64_new_N4;
    case TXFM_TYPE_ADST4:
        return svt_av1_fadst4_new_N4;
    case TXFM_TYPE_ADST8:
        return svt_av1_fadst8_new_N4;
    case TXFM_TYPE_ADST16:
        return svt_av1_fadst16_new_N4;
    case TXFM_TYPE_ADST32:
        return av1_fadst32_new;
    case TXFM_TYPE_IDENTITY4:
        return svt_av1_fidentity4_N4_c;
    case TXFM_TYPE_IDENTITY8:
        return svt_av1_fidentity8_N4_c;
    case TXFM_TYPE_IDENTITY16:
        return svt_av1_fidentity16_N4_c;
    case TXFM_TYPE_IDENTITY32:
        return svt_av1_fidentity32_N4_c;
    case TXFM_TYPE_IDENTITY64:
        return av1_fidentity64_N4_c;
    default:
        assert(0);
        return NULL;
    }
}

static INLINE void av1_tranform_two_d_core_N4_c(int16_t* input, uint32_t input_stride, int32_t* output,
                                                const Txfm2dFlipCfg* cfg, int32_t* buf, uint8_t bit_depth) {
    int32_t c, r;
    // Note when assigning txfm_size_col, we use the txfm_size from the
    // row configuration and vice versa. This is intentionally done to
    // accurately perform rectangular transforms. When the transform is
    // rectangular, the number of columns will be the same as the
    // txfm_size stored in the row cfg struct. It will make no difference
    // for square transforms.
    const int32_t txfm_size_col = tx_size_wide[cfg->tx_size];
    const int32_t txfm_size_row = tx_size_high[cfg->tx_size];
    // Take the shift from the larger dimension in the rectangular case.
    const int8_t* shift     = cfg->shift;
    const int32_t rect_type = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);
    int8_t        stage_range_col[MAX_TXFM_STAGE_NUM];
    int8_t        stage_range_row[MAX_TXFM_STAGE_NUM];
    assert(cfg->stage_num_col <= MAX_TXFM_STAGE_NUM);
    assert(cfg->stage_num_row <= MAX_TXFM_STAGE_NUM);
    svt_av1_gen_fwd_stage_range(stage_range_col, stage_range_row, cfg, bit_depth);

    const int8_t   cos_bit_col   = cfg->cos_bit_col;
    const int8_t   cos_bit_row   = cfg->cos_bit_row;
    const TxfmFunc txfm_func_col = fwd_txfm_type_to_func_N4(cfg->txfm_type_col);
    const TxfmFunc txfm_func_row = fwd_txfm_type_to_func_N4(cfg->txfm_type_row);
    ASSERT(txfm_func_col != NULL);
    ASSERT(txfm_func_row != NULL);
    // use output buffer as temp buffer
    int32_t* temp_in  = output;
    int32_t* temp_out = output + txfm_size_row;

    // Columns
    for (c = 0; c < txfm_size_col; ++c) {
        if (cfg->ud_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                temp_in[r] = input[r * input_stride + c];
            }
        } else {
            for (r = 0; r < txfm_size_row; ++r) {
                // flip upside down
                temp_in[r] = input[(txfm_size_row - r - 1) * input_stride + c];
            }
        }
        svt_av1_round_shift_array_c(temp_in, txfm_size_row, -shift[0]); // NM svt_av1_round_shift_array_c
        txfm_func_col(temp_in, temp_out, cos_bit_col, stage_range_col);
        svt_av1_round_shift_array_c(temp_out, txfm_size_row / 4, -shift[1]); // NM svt_av1_round_shift_array_c
        if (cfg->lr_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                buf[r * txfm_size_col + c] = temp_out[r];
            }
        } else {
            for (r = 0; r < txfm_size_row; ++r) {
                // flip from left to right
                buf[r * txfm_size_col + (txfm_size_col - c - 1)] = temp_out[r];
            }
        }
    }

    // Rows
    for (r = 0; r < txfm_size_row / 4; ++r) {
        txfm_func_row(buf + r * txfm_size_col, output + r * txfm_size_col, cos_bit_row, stage_range_row);
        svt_av1_round_shift_array_c(output + r * txfm_size_col, txfm_size_col / 4, -shift[2]);

        if (abs(rect_type) == 1) {
            // Multiply everything by Sqrt2 if the transform is rectangular and the
            // size difference is a factor of 2.
            for (c = 0; c < txfm_size_col / 4; ++c) {
                output[r * txfm_size_col + c] = round_shift((int64_t)output[r * txfm_size_col + c] * new_sqrt2,
                                                            new_sqrt2_bits);
            }
        }
    }
    for (int i = 0; i < (txfm_size_col * txfm_size_row); i++) {
        if (i % txfm_size_col >= (txfm_size_col >> 2) || i / txfm_size_col >= (txfm_size_row >> 2)) {
            output[i] = 0;
        }
    }
}

void svt_aom_transform_two_d_64x64_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                        uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 64];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_64X64, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_32x32_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                        uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X32, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_16x16_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                        uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X16, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_8x8_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                      uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X8, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_aom_transform_two_d_4x4_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                      uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 4];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_4X4, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_64x32_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_64X32, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x64_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 64];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X64, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_64x16_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[64 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_64X16, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x64_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 64];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X64, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x16_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X16, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x32_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                   uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X32, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x8_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X8, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x16_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X16, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_32x8_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[32 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_32X8, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x32_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 32];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X32, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_16x4_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[16 * 4];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_16X4, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_4x16_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                  uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 16];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_4X16, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_8x4_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                 uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[8 * 4];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_8X4, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}

void svt_av1_fwd_txfm2d_4x8_N4_c(int16_t* input, int32_t* output, uint32_t input_stride, TxType transform_type,
                                 uint8_t bit_depth) {
    int32_t       intermediate_transform_buffer[4 * 8];
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(transform_type, TX_4X8, &cfg);
    av1_tranform_two_d_core_N4_c(input, input_stride, output, &cfg, intermediate_transform_buffer, bit_depth);
}
