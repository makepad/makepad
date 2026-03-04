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
#include "inv_transforms.h"
#include "common_dsp_rtcd.h"

static const int8_t inv_shift_4x4[2]   = {0, -4};
static const int8_t inv_shift_8x8[2]   = {-1, -4};
static const int8_t inv_shift_16x16[2] = {-2, -4};
static const int8_t inv_shift_32x32[2] = {-2, -4};
static const int8_t inv_shift_64x64[2] = {-2, -4};
static const int8_t inv_shift_4x8[2]   = {0, -4};
static const int8_t inv_shift_8x4[2]   = {0, -4};
static const int8_t inv_shift_8x16[2]  = {-1, -4};
static const int8_t inv_shift_16x8[2]  = {-1, -4};
static const int8_t inv_shift_16x32[2] = {-1, -4};
static const int8_t inv_shift_32x16[2] = {-1, -4};
static const int8_t inv_shift_32x64[2] = {-1, -4};
static const int8_t inv_shift_64x32[2] = {-1, -4};
static const int8_t inv_shift_4x16[2]  = {-1, -4};
static const int8_t inv_shift_16x4[2]  = {-1, -4};
static const int8_t inv_shift_8x32[2]  = {-2, -4};
static const int8_t inv_shift_32x8[2]  = {-2, -4};
static const int8_t inv_shift_16x64[2] = {-2, -4};
static const int8_t inv_shift_64x16[2] = {-2, -4};

const int8_t* svt_aom_inv_txfm_shift_ls[TX_SIZES_ALL] = {
    inv_shift_4x4,  inv_shift_8x8,  inv_shift_16x16, inv_shift_32x32, inv_shift_64x64, inv_shift_4x8,   inv_shift_8x4,
    inv_shift_8x16, inv_shift_16x8, inv_shift_16x32, inv_shift_32x16, inv_shift_32x64, inv_shift_64x32, inv_shift_4x16,
    inv_shift_16x4, inv_shift_8x32, inv_shift_32x8,  inv_shift_16x64, inv_shift_64x16,
};

void svt_av1_gen_inv_stage_range(int8_t* stage_range_col, int8_t* stage_range_row, const Txfm2dFlipCfg* cfg,
                                 TxSize tx_size, int32_t bd) {
    const int32_t fwd_shift = inv_start_range[tx_size];
    const int8_t* shift     = cfg->shift;
    int8_t        opt_range_row, opt_range_col;
    if (bd == 8) {
        opt_range_row = 16;
        opt_range_col = 16;
    } else if (bd == 10) {
        opt_range_row = 18;
        opt_range_col = 16;
    } else {
        assert(bd == 12);
        opt_range_row = 20;
        opt_range_col = 18;
    }
    // i < MAX_TXFM_STAGE_NUM will mute above array bounds warning
    for (int32_t i = 0; i < cfg->stage_num_row && i < MAX_TXFM_STAGE_NUM; ++i) {
        int32_t real_range_row = cfg->stage_range_row[i] + fwd_shift + bd + 1;
        (void)real_range_row;
        if (cfg->txfm_type_row == TXFM_TYPE_ADST4 && i == 1) {
            // the adst4 may use 1 extra bit on top of opt_range_row at stage 1
            // so opt_range_col >= real_range_col will not hold
            stage_range_row[i] = opt_range_row;
        } else {
            assert(opt_range_row >= real_range_row);
            stage_range_row[i] = opt_range_row;
        }
    }
    // i < MAX_TXFM_STAGE_NUM will mute above array bounds warning
    for (int32_t i = 0; i < cfg->stage_num_col && i < MAX_TXFM_STAGE_NUM; ++i) {
        int32_t real_range_col = cfg->stage_range_col[i] + fwd_shift + shift[0] + bd + 1;
        (void)real_range_col;
        if (cfg->txfm_type_col == TXFM_TYPE_ADST4 && i == 1) {
            // the adst4 may use 1 extra bit on top of opt_range_row at stage 1
            // so opt_range_col >= real_range_col will not hold
            stage_range_col[i] = opt_range_col;
        } else {
            assert(opt_range_col >= real_range_col);
            stage_range_col[i] = opt_range_col;
        }
    }
}

static INLINE int32_t clamp_value(int32_t value, int8_t bit) {
    if (bit <= 0) {
        return value; // Do nothing for invalid clamp bit.
    }
    const int64_t max_value = (1LL << (bit - 1)) - 1;
    const int64_t min_value = -(1LL << (bit - 1));
    return (int32_t)clamp64(value, min_value, max_value);
}

void svt_av1_idct4_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[4];

    // stage 0;

    // stage 1;
    stage++;
    bf1    = output;
    bf1[0] = input[0];
    bf1[1] = input[2];
    bf1[2] = input[1];
    bf1[3] = input[3];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
    bf0    = output;
    bf1    = step;
    bf1[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1] = half_btf(cospi[32], bf0[0], -cospi[32], bf0[1], cos_bit);
    bf1[2] = half_btf(cospi[48], bf0[2], -cospi[16], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[16], bf0[2], cospi[48], bf0[3], cos_bit);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
    bf0    = step;
    bf1    = output;
    bf1[0] = clamp_value(bf0[0] + bf0[3], stage_range[stage]);
    bf1[1] = clamp_value(bf0[1] + bf0[2], stage_range[stage]);
    bf1[2] = clamp_value(bf0[1] - bf0[2], stage_range[stage]);
    bf1[3] = clamp_value(bf0[0] - bf0[3], stage_range[stage]);
}

void svt_av1_idct8_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    stage++;
    bf1    = output;
    bf1[0] = input[0];
    bf1[1] = input[4];
    bf1[2] = input[2];
    bf1[3] = input[6];
    bf1[4] = input[1];
    bf1[5] = input[5];
    bf1[6] = input[3];
    bf1[7] = input[7];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
    bf0    = output;
    bf1    = step;
    bf1[0] = bf0[0];
    bf1[1] = bf0[1];
    bf1[2] = bf0[2];
    bf1[3] = bf0[3];
    bf1[4] = half_btf(cospi[56], bf0[4], -cospi[8], bf0[7], cos_bit);
    bf1[5] = half_btf(cospi[24], bf0[5], -cospi[40], bf0[6], cos_bit);
    bf1[6] = half_btf(cospi[40], bf0[5], cospi[24], bf0[6], cos_bit);
    bf1[7] = half_btf(cospi[8], bf0[4], cospi[56], bf0[7], cos_bit);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
    bf0    = step;
    bf1    = output;
    bf1[0] = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1] = half_btf(cospi[32], bf0[0], -cospi[32], bf0[1], cos_bit);
    bf1[2] = half_btf(cospi[48], bf0[2], -cospi[16], bf0[3], cos_bit);
    bf1[3] = half_btf(cospi[16], bf0[2], cospi[48], bf0[3], cos_bit);
    bf1[4] = clamp_value(bf0[4] + bf0[5], stage_range[stage]);
    bf1[5] = clamp_value(bf0[4] - bf0[5], stage_range[stage]);
    bf1[6] = clamp_value(-bf0[6] + bf0[7], stage_range[stage]);
    bf1[7] = clamp_value(bf0[6] + bf0[7], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 4
    stage++;
    bf0    = output;
    bf1    = step;
    bf1[0] = clamp_value(bf0[0] + bf0[3], stage_range[stage]);
    bf1[1] = clamp_value(bf0[1] + bf0[2], stage_range[stage]);
    bf1[2] = clamp_value(bf0[1] - bf0[2], stage_range[stage]);
    bf1[3] = clamp_value(bf0[0] - bf0[3], stage_range[stage]);
    bf1[4] = bf0[4];
    bf1[5] = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6] = half_btf(cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[7] = bf0[7];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 5
    stage++;
    bf0    = step;
    bf1    = output;
    bf1[0] = clamp_value(bf0[0] + bf0[7], stage_range[stage]);
    bf1[1] = clamp_value(bf0[1] + bf0[6], stage_range[stage]);
    bf1[2] = clamp_value(bf0[2] + bf0[5], stage_range[stage]);
    bf1[3] = clamp_value(bf0[3] + bf0[4], stage_range[stage]);
    bf1[4] = clamp_value(bf0[3] - bf0[4], stage_range[stage]);
    bf1[5] = clamp_value(bf0[2] - bf0[5], stage_range[stage]);
    bf1[6] = clamp_value(bf0[1] - bf0[6], stage_range[stage]);
    bf1[7] = clamp_value(bf0[0] - bf0[7], stage_range[stage]);
}

void svt_av1_idct16_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    stage++;
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = input[8];
    bf1[2]  = input[4];
    bf1[3]  = input[12];
    bf1[4]  = input[2];
    bf1[5]  = input[10];
    bf1[6]  = input[6];
    bf1[7]  = input[14];
    bf1[8]  = input[1];
    bf1[9]  = input[9];
    bf1[10] = input[5];
    bf1[11] = input[13];
    bf1[12] = input[3];
    bf1[13] = input[11];
    bf1[14] = input[7];
    bf1[15] = input[15];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
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
    bf1[8]  = half_btf(cospi[60], bf0[8], -cospi[4], bf0[15], cos_bit);
    bf1[9]  = half_btf(cospi[28], bf0[9], -cospi[36], bf0[14], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], -cospi[20], bf0[13], cos_bit);
    bf1[11] = half_btf(cospi[12], bf0[11], -cospi[52], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[52], bf0[11], cospi[12], bf0[12], cos_bit);
    bf1[13] = half_btf(cospi[20], bf0[10], cospi[44], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[36], bf0[9], cospi[28], bf0[14], cos_bit);
    bf1[15] = half_btf(cospi[4], bf0[8], cospi[60], bf0[15], cos_bit);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[56], bf0[4], -cospi[8], bf0[7], cos_bit);
    bf1[5]  = half_btf(cospi[24], bf0[5], -cospi[40], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[40], bf0[5], cospi[24], bf0[6], cos_bit);
    bf1[7]  = half_btf(cospi[8], bf0[4], cospi[56], bf0[7], cos_bit);
    bf1[8]  = clamp_value(bf0[8] + bf0[9], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[8] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(-bf0[10] + bf0[11], stage_range[stage]);
    bf1[11] = clamp_value(bf0[10] + bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[13], stage_range[stage]);
    bf1[13] = clamp_value(bf0[12] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(-bf0[14] + bf0[15], stage_range[stage]);
    bf1[15] = clamp_value(bf0[14] + bf0[15], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 4
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1]  = half_btf(cospi[32], bf0[0], -cospi[32], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], -cospi[16], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[16], bf0[2], cospi[48], bf0[3], cos_bit);
    bf1[4]  = clamp_value(bf0[4] + bf0[5], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[4] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(-bf0[6] + bf0[7], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[6] + bf0[7], stage_range[stage]);
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(-cospi[16], bf0[10], cospi[48], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[48], bf0[9], cospi[16], bf0[14], cos_bit);
    bf1[15] = bf0[15];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 5
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[3], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[2], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[1] - bf0[2], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[0] - bf0[3], stage_range[stage]);
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = clamp_value(bf0[8] + bf0[11], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[10], stage_range[stage]);
    bf1[10] = clamp_value(bf0[9] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[8] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(-bf0[12] + bf0[15], stage_range[stage]);
    bf1[13] = clamp_value(-bf0[13] + bf0[14], stage_range[stage]);
    bf1[14] = clamp_value(bf0[13] + bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[12] + bf0[15], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 6
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = clamp_value(bf0[0] + bf0[7], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[6], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[5], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[4], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[3] - bf0[4], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[2] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[1] - bf0[6], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[0] - bf0[7], stage_range[stage]);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 7
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[15], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[14], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[13], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[12], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[11], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[10], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[9], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[8], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[7] - bf0[8], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[6] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(bf0[5] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[4] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[3] - bf0[12], stage_range[stage]);
    bf1[13] = clamp_value(bf0[2] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(bf0[1] - bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[0] - bf0[15], stage_range[stage]);
}

void svt_av1_idct32_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[32];

    // stage 0;

    // stage 1;
    stage++;
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = input[16];
    bf1[2]  = input[8];
    bf1[3]  = input[24];
    bf1[4]  = input[4];
    bf1[5]  = input[20];
    bf1[6]  = input[12];
    bf1[7]  = input[28];
    bf1[8]  = input[2];
    bf1[9]  = input[18];
    bf1[10] = input[10];
    bf1[11] = input[26];
    bf1[12] = input[6];
    bf1[13] = input[22];
    bf1[14] = input[14];
    bf1[15] = input[30];
    bf1[16] = input[1];
    bf1[17] = input[17];
    bf1[18] = input[9];
    bf1[19] = input[25];
    bf1[20] = input[5];
    bf1[21] = input[21];
    bf1[22] = input[13];
    bf1[23] = input[29];
    bf1[24] = input[3];
    bf1[25] = input[19];
    bf1[26] = input[11];
    bf1[27] = input[27];
    bf1[28] = input[7];
    bf1[29] = input[23];
    bf1[30] = input[15];
    bf1[31] = input[31];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
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
    bf1[16] = half_btf(cospi[62], bf0[16], -cospi[2], bf0[31], cos_bit);
    bf1[17] = half_btf(cospi[30], bf0[17], -cospi[34], bf0[30], cos_bit);
    bf1[18] = half_btf(cospi[46], bf0[18], -cospi[18], bf0[29], cos_bit);
    bf1[19] = half_btf(cospi[14], bf0[19], -cospi[50], bf0[28], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], -cospi[10], bf0[27], cos_bit);
    bf1[21] = half_btf(cospi[22], bf0[21], -cospi[42], bf0[26], cos_bit);
    bf1[22] = half_btf(cospi[38], bf0[22], -cospi[26], bf0[25], cos_bit);
    bf1[23] = half_btf(cospi[6], bf0[23], -cospi[58], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[58], bf0[23], cospi[6], bf0[24], cos_bit);
    bf1[25] = half_btf(cospi[26], bf0[22], cospi[38], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[42], bf0[21], cospi[22], bf0[26], cos_bit);
    bf1[27] = half_btf(cospi[10], bf0[20], cospi[54], bf0[27], cos_bit);
    bf1[28] = half_btf(cospi[50], bf0[19], cospi[14], bf0[28], cos_bit);
    bf1[29] = half_btf(cospi[18], bf0[18], cospi[46], bf0[29], cos_bit);
    bf1[30] = half_btf(cospi[34], bf0[17], cospi[30], bf0[30], cos_bit);
    bf1[31] = half_btf(cospi[2], bf0[16], cospi[62], bf0[31], cos_bit);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
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
    bf1[8]  = half_btf(cospi[60], bf0[8], -cospi[4], bf0[15], cos_bit);
    bf1[9]  = half_btf(cospi[28], bf0[9], -cospi[36], bf0[14], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], -cospi[20], bf0[13], cos_bit);
    bf1[11] = half_btf(cospi[12], bf0[11], -cospi[52], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[52], bf0[11], cospi[12], bf0[12], cos_bit);
    bf1[13] = half_btf(cospi[20], bf0[10], cospi[44], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[36], bf0[9], cospi[28], bf0[14], cos_bit);
    bf1[15] = half_btf(cospi[4], bf0[8], cospi[60], bf0[15], cos_bit);
    bf1[16] = clamp_value(bf0[16] + bf0[17], stage_range[stage]);
    bf1[17] = clamp_value(bf0[16] - bf0[17], stage_range[stage]);
    bf1[18] = clamp_value(-bf0[18] + bf0[19], stage_range[stage]);
    bf1[19] = clamp_value(bf0[18] + bf0[19], stage_range[stage]);
    bf1[20] = clamp_value(bf0[20] + bf0[21], stage_range[stage]);
    bf1[21] = clamp_value(bf0[20] - bf0[21], stage_range[stage]);
    bf1[22] = clamp_value(-bf0[22] + bf0[23], stage_range[stage]);
    bf1[23] = clamp_value(bf0[22] + bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(bf0[24] + bf0[25], stage_range[stage]);
    bf1[25] = clamp_value(bf0[24] - bf0[25], stage_range[stage]);
    bf1[26] = clamp_value(-bf0[26] + bf0[27], stage_range[stage]);
    bf1[27] = clamp_value(bf0[26] + bf0[27], stage_range[stage]);
    bf1[28] = clamp_value(bf0[28] + bf0[29], stage_range[stage]);
    bf1[29] = clamp_value(bf0[28] - bf0[29], stage_range[stage]);
    bf1[30] = clamp_value(-bf0[30] + bf0[31], stage_range[stage]);
    bf1[31] = clamp_value(bf0[30] + bf0[31], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 4
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[56], bf0[4], -cospi[8], bf0[7], cos_bit);
    bf1[5]  = half_btf(cospi[24], bf0[5], -cospi[40], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[40], bf0[5], cospi[24], bf0[6], cos_bit);
    bf1[7]  = half_btf(cospi[8], bf0[4], cospi[56], bf0[7], cos_bit);
    bf1[8]  = clamp_value(bf0[8] + bf0[9], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[8] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(-bf0[10] + bf0[11], stage_range[stage]);
    bf1[11] = clamp_value(bf0[10] + bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[13], stage_range[stage]);
    bf1[13] = clamp_value(bf0[12] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(-bf0[14] + bf0[15], stage_range[stage]);
    bf1[15] = clamp_value(bf0[14] + bf0[15], stage_range[stage]);
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(-cospi[40], bf0[22], cospi[24], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[24], bf0[21], cospi[40], bf0[26], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(-cospi[8], bf0[18], cospi[56], bf0[29], cos_bit);
    bf1[30] = half_btf(cospi[56], bf0[17], cospi[8], bf0[30], cos_bit);
    bf1[31] = bf0[31];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 5
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1]  = half_btf(cospi[32], bf0[0], -cospi[32], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], -cospi[16], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[16], bf0[2], cospi[48], bf0[3], cos_bit);
    bf1[4]  = clamp_value(bf0[4] + bf0[5], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[4] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(-bf0[6] + bf0[7], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[6] + bf0[7], stage_range[stage]);
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(-cospi[16], bf0[10], cospi[48], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[48], bf0[9], cospi[16], bf0[14], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = clamp_value(bf0[16] + bf0[19], stage_range[stage]);
    bf1[17] = clamp_value(bf0[17] + bf0[18], stage_range[stage]);
    bf1[18] = clamp_value(bf0[17] - bf0[18], stage_range[stage]);
    bf1[19] = clamp_value(bf0[16] - bf0[19], stage_range[stage]);
    bf1[20] = clamp_value(-bf0[20] + bf0[23], stage_range[stage]);
    bf1[21] = clamp_value(-bf0[21] + bf0[22], stage_range[stage]);
    bf1[22] = clamp_value(bf0[21] + bf0[22], stage_range[stage]);
    bf1[23] = clamp_value(bf0[20] + bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(bf0[24] + bf0[27], stage_range[stage]);
    bf1[25] = clamp_value(bf0[25] + bf0[26], stage_range[stage]);
    bf1[26] = clamp_value(bf0[25] - bf0[26], stage_range[stage]);
    bf1[27] = clamp_value(bf0[24] - bf0[27], stage_range[stage]);
    bf1[28] = clamp_value(-bf0[28] + bf0[31], stage_range[stage]);
    bf1[29] = clamp_value(-bf0[29] + bf0[30], stage_range[stage]);
    bf1[30] = clamp_value(bf0[29] + bf0[30], stage_range[stage]);
    bf1[31] = clamp_value(bf0[28] + bf0[31], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 6
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = clamp_value(bf0[0] + bf0[3], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[2], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[1] - bf0[2], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[0] - bf0[3], stage_range[stage]);
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = clamp_value(bf0[8] + bf0[11], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[10], stage_range[stage]);
    bf1[10] = clamp_value(bf0[9] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[8] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(-bf0[12] + bf0[15], stage_range[stage]);
    bf1[13] = clamp_value(-bf0[13] + bf0[14], stage_range[stage]);
    bf1[14] = clamp_value(bf0[13] + bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[12] + bf0[15], stage_range[stage]);
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
    bf1[26] = half_btf(-cospi[16], bf0[21], cospi[48], bf0[26], cos_bit);
    bf1[27] = half_btf(-cospi[16], bf0[20], cospi[48], bf0[27], cos_bit);
    bf1[28] = half_btf(cospi[48], bf0[19], cospi[16], bf0[28], cos_bit);
    bf1[29] = half_btf(cospi[48], bf0[18], cospi[16], bf0[29], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 7
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[7], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[6], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[5], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[4], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[3] - bf0[4], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[2] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[1] - bf0[6], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[0] - bf0[7], stage_range[stage]);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = clamp_value(bf0[16] + bf0[23], stage_range[stage]);
    bf1[17] = clamp_value(bf0[17] + bf0[22], stage_range[stage]);
    bf1[18] = clamp_value(bf0[18] + bf0[21], stage_range[stage]);
    bf1[19] = clamp_value(bf0[19] + bf0[20], stage_range[stage]);
    bf1[20] = clamp_value(bf0[19] - bf0[20], stage_range[stage]);
    bf1[21] = clamp_value(bf0[18] - bf0[21], stage_range[stage]);
    bf1[22] = clamp_value(bf0[17] - bf0[22], stage_range[stage]);
    bf1[23] = clamp_value(bf0[16] - bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(-bf0[24] + bf0[31], stage_range[stage]);
    bf1[25] = clamp_value(-bf0[25] + bf0[30], stage_range[stage]);
    bf1[26] = clamp_value(-bf0[26] + bf0[29], stage_range[stage]);
    bf1[27] = clamp_value(-bf0[27] + bf0[28], stage_range[stage]);
    bf1[28] = clamp_value(bf0[27] + bf0[28], stage_range[stage]);
    bf1[29] = clamp_value(bf0[26] + bf0[29], stage_range[stage]);
    bf1[30] = clamp_value(bf0[25] + bf0[30], stage_range[stage]);
    bf1[31] = clamp_value(bf0[24] + bf0[31], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 8
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = clamp_value(bf0[0] + bf0[15], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[14], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[13], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[12], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[11], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[10], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[9], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[8], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[7] - bf0[8], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[6] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(bf0[5] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[4] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[3] - bf0[12], stage_range[stage]);
    bf1[13] = clamp_value(bf0[2] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(bf0[1] - bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[0] - bf0[15], stage_range[stage]);
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 9
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[31], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[30], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[29], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[28], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[27], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[26], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[25], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[24], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[8] + bf0[23], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[22], stage_range[stage]);
    bf1[10] = clamp_value(bf0[10] + bf0[21], stage_range[stage]);
    bf1[11] = clamp_value(bf0[11] + bf0[20], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[19], stage_range[stage]);
    bf1[13] = clamp_value(bf0[13] + bf0[18], stage_range[stage]);
    bf1[14] = clamp_value(bf0[14] + bf0[17], stage_range[stage]);
    bf1[15] = clamp_value(bf0[15] + bf0[16], stage_range[stage]);
    bf1[16] = clamp_value(bf0[15] - bf0[16], stage_range[stage]);
    bf1[17] = clamp_value(bf0[14] - bf0[17], stage_range[stage]);
    bf1[18] = clamp_value(bf0[13] - bf0[18], stage_range[stage]);
    bf1[19] = clamp_value(bf0[12] - bf0[19], stage_range[stage]);
    bf1[20] = clamp_value(bf0[11] - bf0[20], stage_range[stage]);
    bf1[21] = clamp_value(bf0[10] - bf0[21], stage_range[stage]);
    bf1[22] = clamp_value(bf0[9] - bf0[22], stage_range[stage]);
    bf1[23] = clamp_value(bf0[8] - bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(bf0[7] - bf0[24], stage_range[stage]);
    bf1[25] = clamp_value(bf0[6] - bf0[25], stage_range[stage]);
    bf1[26] = clamp_value(bf0[5] - bf0[26], stage_range[stage]);
    bf1[27] = clamp_value(bf0[4] - bf0[27], stage_range[stage]);
    bf1[28] = clamp_value(bf0[3] - bf0[28], stage_range[stage]);
    bf1[29] = clamp_value(bf0[2] - bf0[29], stage_range[stage]);
    bf1[30] = clamp_value(bf0[1] - bf0[30], stage_range[stage]);
    bf1[31] = clamp_value(bf0[0] - bf0[31], stage_range[stage]);
}

void svt_av1_iadst4_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;
    int32_t        bit   = cos_bit;
    const int32_t* sinpi = sinpi_arr(bit);
    int32_t        s0, s1, s2, s3, s4, s5, s6, s7;

    int32_t x0 = input[0];
    int32_t x1 = input[1];
    int32_t x2 = input[2];
    int32_t x3 = input[3];

    if (!(x0 | x1 | x2 | x3)) {
        output[0] = output[1] = output[2] = output[3] = 0;
        return;
    }

    assert(sinpi[1] + sinpi[2] == sinpi[4]);

    // stage 1
    //s0 = range_check_value(sinpi[1] * x0, stage_range[1] + bit);
    //s1 = range_check_value(sinpi[2] * x0, stage_range[1] + bit);
    //s2 = range_check_value(sinpi[3] * x1, stage_range[1] + bit);
    //s3 = range_check_value(sinpi[4] * x2, stage_range[1] + bit);
    //s4 = range_check_value(sinpi[1] * x2, stage_range[1] + bit);
    //s5 = range_check_value(sinpi[2] * x3, stage_range[1] + bit);
    //s6 = range_check_value(sinpi[4] * x3, stage_range[1] + bit);

    s0 = sinpi[1] * x0;
    s1 = sinpi[2] * x0;
    s2 = sinpi[3] * x1;
    s3 = sinpi[4] * x2;
    s4 = sinpi[1] * x2;
    s5 = sinpi[2] * x3;
    s6 = sinpi[4] * x3;

    // stage 2
    // NOTICE: (x0 - x2) here may use one extra bit compared to the
    // opt_range_row/col specified in svt_av1_gen_inv_stage_range()
    //s7 = range_check_value((x0 - x2) + x3, stage_range[2]);

    //// stage 3
    //s0 = range_check_value(s0 + s3, stage_range[3] + bit);
    //s1 = range_check_value(s1 - s4, stage_range[3] + bit);
    //s3 = range_check_value(s2, stage_range[3] + bit);
    //s2 = range_check_value(sinpi[3] * s7, stage_range[3] + bit);

    //// stage 4
    //s0 = range_check_value(s0 + s5, stage_range[4] + bit);
    //s1 = range_check_value(s1 - s6, stage_range[4] + bit);

    //// stage 5
    //x0 = range_check_value(s0 + s3, stage_range[5] + bit);
    //x1 = range_check_value(s1 + s3, stage_range[5] + bit);
    //x2 = range_check_value(s2, stage_range[5] + bit);
    //x3 = range_check_value(s0 + s1, stage_range[5] + bit);

    //// stage 6
    //x3 = range_check_value(x3 - s3, stage_range[6] + bit);

    s7 = (x0 - x2) + x3;

    // stage 3
    s0 = s0 + s3;
    s1 = s1 - s4;
    s3 = s2;
    s2 = sinpi[3] * s7;

    // stage 4
    s0 = s0 + s5;
    s1 = s1 - s6;

    // stage 5
    x0 = s0 + s3;
    x1 = s1 + s3;
    x2 = s2;
    x3 = s0 + s1;

    // stage 6
    x3 = x3 - s3;

    output[0] = round_shift(x0, bit);
    output[1] = round_shift(x1, bit);
    output[2] = round_shift(x2, bit);
    output[3] = round_shift(x3, bit);
    //range_check_buf(6, input, output, 4, stage_range[6]);
}

static INLINE void clamp_buf(int32_t* buf, int32_t size, int8_t bit) {
    for (int32_t i = 0; i < size; ++i) {
        buf[i] = clamp_value(buf[i], bit);
    }
}

void svt_av1_iadst8_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[8];

    // stage 0;

    // stage 1;
    stage++;
    bf1    = output;
    bf1[0] = input[7];
    bf1[1] = input[0];
    bf1[2] = input[5];
    bf1[3] = input[2];
    bf1[4] = input[3];
    bf1[5] = input[4];
    bf1[6] = input[1];
    bf1[7] = input[6];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
    bf0    = step;
    bf1    = output;
    bf1[0] = clamp_value(bf0[0] + bf0[4], stage_range[stage]);
    bf1[1] = clamp_value(bf0[1] + bf0[5], stage_range[stage]);
    bf1[2] = clamp_value(bf0[2] + bf0[6], stage_range[stage]);
    bf1[3] = clamp_value(bf0[3] + bf0[7], stage_range[stage]);
    bf1[4] = clamp_value(bf0[0] - bf0[4], stage_range[stage]);
    bf1[5] = clamp_value(bf0[1] - bf0[5], stage_range[stage]);
    bf1[6] = clamp_value(bf0[2] - bf0[6], stage_range[stage]);
    bf1[7] = clamp_value(bf0[3] - bf0[7], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 4
    stage++;
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 5
    stage++;
    bf0    = step;
    bf1    = output;
    bf1[0] = clamp_value(bf0[0] + bf0[2], stage_range[stage]);
    bf1[1] = clamp_value(bf0[1] + bf0[3], stage_range[stage]);
    bf1[2] = clamp_value(bf0[0] - bf0[2], stage_range[stage]);
    bf1[3] = clamp_value(bf0[1] - bf0[3], stage_range[stage]);
    bf1[4] = clamp_value(bf0[4] + bf0[6], stage_range[stage]);
    bf1[5] = clamp_value(bf0[5] + bf0[7], stage_range[stage]);
    bf1[6] = clamp_value(bf0[4] - bf0[6], stage_range[stage]);
    bf1[7] = clamp_value(bf0[5] - bf0[7], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 6
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 7
    bf0    = step;
    bf1    = output;
    bf1[0] = bf0[0];
    bf1[1] = -bf0[4];
    bf1[2] = bf0[6];
    bf1[3] = -bf0[2];
    bf1[4] = bf0[3];
    bf1[5] = -bf0[7];
    bf1[6] = bf0[5];
    bf1[7] = -bf0[1];
}

void svt_av1_iadst16_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[16];

    // stage 0;

    // stage 1;
    stage++;
    bf1     = output;
    bf1[0]  = input[15];
    bf1[1]  = input[0];
    bf1[2]  = input[13];
    bf1[3]  = input[2];
    bf1[4]  = input[11];
    bf1[5]  = input[4];
    bf1[6]  = input[9];
    bf1[7]  = input[6];
    bf1[8]  = input[7];
    bf1[9]  = input[8];
    bf1[10] = input[5];
    bf1[11] = input[10];
    bf1[12] = input[3];
    bf1[13] = input[12];
    bf1[14] = input[1];
    bf1[15] = input[14];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[8], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[9], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[10], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[11], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[12], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[13], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[14], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[15], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[0] - bf0[8], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[1] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(bf0[2] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[3] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[4] - bf0[12], stage_range[stage]);
    bf1[13] = clamp_value(bf0[5] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(bf0[6] - bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[7] - bf0[15], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 4
    stage++;
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 5
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[4], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[5], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[6], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[7], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[0] - bf0[4], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[1] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[2] - bf0[6], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[3] - bf0[7], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[8] + bf0[12], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[13], stage_range[stage]);
    bf1[10] = clamp_value(bf0[10] + bf0[14], stage_range[stage]);
    bf1[11] = clamp_value(bf0[11] + bf0[15], stage_range[stage]);
    bf1[12] = clamp_value(bf0[8] - bf0[12], stage_range[stage]);
    bf1[13] = clamp_value(bf0[9] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(bf0[10] - bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[11] - bf0[15], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 6
    stage++;
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 7
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[2], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[3], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[0] - bf0[2], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[1] - bf0[3], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[6], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[7], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[4] - bf0[6], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[5] - bf0[7], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[8] + bf0[10], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[11], stage_range[stage]);
    bf1[10] = clamp_value(bf0[8] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[9] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[14], stage_range[stage]);
    bf1[13] = clamp_value(bf0[13] + bf0[15], stage_range[stage]);
    bf1[14] = clamp_value(bf0[12] - bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[13] - bf0[15], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 8
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
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 9
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = -bf0[8];
    bf1[2]  = bf0[12];
    bf1[3]  = -bf0[4];
    bf1[4]  = bf0[6];
    bf1[5]  = -bf0[14];
    bf1[6]  = bf0[10];
    bf1[7]  = -bf0[2];
    bf1[8]  = bf0[3];
    bf1[9]  = -bf0[11];
    bf1[10] = bf0[15];
    bf1[11] = -bf0[7];
    bf1[12] = bf0[5];
    bf1[13] = -bf0[13];
    bf1[14] = bf0[9];
    bf1[15] = -bf0[1];
}

static void av1_iadst32_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    const int32_t  size = 32;
    const int32_t* cospi;

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[32];

    // stage 0;
    clamp_buf((int32_t*)input, size, stage_range[stage]);

    // stage 1;
    stage++;
    assert(output != input);
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = -input[31];
    bf1[2]  = -input[15];
    bf1[3]  = input[16];
    bf1[4]  = -input[7];
    bf1[5]  = input[24];
    bf1[6]  = input[8];
    bf1[7]  = -input[23];
    bf1[8]  = -input[3];
    bf1[9]  = input[28];
    bf1[10] = input[12];
    bf1[11] = -input[19];
    bf1[12] = input[4];
    bf1[13] = -input[27];
    bf1[14] = -input[11];
    bf1[15] = input[20];
    bf1[16] = -input[1];
    bf1[17] = input[30];
    bf1[18] = input[14];
    bf1[19] = -input[17];
    bf1[20] = input[6];
    bf1[21] = -input[25];
    bf1[22] = -input[9];
    bf1[23] = input[22];
    bf1[24] = input[2];
    bf1[25] = -input[29];
    bf1[26] = -input[13];
    bf1[27] = input[18];
    bf1[28] = -input[5];
    bf1[29] = input[26];
    bf1[30] = input[10];
    bf1[31] = -input[21];
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 2
    stage++;
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
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = half_btf(cospi[32], bf0[18], cospi[32], bf0[19], cos_bit);
    bf1[19] = half_btf(cospi[32], bf0[18], -cospi[32], bf0[19], cos_bit);
    bf1[20] = bf0[20];
    bf1[21] = bf0[21];
    bf1[22] = half_btf(cospi[32], bf0[22], cospi[32], bf0[23], cos_bit);
    bf1[23] = half_btf(cospi[32], bf0[22], -cospi[32], bf0[23], cos_bit);
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = half_btf(cospi[32], bf0[26], cospi[32], bf0[27], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[26], -cospi[32], bf0[27], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = half_btf(cospi[32], bf0[30], cospi[32], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[32], bf0[30], -cospi[32], bf0[31], cos_bit);
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 3
    stage++;
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
    bf1[16] = bf0[16] + bf0[18];
    bf1[17] = bf0[17] + bf0[19];
    bf1[18] = bf0[16] - bf0[18];
    bf1[19] = bf0[17] - bf0[19];
    bf1[20] = bf0[20] + bf0[22];
    bf1[21] = bf0[21] + bf0[23];
    bf1[22] = bf0[20] - bf0[22];
    bf1[23] = bf0[21] - bf0[23];
    bf1[24] = bf0[24] + bf0[26];
    bf1[25] = bf0[25] + bf0[27];
    bf1[26] = bf0[24] - bf0[26];
    bf1[27] = bf0[25] - bf0[27];
    bf1[28] = bf0[28] + bf0[30];
    bf1[29] = bf0[29] + bf0[31];
    bf1[30] = bf0[28] - bf0[30];
    bf1[31] = bf0[29] - bf0[31];
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 4
    stage++;
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
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(cospi[16], bf0[20], cospi[48], bf0[21], cos_bit);
    bf1[21] = half_btf(cospi[48], bf0[20], -cospi[16], bf0[21], cos_bit);
    bf1[22] = half_btf(-cospi[48], bf0[22], cospi[16], bf0[23], cos_bit);
    bf1[23] = half_btf(cospi[16], bf0[22], cospi[48], bf0[23], cos_bit);
    bf1[24] = bf0[24];
    bf1[25] = bf0[25];
    bf1[26] = bf0[26];
    bf1[27] = bf0[27];
    bf1[28] = half_btf(cospi[16], bf0[28], cospi[48], bf0[29], cos_bit);
    bf1[29] = half_btf(cospi[48], bf0[28], -cospi[16], bf0[29], cos_bit);
    bf1[30] = half_btf(-cospi[48], bf0[30], cospi[16], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[16], bf0[30], cospi[48], bf0[31], cos_bit);
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 5
    stage++;
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
    bf1[16] = bf0[16] + bf0[20];
    bf1[17] = bf0[17] + bf0[21];
    bf1[18] = bf0[18] + bf0[22];
    bf1[19] = bf0[19] + bf0[23];
    bf1[20] = bf0[16] - bf0[20];
    bf1[21] = bf0[17] - bf0[21];
    bf1[22] = bf0[18] - bf0[22];
    bf1[23] = bf0[19] - bf0[23];
    bf1[24] = bf0[24] + bf0[28];
    bf1[25] = bf0[25] + bf0[29];
    bf1[26] = bf0[26] + bf0[30];
    bf1[27] = bf0[27] + bf0[31];
    bf1[28] = bf0[24] - bf0[28];
    bf1[29] = bf0[25] - bf0[29];
    bf1[30] = bf0[26] - bf0[30];
    bf1[31] = bf0[27] - bf0[31];
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 6
    stage++;
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
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = bf0[21];
    bf1[22] = bf0[22];
    bf1[23] = bf0[23];
    bf1[24] = half_btf(cospi[8], bf0[24], cospi[56], bf0[25], cos_bit);
    bf1[25] = half_btf(cospi[56], bf0[24], -cospi[8], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[40], bf0[26], cospi[24], bf0[27], cos_bit);
    bf1[27] = half_btf(cospi[24], bf0[26], -cospi[40], bf0[27], cos_bit);
    bf1[28] = half_btf(-cospi[56], bf0[28], cospi[8], bf0[29], cos_bit);
    bf1[29] = half_btf(cospi[8], bf0[28], cospi[56], bf0[29], cos_bit);
    bf1[30] = half_btf(-cospi[24], bf0[30], cospi[40], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[40], bf0[30], cospi[24], bf0[31], cos_bit);
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 7
    stage++;
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
    bf1[16] = bf0[16] + bf0[24];
    bf1[17] = bf0[17] + bf0[25];
    bf1[18] = bf0[18] + bf0[26];
    bf1[19] = bf0[19] + bf0[27];
    bf1[20] = bf0[20] + bf0[28];
    bf1[21] = bf0[21] + bf0[29];
    bf1[22] = bf0[22] + bf0[30];
    bf1[23] = bf0[23] + bf0[31];
    bf1[24] = bf0[16] - bf0[24];
    bf1[25] = bf0[17] - bf0[25];
    bf1[26] = bf0[18] - bf0[26];
    bf1[27] = bf0[19] - bf0[27];
    bf1[28] = bf0[20] - bf0[28];
    bf1[29] = bf0[21] - bf0[29];
    bf1[30] = bf0[22] - bf0[30];
    bf1[31] = bf0[23] - bf0[31];
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 8
    stage++;
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
    bf1[17] = half_btf(cospi[60], bf0[16], -cospi[4], bf0[17], cos_bit);
    bf1[18] = half_btf(cospi[20], bf0[18], cospi[44], bf0[19], cos_bit);
    bf1[19] = half_btf(cospi[44], bf0[18], -cospi[20], bf0[19], cos_bit);
    bf1[20] = half_btf(cospi[36], bf0[20], cospi[28], bf0[21], cos_bit);
    bf1[21] = half_btf(cospi[28], bf0[20], -cospi[36], bf0[21], cos_bit);
    bf1[22] = half_btf(cospi[52], bf0[22], cospi[12], bf0[23], cos_bit);
    bf1[23] = half_btf(cospi[12], bf0[22], -cospi[52], bf0[23], cos_bit);
    bf1[24] = half_btf(-cospi[60], bf0[24], cospi[4], bf0[25], cos_bit);
    bf1[25] = half_btf(cospi[4], bf0[24], cospi[60], bf0[25], cos_bit);
    bf1[26] = half_btf(-cospi[44], bf0[26], cospi[20], bf0[27], cos_bit);
    bf1[27] = half_btf(cospi[20], bf0[26], cospi[44], bf0[27], cos_bit);
    bf1[28] = half_btf(-cospi[28], bf0[28], cospi[36], bf0[29], cos_bit);
    bf1[29] = half_btf(cospi[36], bf0[28], cospi[28], bf0[29], cos_bit);
    bf1[30] = half_btf(-cospi[12], bf0[30], cospi[52], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[52], bf0[30], cospi[12], bf0[31], cos_bit);
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 9
    stage++;
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
    bf1[16] = bf0[0] - bf0[16];
    bf1[17] = bf0[1] - bf0[17];
    bf1[18] = bf0[2] - bf0[18];
    bf1[19] = bf0[3] - bf0[19];
    bf1[20] = bf0[4] - bf0[20];
    bf1[21] = bf0[5] - bf0[21];
    bf1[22] = bf0[6] - bf0[22];
    bf1[23] = bf0[7] - bf0[23];
    bf1[24] = bf0[8] - bf0[24];
    bf1[25] = bf0[9] - bf0[25];
    bf1[26] = bf0[10] - bf0[26];
    bf1[27] = bf0[11] - bf0[27];
    bf1[28] = bf0[12] - bf0[28];
    bf1[29] = bf0[13] - bf0[29];
    bf1[30] = bf0[14] - bf0[30];
    bf1[31] = bf0[15] - bf0[31];
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 10
    stage++;
    cospi   = cospi_arr(cos_bit);
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[1], bf0[0], cospi[63], bf0[1], cos_bit);
    bf1[1]  = half_btf(cospi[63], bf0[0], -cospi[1], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[5], bf0[2], cospi[59], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[59], bf0[2], -cospi[5], bf0[3], cos_bit);
    bf1[4]  = half_btf(cospi[9], bf0[4], cospi[55], bf0[5], cos_bit);
    bf1[5]  = half_btf(cospi[55], bf0[4], -cospi[9], bf0[5], cos_bit);
    bf1[6]  = half_btf(cospi[13], bf0[6], cospi[51], bf0[7], cos_bit);
    bf1[7]  = half_btf(cospi[51], bf0[6], -cospi[13], bf0[7], cos_bit);
    bf1[8]  = half_btf(cospi[17], bf0[8], cospi[47], bf0[9], cos_bit);
    bf1[9]  = half_btf(cospi[47], bf0[8], -cospi[17], bf0[9], cos_bit);
    bf1[10] = half_btf(cospi[21], bf0[10], cospi[43], bf0[11], cos_bit);
    bf1[11] = half_btf(cospi[43], bf0[10], -cospi[21], bf0[11], cos_bit);
    bf1[12] = half_btf(cospi[25], bf0[12], cospi[39], bf0[13], cos_bit);
    bf1[13] = half_btf(cospi[39], bf0[12], -cospi[25], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[29], bf0[14], cospi[35], bf0[15], cos_bit);
    bf1[15] = half_btf(cospi[35], bf0[14], -cospi[29], bf0[15], cos_bit);
    bf1[16] = half_btf(cospi[33], bf0[16], cospi[31], bf0[17], cos_bit);
    bf1[17] = half_btf(cospi[31], bf0[16], -cospi[33], bf0[17], cos_bit);
    bf1[18] = half_btf(cospi[37], bf0[18], cospi[27], bf0[19], cos_bit);
    bf1[19] = half_btf(cospi[27], bf0[18], -cospi[37], bf0[19], cos_bit);
    bf1[20] = half_btf(cospi[41], bf0[20], cospi[23], bf0[21], cos_bit);
    bf1[21] = half_btf(cospi[23], bf0[20], -cospi[41], bf0[21], cos_bit);
    bf1[22] = half_btf(cospi[45], bf0[22], cospi[19], bf0[23], cos_bit);
    bf1[23] = half_btf(cospi[19], bf0[22], -cospi[45], bf0[23], cos_bit);
    bf1[24] = half_btf(cospi[49], bf0[24], cospi[15], bf0[25], cos_bit);
    bf1[25] = half_btf(cospi[15], bf0[24], -cospi[49], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[53], bf0[26], cospi[11], bf0[27], cos_bit);
    bf1[27] = half_btf(cospi[11], bf0[26], -cospi[53], bf0[27], cos_bit);
    bf1[28] = half_btf(cospi[57], bf0[28], cospi[7], bf0[29], cos_bit);
    bf1[29] = half_btf(cospi[7], bf0[28], -cospi[57], bf0[29], cos_bit);
    bf1[30] = half_btf(cospi[61], bf0[30], cospi[3], bf0[31], cos_bit);
    bf1[31] = half_btf(cospi[3], bf0[30], -cospi[61], bf0[31], cos_bit);
    clamp_buf(bf1, size, stage_range[stage]);

    // stage 11
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[1];
    bf1[1]  = bf0[30];
    bf1[2]  = bf0[3];
    bf1[3]  = bf0[28];
    bf1[4]  = bf0[5];
    bf1[5]  = bf0[26];
    bf1[6]  = bf0[7];
    bf1[7]  = bf0[24];
    bf1[8]  = bf0[9];
    bf1[9]  = bf0[22];
    bf1[10] = bf0[11];
    bf1[11] = bf0[20];
    bf1[12] = bf0[13];
    bf1[13] = bf0[18];
    bf1[14] = bf0[15];
    bf1[15] = bf0[16];
    bf1[16] = bf0[17];
    bf1[17] = bf0[14];
    bf1[18] = bf0[19];
    bf1[19] = bf0[12];
    bf1[20] = bf0[21];
    bf1[21] = bf0[10];
    bf1[22] = bf0[23];
    bf1[23] = bf0[8];
    bf1[24] = bf0[25];
    bf1[25] = bf0[6];
    bf1[26] = bf0[27];
    bf1[27] = bf0[4];
    bf1[28] = bf0[29];
    bf1[29] = bf0[2];
    bf1[30] = bf0[31];
    bf1[31] = bf0[0];
    clamp_buf(bf1, size, stage_range[stage]);
}

void svt_av1_idct64_new(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    assert(output != input);
    const int32_t* cospi = cospi_arr(cos_bit);

    int32_t  stage = 0;
    int32_t *bf0, *bf1;
    int32_t  step[64];

    // stage 0;

    // stage 1;
    stage++;
    bf1     = output;
    bf1[0]  = input[0];
    bf1[1]  = input[32];
    bf1[2]  = input[16];
    bf1[3]  = input[48];
    bf1[4]  = input[8];
    bf1[5]  = input[40];
    bf1[6]  = input[24];
    bf1[7]  = input[56];
    bf1[8]  = input[4];
    bf1[9]  = input[36];
    bf1[10] = input[20];
    bf1[11] = input[52];
    bf1[12] = input[12];
    bf1[13] = input[44];
    bf1[14] = input[28];
    bf1[15] = input[60];
    bf1[16] = input[2];
    bf1[17] = input[34];
    bf1[18] = input[18];
    bf1[19] = input[50];
    bf1[20] = input[10];
    bf1[21] = input[42];
    bf1[22] = input[26];
    bf1[23] = input[58];
    bf1[24] = input[6];
    bf1[25] = input[38];
    bf1[26] = input[22];
    bf1[27] = input[54];
    bf1[28] = input[14];
    bf1[29] = input[46];
    bf1[30] = input[30];
    bf1[31] = input[62];
    bf1[32] = input[1];
    bf1[33] = input[33];
    bf1[34] = input[17];
    bf1[35] = input[49];
    bf1[36] = input[9];
    bf1[37] = input[41];
    bf1[38] = input[25];
    bf1[39] = input[57];
    bf1[40] = input[5];
    bf1[41] = input[37];
    bf1[42] = input[21];
    bf1[43] = input[53];
    bf1[44] = input[13];
    bf1[45] = input[45];
    bf1[46] = input[29];
    bf1[47] = input[61];
    bf1[48] = input[3];
    bf1[49] = input[35];
    bf1[50] = input[19];
    bf1[51] = input[51];
    bf1[52] = input[11];
    bf1[53] = input[43];
    bf1[54] = input[27];
    bf1[55] = input[59];
    bf1[56] = input[7];
    bf1[57] = input[39];
    bf1[58] = input[23];
    bf1[59] = input[55];
    bf1[60] = input[15];
    bf1[61] = input[47];
    bf1[62] = input[31];
    bf1[63] = input[63];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 2
    stage++;
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
    bf1[32] = half_btf(cospi[63], bf0[32], -cospi[1], bf0[63], cos_bit);
    bf1[33] = half_btf(cospi[31], bf0[33], -cospi[33], bf0[62], cos_bit);
    bf1[34] = half_btf(cospi[47], bf0[34], -cospi[17], bf0[61], cos_bit);
    bf1[35] = half_btf(cospi[15], bf0[35], -cospi[49], bf0[60], cos_bit);
    bf1[36] = half_btf(cospi[55], bf0[36], -cospi[9], bf0[59], cos_bit);
    bf1[37] = half_btf(cospi[23], bf0[37], -cospi[41], bf0[58], cos_bit);
    bf1[38] = half_btf(cospi[39], bf0[38], -cospi[25], bf0[57], cos_bit);
    bf1[39] = half_btf(cospi[7], bf0[39], -cospi[57], bf0[56], cos_bit);
    bf1[40] = half_btf(cospi[59], bf0[40], -cospi[5], bf0[55], cos_bit);
    bf1[41] = half_btf(cospi[27], bf0[41], -cospi[37], bf0[54], cos_bit);
    bf1[42] = half_btf(cospi[43], bf0[42], -cospi[21], bf0[53], cos_bit);
    bf1[43] = half_btf(cospi[11], bf0[43], -cospi[53], bf0[52], cos_bit);
    bf1[44] = half_btf(cospi[51], bf0[44], -cospi[13], bf0[51], cos_bit);
    bf1[45] = half_btf(cospi[19], bf0[45], -cospi[45], bf0[50], cos_bit);
    bf1[46] = half_btf(cospi[35], bf0[46], -cospi[29], bf0[49], cos_bit);
    bf1[47] = half_btf(cospi[3], bf0[47], -cospi[61], bf0[48], cos_bit);
    bf1[48] = half_btf(cospi[61], bf0[47], cospi[3], bf0[48], cos_bit);
    bf1[49] = half_btf(cospi[29], bf0[46], cospi[35], bf0[49], cos_bit);
    bf1[50] = half_btf(cospi[45], bf0[45], cospi[19], bf0[50], cos_bit);
    bf1[51] = half_btf(cospi[13], bf0[44], cospi[51], bf0[51], cos_bit);
    bf1[52] = half_btf(cospi[53], bf0[43], cospi[11], bf0[52], cos_bit);
    bf1[53] = half_btf(cospi[21], bf0[42], cospi[43], bf0[53], cos_bit);
    bf1[54] = half_btf(cospi[37], bf0[41], cospi[27], bf0[54], cos_bit);
    bf1[55] = half_btf(cospi[5], bf0[40], cospi[59], bf0[55], cos_bit);
    bf1[56] = half_btf(cospi[57], bf0[39], cospi[7], bf0[56], cos_bit);
    bf1[57] = half_btf(cospi[25], bf0[38], cospi[39], bf0[57], cos_bit);
    bf1[58] = half_btf(cospi[41], bf0[37], cospi[23], bf0[58], cos_bit);
    bf1[59] = half_btf(cospi[9], bf0[36], cospi[55], bf0[59], cos_bit);
    bf1[60] = half_btf(cospi[49], bf0[35], cospi[15], bf0[60], cos_bit);
    bf1[61] = half_btf(cospi[17], bf0[34], cospi[47], bf0[61], cos_bit);
    bf1[62] = half_btf(cospi[33], bf0[33], cospi[31], bf0[62], cos_bit);
    bf1[63] = half_btf(cospi[1], bf0[32], cospi[63], bf0[63], cos_bit);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 3
    stage++;
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
    bf1[16] = half_btf(cospi[62], bf0[16], -cospi[2], bf0[31], cos_bit);
    bf1[17] = half_btf(cospi[30], bf0[17], -cospi[34], bf0[30], cos_bit);
    bf1[18] = half_btf(cospi[46], bf0[18], -cospi[18], bf0[29], cos_bit);
    bf1[19] = half_btf(cospi[14], bf0[19], -cospi[50], bf0[28], cos_bit);
    bf1[20] = half_btf(cospi[54], bf0[20], -cospi[10], bf0[27], cos_bit);
    bf1[21] = half_btf(cospi[22], bf0[21], -cospi[42], bf0[26], cos_bit);
    bf1[22] = half_btf(cospi[38], bf0[22], -cospi[26], bf0[25], cos_bit);
    bf1[23] = half_btf(cospi[6], bf0[23], -cospi[58], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[58], bf0[23], cospi[6], bf0[24], cos_bit);
    bf1[25] = half_btf(cospi[26], bf0[22], cospi[38], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[42], bf0[21], cospi[22], bf0[26], cos_bit);
    bf1[27] = half_btf(cospi[10], bf0[20], cospi[54], bf0[27], cos_bit);
    bf1[28] = half_btf(cospi[50], bf0[19], cospi[14], bf0[28], cos_bit);
    bf1[29] = half_btf(cospi[18], bf0[18], cospi[46], bf0[29], cos_bit);
    bf1[30] = half_btf(cospi[34], bf0[17], cospi[30], bf0[30], cos_bit);
    bf1[31] = half_btf(cospi[2], bf0[16], cospi[62], bf0[31], cos_bit);
    bf1[32] = clamp_value(bf0[32] + bf0[33], stage_range[stage]);
    bf1[33] = clamp_value(bf0[32] - bf0[33], stage_range[stage]);
    bf1[34] = clamp_value(-bf0[34] + bf0[35], stage_range[stage]);
    bf1[35] = clamp_value(bf0[34] + bf0[35], stage_range[stage]);
    bf1[36] = clamp_value(bf0[36] + bf0[37], stage_range[stage]);
    bf1[37] = clamp_value(bf0[36] - bf0[37], stage_range[stage]);
    bf1[38] = clamp_value(-bf0[38] + bf0[39], stage_range[stage]);
    bf1[39] = clamp_value(bf0[38] + bf0[39], stage_range[stage]);
    bf1[40] = clamp_value(bf0[40] + bf0[41], stage_range[stage]);
    bf1[41] = clamp_value(bf0[40] - bf0[41], stage_range[stage]);
    bf1[42] = clamp_value(-bf0[42] + bf0[43], stage_range[stage]);
    bf1[43] = clamp_value(bf0[42] + bf0[43], stage_range[stage]);
    bf1[44] = clamp_value(bf0[44] + bf0[45], stage_range[stage]);
    bf1[45] = clamp_value(bf0[44] - bf0[45], stage_range[stage]);
    bf1[46] = clamp_value(-bf0[46] + bf0[47], stage_range[stage]);
    bf1[47] = clamp_value(bf0[46] + bf0[47], stage_range[stage]);
    bf1[48] = clamp_value(bf0[48] + bf0[49], stage_range[stage]);
    bf1[49] = clamp_value(bf0[48] - bf0[49], stage_range[stage]);
    bf1[50] = clamp_value(-bf0[50] + bf0[51], stage_range[stage]);
    bf1[51] = clamp_value(bf0[50] + bf0[51], stage_range[stage]);
    bf1[52] = clamp_value(bf0[52] + bf0[53], stage_range[stage]);
    bf1[53] = clamp_value(bf0[52] - bf0[53], stage_range[stage]);
    bf1[54] = clamp_value(-bf0[54] + bf0[55], stage_range[stage]);
    bf1[55] = clamp_value(bf0[54] + bf0[55], stage_range[stage]);
    bf1[56] = clamp_value(bf0[56] + bf0[57], stage_range[stage]);
    bf1[57] = clamp_value(bf0[56] - bf0[57], stage_range[stage]);
    bf1[58] = clamp_value(-bf0[58] + bf0[59], stage_range[stage]);
    bf1[59] = clamp_value(bf0[58] + bf0[59], stage_range[stage]);
    bf1[60] = clamp_value(bf0[60] + bf0[61], stage_range[stage]);
    bf1[61] = clamp_value(bf0[60] - bf0[61], stage_range[stage]);
    bf1[62] = clamp_value(-bf0[62] + bf0[63], stage_range[stage]);
    bf1[63] = clamp_value(bf0[62] + bf0[63], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 4
    stage++;
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
    bf1[8]  = half_btf(cospi[60], bf0[8], -cospi[4], bf0[15], cos_bit);
    bf1[9]  = half_btf(cospi[28], bf0[9], -cospi[36], bf0[14], cos_bit);
    bf1[10] = half_btf(cospi[44], bf0[10], -cospi[20], bf0[13], cos_bit);
    bf1[11] = half_btf(cospi[12], bf0[11], -cospi[52], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[52], bf0[11], cospi[12], bf0[12], cos_bit);
    bf1[13] = half_btf(cospi[20], bf0[10], cospi[44], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[36], bf0[9], cospi[28], bf0[14], cos_bit);
    bf1[15] = half_btf(cospi[4], bf0[8], cospi[60], bf0[15], cos_bit);
    bf1[16] = clamp_value(bf0[16] + bf0[17], stage_range[stage]);
    bf1[17] = clamp_value(bf0[16] - bf0[17], stage_range[stage]);
    bf1[18] = clamp_value(-bf0[18] + bf0[19], stage_range[stage]);
    bf1[19] = clamp_value(bf0[18] + bf0[19], stage_range[stage]);
    bf1[20] = clamp_value(bf0[20] + bf0[21], stage_range[stage]);
    bf1[21] = clamp_value(bf0[20] - bf0[21], stage_range[stage]);
    bf1[22] = clamp_value(-bf0[22] + bf0[23], stage_range[stage]);
    bf1[23] = clamp_value(bf0[22] + bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(bf0[24] + bf0[25], stage_range[stage]);
    bf1[25] = clamp_value(bf0[24] - bf0[25], stage_range[stage]);
    bf1[26] = clamp_value(-bf0[26] + bf0[27], stage_range[stage]);
    bf1[27] = clamp_value(bf0[26] + bf0[27], stage_range[stage]);
    bf1[28] = clamp_value(bf0[28] + bf0[29], stage_range[stage]);
    bf1[29] = clamp_value(bf0[28] - bf0[29], stage_range[stage]);
    bf1[30] = clamp_value(-bf0[30] + bf0[31], stage_range[stage]);
    bf1[31] = clamp_value(bf0[30] + bf0[31], stage_range[stage]);
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
    bf1[49] = half_btf(-cospi[52], bf0[46], cospi[12], bf0[49], cos_bit);
    bf1[50] = half_btf(cospi[12], bf0[45], cospi[52], bf0[50], cos_bit);
    bf1[51] = bf0[51];
    bf1[52] = bf0[52];
    bf1[53] = half_btf(-cospi[20], bf0[42], cospi[44], bf0[53], cos_bit);
    bf1[54] = half_btf(cospi[44], bf0[41], cospi[20], bf0[54], cos_bit);
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = half_btf(-cospi[36], bf0[38], cospi[28], bf0[57], cos_bit);
    bf1[58] = half_btf(cospi[28], bf0[37], cospi[36], bf0[58], cos_bit);
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = half_btf(-cospi[4], bf0[34], cospi[60], bf0[61], cos_bit);
    bf1[62] = half_btf(cospi[60], bf0[33], cospi[4], bf0[62], cos_bit);
    bf1[63] = bf0[63];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 5
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = bf0[0];
    bf1[1]  = bf0[1];
    bf1[2]  = bf0[2];
    bf1[3]  = bf0[3];
    bf1[4]  = half_btf(cospi[56], bf0[4], -cospi[8], bf0[7], cos_bit);
    bf1[5]  = half_btf(cospi[24], bf0[5], -cospi[40], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[40], bf0[5], cospi[24], bf0[6], cos_bit);
    bf1[7]  = half_btf(cospi[8], bf0[4], cospi[56], bf0[7], cos_bit);
    bf1[8]  = clamp_value(bf0[8] + bf0[9], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[8] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(-bf0[10] + bf0[11], stage_range[stage]);
    bf1[11] = clamp_value(bf0[10] + bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[13], stage_range[stage]);
    bf1[13] = clamp_value(bf0[12] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(-bf0[14] + bf0[15], stage_range[stage]);
    bf1[15] = clamp_value(bf0[14] + bf0[15], stage_range[stage]);
    bf1[16] = bf0[16];
    bf1[17] = half_btf(-cospi[8], bf0[17], cospi[56], bf0[30], cos_bit);
    bf1[18] = half_btf(-cospi[56], bf0[18], -cospi[8], bf0[29], cos_bit);
    bf1[19] = bf0[19];
    bf1[20] = bf0[20];
    bf1[21] = half_btf(-cospi[40], bf0[21], cospi[24], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[24], bf0[22], -cospi[40], bf0[25], cos_bit);
    bf1[23] = bf0[23];
    bf1[24] = bf0[24];
    bf1[25] = half_btf(-cospi[40], bf0[22], cospi[24], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[24], bf0[21], cospi[40], bf0[26], cos_bit);
    bf1[27] = bf0[27];
    bf1[28] = bf0[28];
    bf1[29] = half_btf(-cospi[8], bf0[18], cospi[56], bf0[29], cos_bit);
    bf1[30] = half_btf(cospi[56], bf0[17], cospi[8], bf0[30], cos_bit);
    bf1[31] = bf0[31];
    bf1[32] = clamp_value(bf0[32] + bf0[35], stage_range[stage]);
    bf1[33] = clamp_value(bf0[33] + bf0[34], stage_range[stage]);
    bf1[34] = clamp_value(bf0[33] - bf0[34], stage_range[stage]);
    bf1[35] = clamp_value(bf0[32] - bf0[35], stage_range[stage]);
    bf1[36] = clamp_value(-bf0[36] + bf0[39], stage_range[stage]);
    bf1[37] = clamp_value(-bf0[37] + bf0[38], stage_range[stage]);
    bf1[38] = clamp_value(bf0[37] + bf0[38], stage_range[stage]);
    bf1[39] = clamp_value(bf0[36] + bf0[39], stage_range[stage]);
    bf1[40] = clamp_value(bf0[40] + bf0[43], stage_range[stage]);
    bf1[41] = clamp_value(bf0[41] + bf0[42], stage_range[stage]);
    bf1[42] = clamp_value(bf0[41] - bf0[42], stage_range[stage]);
    bf1[43] = clamp_value(bf0[40] - bf0[43], stage_range[stage]);
    bf1[44] = clamp_value(-bf0[44] + bf0[47], stage_range[stage]);
    bf1[45] = clamp_value(-bf0[45] + bf0[46], stage_range[stage]);
    bf1[46] = clamp_value(bf0[45] + bf0[46], stage_range[stage]);
    bf1[47] = clamp_value(bf0[44] + bf0[47], stage_range[stage]);
    bf1[48] = clamp_value(bf0[48] + bf0[51], stage_range[stage]);
    bf1[49] = clamp_value(bf0[49] + bf0[50], stage_range[stage]);
    bf1[50] = clamp_value(bf0[49] - bf0[50], stage_range[stage]);
    bf1[51] = clamp_value(bf0[48] - bf0[51], stage_range[stage]);
    bf1[52] = clamp_value(-bf0[52] + bf0[55], stage_range[stage]);
    bf1[53] = clamp_value(-bf0[53] + bf0[54], stage_range[stage]);
    bf1[54] = clamp_value(bf0[53] + bf0[54], stage_range[stage]);
    bf1[55] = clamp_value(bf0[52] + bf0[55], stage_range[stage]);
    bf1[56] = clamp_value(bf0[56] + bf0[59], stage_range[stage]);
    bf1[57] = clamp_value(bf0[57] + bf0[58], stage_range[stage]);
    bf1[58] = clamp_value(bf0[57] - bf0[58], stage_range[stage]);
    bf1[59] = clamp_value(bf0[56] - bf0[59], stage_range[stage]);
    bf1[60] = clamp_value(-bf0[60] + bf0[63], stage_range[stage]);
    bf1[61] = clamp_value(-bf0[61] + bf0[62], stage_range[stage]);
    bf1[62] = clamp_value(bf0[61] + bf0[62], stage_range[stage]);
    bf1[63] = clamp_value(bf0[60] + bf0[63], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 6
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = half_btf(cospi[32], bf0[0], cospi[32], bf0[1], cos_bit);
    bf1[1]  = half_btf(cospi[32], bf0[0], -cospi[32], bf0[1], cos_bit);
    bf1[2]  = half_btf(cospi[48], bf0[2], -cospi[16], bf0[3], cos_bit);
    bf1[3]  = half_btf(cospi[16], bf0[2], cospi[48], bf0[3], cos_bit);
    bf1[4]  = clamp_value(bf0[4] + bf0[5], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[4] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(-bf0[6] + bf0[7], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[6] + bf0[7], stage_range[stage]);
    bf1[8]  = bf0[8];
    bf1[9]  = half_btf(-cospi[16], bf0[9], cospi[48], bf0[14], cos_bit);
    bf1[10] = half_btf(-cospi[48], bf0[10], -cospi[16], bf0[13], cos_bit);
    bf1[11] = bf0[11];
    bf1[12] = bf0[12];
    bf1[13] = half_btf(-cospi[16], bf0[10], cospi[48], bf0[13], cos_bit);
    bf1[14] = half_btf(cospi[48], bf0[9], cospi[16], bf0[14], cos_bit);
    bf1[15] = bf0[15];
    bf1[16] = clamp_value(bf0[16] + bf0[19], stage_range[stage]);
    bf1[17] = clamp_value(bf0[17] + bf0[18], stage_range[stage]);
    bf1[18] = clamp_value(bf0[17] - bf0[18], stage_range[stage]);
    bf1[19] = clamp_value(bf0[16] - bf0[19], stage_range[stage]);
    bf1[20] = clamp_value(-bf0[20] + bf0[23], stage_range[stage]);
    bf1[21] = clamp_value(-bf0[21] + bf0[22], stage_range[stage]);
    bf1[22] = clamp_value(bf0[21] + bf0[22], stage_range[stage]);
    bf1[23] = clamp_value(bf0[20] + bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(bf0[24] + bf0[27], stage_range[stage]);
    bf1[25] = clamp_value(bf0[25] + bf0[26], stage_range[stage]);
    bf1[26] = clamp_value(bf0[25] - bf0[26], stage_range[stage]);
    bf1[27] = clamp_value(bf0[24] - bf0[27], stage_range[stage]);
    bf1[28] = clamp_value(-bf0[28] + bf0[31], stage_range[stage]);
    bf1[29] = clamp_value(-bf0[29] + bf0[30], stage_range[stage]);
    bf1[30] = clamp_value(bf0[29] + bf0[30], stage_range[stage]);
    bf1[31] = clamp_value(bf0[28] + bf0[31], stage_range[stage]);
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
    bf1[50] = half_btf(-cospi[40], bf0[45], cospi[24], bf0[50], cos_bit);
    bf1[51] = half_btf(-cospi[40], bf0[44], cospi[24], bf0[51], cos_bit);
    bf1[52] = half_btf(cospi[24], bf0[43], cospi[40], bf0[52], cos_bit);
    bf1[53] = half_btf(cospi[24], bf0[42], cospi[40], bf0[53], cos_bit);
    bf1[54] = bf0[54];
    bf1[55] = bf0[55];
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = half_btf(-cospi[8], bf0[37], cospi[56], bf0[58], cos_bit);
    bf1[59] = half_btf(-cospi[8], bf0[36], cospi[56], bf0[59], cos_bit);
    bf1[60] = half_btf(cospi[56], bf0[35], cospi[8], bf0[60], cos_bit);
    bf1[61] = half_btf(cospi[56], bf0[34], cospi[8], bf0[61], cos_bit);
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 7
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[3], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[2], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[1] - bf0[2], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[0] - bf0[3], stage_range[stage]);
    bf1[4]  = bf0[4];
    bf1[5]  = half_btf(-cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[6]  = half_btf(cospi[32], bf0[5], cospi[32], bf0[6], cos_bit);
    bf1[7]  = bf0[7];
    bf1[8]  = clamp_value(bf0[8] + bf0[11], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[10], stage_range[stage]);
    bf1[10] = clamp_value(bf0[9] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[8] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(-bf0[12] + bf0[15], stage_range[stage]);
    bf1[13] = clamp_value(-bf0[13] + bf0[14], stage_range[stage]);
    bf1[14] = clamp_value(bf0[13] + bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[12] + bf0[15], stage_range[stage]);
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
    bf1[26] = half_btf(-cospi[16], bf0[21], cospi[48], bf0[26], cos_bit);
    bf1[27] = half_btf(-cospi[16], bf0[20], cospi[48], bf0[27], cos_bit);
    bf1[28] = half_btf(cospi[48], bf0[19], cospi[16], bf0[28], cos_bit);
    bf1[29] = half_btf(cospi[48], bf0[18], cospi[16], bf0[29], cos_bit);
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = clamp_value(bf0[32] + bf0[39], stage_range[stage]);
    bf1[33] = clamp_value(bf0[33] + bf0[38], stage_range[stage]);
    bf1[34] = clamp_value(bf0[34] + bf0[37], stage_range[stage]);
    bf1[35] = clamp_value(bf0[35] + bf0[36], stage_range[stage]);
    bf1[36] = clamp_value(bf0[35] - bf0[36], stage_range[stage]);
    bf1[37] = clamp_value(bf0[34] - bf0[37], stage_range[stage]);
    bf1[38] = clamp_value(bf0[33] - bf0[38], stage_range[stage]);
    bf1[39] = clamp_value(bf0[32] - bf0[39], stage_range[stage]);
    bf1[40] = clamp_value(-bf0[40] + bf0[47], stage_range[stage]);
    bf1[41] = clamp_value(-bf0[41] + bf0[46], stage_range[stage]);
    bf1[42] = clamp_value(-bf0[42] + bf0[45], stage_range[stage]);
    bf1[43] = clamp_value(-bf0[43] + bf0[44], stage_range[stage]);
    bf1[44] = clamp_value(bf0[43] + bf0[44], stage_range[stage]);
    bf1[45] = clamp_value(bf0[42] + bf0[45], stage_range[stage]);
    bf1[46] = clamp_value(bf0[41] + bf0[46], stage_range[stage]);
    bf1[47] = clamp_value(bf0[40] + bf0[47], stage_range[stage]);
    bf1[48] = clamp_value(bf0[48] + bf0[55], stage_range[stage]);
    bf1[49] = clamp_value(bf0[49] + bf0[54], stage_range[stage]);
    bf1[50] = clamp_value(bf0[50] + bf0[53], stage_range[stage]);
    bf1[51] = clamp_value(bf0[51] + bf0[52], stage_range[stage]);
    bf1[52] = clamp_value(bf0[51] - bf0[52], stage_range[stage]);
    bf1[53] = clamp_value(bf0[50] - bf0[53], stage_range[stage]);
    bf1[54] = clamp_value(bf0[49] - bf0[54], stage_range[stage]);
    bf1[55] = clamp_value(bf0[48] - bf0[55], stage_range[stage]);
    bf1[56] = clamp_value(-bf0[56] + bf0[63], stage_range[stage]);
    bf1[57] = clamp_value(-bf0[57] + bf0[62], stage_range[stage]);
    bf1[58] = clamp_value(-bf0[58] + bf0[61], stage_range[stage]);
    bf1[59] = clamp_value(-bf0[59] + bf0[60], stage_range[stage]);
    bf1[60] = clamp_value(bf0[59] + bf0[60], stage_range[stage]);
    bf1[61] = clamp_value(bf0[58] + bf0[61], stage_range[stage]);
    bf1[62] = clamp_value(bf0[57] + bf0[62], stage_range[stage]);
    bf1[63] = clamp_value(bf0[56] + bf0[63], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 8
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = clamp_value(bf0[0] + bf0[7], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[6], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[5], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[4], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[3] - bf0[4], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[2] - bf0[5], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[1] - bf0[6], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[0] - bf0[7], stage_range[stage]);
    bf1[8]  = bf0[8];
    bf1[9]  = bf0[9];
    bf1[10] = half_btf(-cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[11] = half_btf(-cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[12] = half_btf(cospi[32], bf0[11], cospi[32], bf0[12], cos_bit);
    bf1[13] = half_btf(cospi[32], bf0[10], cospi[32], bf0[13], cos_bit);
    bf1[14] = bf0[14];
    bf1[15] = bf0[15];
    bf1[16] = clamp_value(bf0[16] + bf0[23], stage_range[stage]);
    bf1[17] = clamp_value(bf0[17] + bf0[22], stage_range[stage]);
    bf1[18] = clamp_value(bf0[18] + bf0[21], stage_range[stage]);
    bf1[19] = clamp_value(bf0[19] + bf0[20], stage_range[stage]);
    bf1[20] = clamp_value(bf0[19] - bf0[20], stage_range[stage]);
    bf1[21] = clamp_value(bf0[18] - bf0[21], stage_range[stage]);
    bf1[22] = clamp_value(bf0[17] - bf0[22], stage_range[stage]);
    bf1[23] = clamp_value(bf0[16] - bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(-bf0[24] + bf0[31], stage_range[stage]);
    bf1[25] = clamp_value(-bf0[25] + bf0[30], stage_range[stage]);
    bf1[26] = clamp_value(-bf0[26] + bf0[29], stage_range[stage]);
    bf1[27] = clamp_value(-bf0[27] + bf0[28], stage_range[stage]);
    bf1[28] = clamp_value(bf0[27] + bf0[28], stage_range[stage]);
    bf1[29] = clamp_value(bf0[26] + bf0[29], stage_range[stage]);
    bf1[30] = clamp_value(bf0[25] + bf0[30], stage_range[stage]);
    bf1[31] = clamp_value(bf0[24] + bf0[31], stage_range[stage]);
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
    bf1[52] = half_btf(-cospi[16], bf0[43], cospi[48], bf0[52], cos_bit);
    bf1[53] = half_btf(-cospi[16], bf0[42], cospi[48], bf0[53], cos_bit);
    bf1[54] = half_btf(-cospi[16], bf0[41], cospi[48], bf0[54], cos_bit);
    bf1[55] = half_btf(-cospi[16], bf0[40], cospi[48], bf0[55], cos_bit);
    bf1[56] = half_btf(cospi[48], bf0[39], cospi[16], bf0[56], cos_bit);
    bf1[57] = half_btf(cospi[48], bf0[38], cospi[16], bf0[57], cos_bit);
    bf1[58] = half_btf(cospi[48], bf0[37], cospi[16], bf0[58], cos_bit);
    bf1[59] = half_btf(cospi[48], bf0[36], cospi[16], bf0[59], cos_bit);
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 9
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[15], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[14], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[13], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[12], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[11], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[10], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[9], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[8], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[7] - bf0[8], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[6] - bf0[9], stage_range[stage]);
    bf1[10] = clamp_value(bf0[5] - bf0[10], stage_range[stage]);
    bf1[11] = clamp_value(bf0[4] - bf0[11], stage_range[stage]);
    bf1[12] = clamp_value(bf0[3] - bf0[12], stage_range[stage]);
    bf1[13] = clamp_value(bf0[2] - bf0[13], stage_range[stage]);
    bf1[14] = clamp_value(bf0[1] - bf0[14], stage_range[stage]);
    bf1[15] = clamp_value(bf0[0] - bf0[15], stage_range[stage]);
    bf1[16] = bf0[16];
    bf1[17] = bf0[17];
    bf1[18] = bf0[18];
    bf1[19] = bf0[19];
    bf1[20] = half_btf(-cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[21] = half_btf(-cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[22] = half_btf(-cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[23] = half_btf(-cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[24] = half_btf(cospi[32], bf0[23], cospi[32], bf0[24], cos_bit);
    bf1[25] = half_btf(cospi[32], bf0[22], cospi[32], bf0[25], cos_bit);
    bf1[26] = half_btf(cospi[32], bf0[21], cospi[32], bf0[26], cos_bit);
    bf1[27] = half_btf(cospi[32], bf0[20], cospi[32], bf0[27], cos_bit);
    bf1[28] = bf0[28];
    bf1[29] = bf0[29];
    bf1[30] = bf0[30];
    bf1[31] = bf0[31];
    bf1[32] = clamp_value(bf0[32] + bf0[47], stage_range[stage]);
    bf1[33] = clamp_value(bf0[33] + bf0[46], stage_range[stage]);
    bf1[34] = clamp_value(bf0[34] + bf0[45], stage_range[stage]);
    bf1[35] = clamp_value(bf0[35] + bf0[44], stage_range[stage]);
    bf1[36] = clamp_value(bf0[36] + bf0[43], stage_range[stage]);
    bf1[37] = clamp_value(bf0[37] + bf0[42], stage_range[stage]);
    bf1[38] = clamp_value(bf0[38] + bf0[41], stage_range[stage]);
    bf1[39] = clamp_value(bf0[39] + bf0[40], stage_range[stage]);
    bf1[40] = clamp_value(bf0[39] - bf0[40], stage_range[stage]);
    bf1[41] = clamp_value(bf0[38] - bf0[41], stage_range[stage]);
    bf1[42] = clamp_value(bf0[37] - bf0[42], stage_range[stage]);
    bf1[43] = clamp_value(bf0[36] - bf0[43], stage_range[stage]);
    bf1[44] = clamp_value(bf0[35] - bf0[44], stage_range[stage]);
    bf1[45] = clamp_value(bf0[34] - bf0[45], stage_range[stage]);
    bf1[46] = clamp_value(bf0[33] - bf0[46], stage_range[stage]);
    bf1[47] = clamp_value(bf0[32] - bf0[47], stage_range[stage]);
    bf1[48] = clamp_value(-bf0[48] + bf0[63], stage_range[stage]);
    bf1[49] = clamp_value(-bf0[49] + bf0[62], stage_range[stage]);
    bf1[50] = clamp_value(-bf0[50] + bf0[61], stage_range[stage]);
    bf1[51] = clamp_value(-bf0[51] + bf0[60], stage_range[stage]);
    bf1[52] = clamp_value(-bf0[52] + bf0[59], stage_range[stage]);
    bf1[53] = clamp_value(-bf0[53] + bf0[58], stage_range[stage]);
    bf1[54] = clamp_value(-bf0[54] + bf0[57], stage_range[stage]);
    bf1[55] = clamp_value(-bf0[55] + bf0[56], stage_range[stage]);
    bf1[56] = clamp_value(bf0[55] + bf0[56], stage_range[stage]);
    bf1[57] = clamp_value(bf0[54] + bf0[57], stage_range[stage]);
    bf1[58] = clamp_value(bf0[53] + bf0[58], stage_range[stage]);
    bf1[59] = clamp_value(bf0[52] + bf0[59], stage_range[stage]);
    bf1[60] = clamp_value(bf0[51] + bf0[60], stage_range[stage]);
    bf1[61] = clamp_value(bf0[50] + bf0[61], stage_range[stage]);
    bf1[62] = clamp_value(bf0[49] + bf0[62], stage_range[stage]);
    bf1[63] = clamp_value(bf0[48] + bf0[63], stage_range[stage]);
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 10
    stage++;
    bf0     = output;
    bf1     = step;
    bf1[0]  = clamp_value(bf0[0] + bf0[31], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[30], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[29], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[28], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[27], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[26], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[25], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[24], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[8] + bf0[23], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[22], stage_range[stage]);
    bf1[10] = clamp_value(bf0[10] + bf0[21], stage_range[stage]);
    bf1[11] = clamp_value(bf0[11] + bf0[20], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[19], stage_range[stage]);
    bf1[13] = clamp_value(bf0[13] + bf0[18], stage_range[stage]);
    bf1[14] = clamp_value(bf0[14] + bf0[17], stage_range[stage]);
    bf1[15] = clamp_value(bf0[15] + bf0[16], stage_range[stage]);
    bf1[16] = clamp_value(bf0[15] - bf0[16], stage_range[stage]);
    bf1[17] = clamp_value(bf0[14] - bf0[17], stage_range[stage]);
    bf1[18] = clamp_value(bf0[13] - bf0[18], stage_range[stage]);
    bf1[19] = clamp_value(bf0[12] - bf0[19], stage_range[stage]);
    bf1[20] = clamp_value(bf0[11] - bf0[20], stage_range[stage]);
    bf1[21] = clamp_value(bf0[10] - bf0[21], stage_range[stage]);
    bf1[22] = clamp_value(bf0[9] - bf0[22], stage_range[stage]);
    bf1[23] = clamp_value(bf0[8] - bf0[23], stage_range[stage]);
    bf1[24] = clamp_value(bf0[7] - bf0[24], stage_range[stage]);
    bf1[25] = clamp_value(bf0[6] - bf0[25], stage_range[stage]);
    bf1[26] = clamp_value(bf0[5] - bf0[26], stage_range[stage]);
    bf1[27] = clamp_value(bf0[4] - bf0[27], stage_range[stage]);
    bf1[28] = clamp_value(bf0[3] - bf0[28], stage_range[stage]);
    bf1[29] = clamp_value(bf0[2] - bf0[29], stage_range[stage]);
    bf1[30] = clamp_value(bf0[1] - bf0[30], stage_range[stage]);
    bf1[31] = clamp_value(bf0[0] - bf0[31], stage_range[stage]);
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
    bf1[48] = half_btf(cospi[32], bf0[47], cospi[32], bf0[48], cos_bit);
    bf1[49] = half_btf(cospi[32], bf0[46], cospi[32], bf0[49], cos_bit);
    bf1[50] = half_btf(cospi[32], bf0[45], cospi[32], bf0[50], cos_bit);
    bf1[51] = half_btf(cospi[32], bf0[44], cospi[32], bf0[51], cos_bit);
    bf1[52] = half_btf(cospi[32], bf0[43], cospi[32], bf0[52], cos_bit);
    bf1[53] = half_btf(cospi[32], bf0[42], cospi[32], bf0[53], cos_bit);
    bf1[54] = half_btf(cospi[32], bf0[41], cospi[32], bf0[54], cos_bit);
    bf1[55] = half_btf(cospi[32], bf0[40], cospi[32], bf0[55], cos_bit);
    bf1[56] = bf0[56];
    bf1[57] = bf0[57];
    bf1[58] = bf0[58];
    bf1[59] = bf0[59];
    bf1[60] = bf0[60];
    bf1[61] = bf0[61];
    bf1[62] = bf0[62];
    bf1[63] = bf0[63];
    //range_check_buf(stage, input, bf1, size, stage_range[stage]);

    // stage 11
    stage++;
    bf0     = step;
    bf1     = output;
    bf1[0]  = clamp_value(bf0[0] + bf0[63], stage_range[stage]);
    bf1[1]  = clamp_value(bf0[1] + bf0[62], stage_range[stage]);
    bf1[2]  = clamp_value(bf0[2] + bf0[61], stage_range[stage]);
    bf1[3]  = clamp_value(bf0[3] + bf0[60], stage_range[stage]);
    bf1[4]  = clamp_value(bf0[4] + bf0[59], stage_range[stage]);
    bf1[5]  = clamp_value(bf0[5] + bf0[58], stage_range[stage]);
    bf1[6]  = clamp_value(bf0[6] + bf0[57], stage_range[stage]);
    bf1[7]  = clamp_value(bf0[7] + bf0[56], stage_range[stage]);
    bf1[8]  = clamp_value(bf0[8] + bf0[55], stage_range[stage]);
    bf1[9]  = clamp_value(bf0[9] + bf0[54], stage_range[stage]);
    bf1[10] = clamp_value(bf0[10] + bf0[53], stage_range[stage]);
    bf1[11] = clamp_value(bf0[11] + bf0[52], stage_range[stage]);
    bf1[12] = clamp_value(bf0[12] + bf0[51], stage_range[stage]);
    bf1[13] = clamp_value(bf0[13] + bf0[50], stage_range[stage]);
    bf1[14] = clamp_value(bf0[14] + bf0[49], stage_range[stage]);
    bf1[15] = clamp_value(bf0[15] + bf0[48], stage_range[stage]);
    bf1[16] = clamp_value(bf0[16] + bf0[47], stage_range[stage]);
    bf1[17] = clamp_value(bf0[17] + bf0[46], stage_range[stage]);
    bf1[18] = clamp_value(bf0[18] + bf0[45], stage_range[stage]);
    bf1[19] = clamp_value(bf0[19] + bf0[44], stage_range[stage]);
    bf1[20] = clamp_value(bf0[20] + bf0[43], stage_range[stage]);
    bf1[21] = clamp_value(bf0[21] + bf0[42], stage_range[stage]);
    bf1[22] = clamp_value(bf0[22] + bf0[41], stage_range[stage]);
    bf1[23] = clamp_value(bf0[23] + bf0[40], stage_range[stage]);
    bf1[24] = clamp_value(bf0[24] + bf0[39], stage_range[stage]);
    bf1[25] = clamp_value(bf0[25] + bf0[38], stage_range[stage]);
    bf1[26] = clamp_value(bf0[26] + bf0[37], stage_range[stage]);
    bf1[27] = clamp_value(bf0[27] + bf0[36], stage_range[stage]);
    bf1[28] = clamp_value(bf0[28] + bf0[35], stage_range[stage]);
    bf1[29] = clamp_value(bf0[29] + bf0[34], stage_range[stage]);
    bf1[30] = clamp_value(bf0[30] + bf0[33], stage_range[stage]);
    bf1[31] = clamp_value(bf0[31] + bf0[32], stage_range[stage]);
    bf1[32] = clamp_value(bf0[31] - bf0[32], stage_range[stage]);
    bf1[33] = clamp_value(bf0[30] - bf0[33], stage_range[stage]);
    bf1[34] = clamp_value(bf0[29] - bf0[34], stage_range[stage]);
    bf1[35] = clamp_value(bf0[28] - bf0[35], stage_range[stage]);
    bf1[36] = clamp_value(bf0[27] - bf0[36], stage_range[stage]);
    bf1[37] = clamp_value(bf0[26] - bf0[37], stage_range[stage]);
    bf1[38] = clamp_value(bf0[25] - bf0[38], stage_range[stage]);
    bf1[39] = clamp_value(bf0[24] - bf0[39], stage_range[stage]);
    bf1[40] = clamp_value(bf0[23] - bf0[40], stage_range[stage]);
    bf1[41] = clamp_value(bf0[22] - bf0[41], stage_range[stage]);
    bf1[42] = clamp_value(bf0[21] - bf0[42], stage_range[stage]);
    bf1[43] = clamp_value(bf0[20] - bf0[43], stage_range[stage]);
    bf1[44] = clamp_value(bf0[19] - bf0[44], stage_range[stage]);
    bf1[45] = clamp_value(bf0[18] - bf0[45], stage_range[stage]);
    bf1[46] = clamp_value(bf0[17] - bf0[46], stage_range[stage]);
    bf1[47] = clamp_value(bf0[16] - bf0[47], stage_range[stage]);
    bf1[48] = clamp_value(bf0[15] - bf0[48], stage_range[stage]);
    bf1[49] = clamp_value(bf0[14] - bf0[49], stage_range[stage]);
    bf1[50] = clamp_value(bf0[13] - bf0[50], stage_range[stage]);
    bf1[51] = clamp_value(bf0[12] - bf0[51], stage_range[stage]);
    bf1[52] = clamp_value(bf0[11] - bf0[52], stage_range[stage]);
    bf1[53] = clamp_value(bf0[10] - bf0[53], stage_range[stage]);
    bf1[54] = clamp_value(bf0[9] - bf0[54], stage_range[stage]);
    bf1[55] = clamp_value(bf0[8] - bf0[55], stage_range[stage]);
    bf1[56] = clamp_value(bf0[7] - bf0[56], stage_range[stage]);
    bf1[57] = clamp_value(bf0[6] - bf0[57], stage_range[stage]);
    bf1[58] = clamp_value(bf0[5] - bf0[58], stage_range[stage]);
    bf1[59] = clamp_value(bf0[4] - bf0[59], stage_range[stage]);
    bf1[60] = clamp_value(bf0[3] - bf0[60], stage_range[stage]);
    bf1[61] = clamp_value(bf0[2] - bf0[61], stage_range[stage]);
    bf1[62] = clamp_value(bf0[1] - bf0[62], stage_range[stage]);
    bf1[63] = clamp_value(bf0[0] - bf0[63], stage_range[stage]);
}

void svt_av1_iidentity4_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    for (int32_t i = 0; i < 4; ++i) {
        // Normal input should fit into 32-bit. Cast to 64-bit here to avoid
        // overflow with corrupted/fuzzed input. The same for av1_iidentity/16/64_c.
        output[i] = round_shift((int64_t)new_sqrt2 * input[i], new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_iidentity8_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    for (int32_t i = 0; i < 8; ++i) {
        output[i] = (int32_t)((int64_t)input[i] * 2);
    }
}

void svt_av1_iidentity16_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    for (int32_t i = 0; i < 16; ++i) {
        output[i] = round_shift((int64_t)new_sqrt2 * 2 * input[i], new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

void svt_av1_iidentity32_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    for (int32_t i = 0; i < 32; ++i) {
        output[i] = (int32_t)((int64_t)input[i] * 4);
    }
}

void av1_iidentity64_c(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range) {
    (void)cos_bit;
    (void)stage_range;
    for (int32_t i = 0; i < 64; ++i) {
        output[i] = round_shift((int64_t)new_sqrt2 * 4 * input[i], new_sqrt2_bits);
    }
    assert(stage_range[0] + new_sqrt2_bits <= 32);
}

TxfmFunc svt_aom_inv_txfm_type_to_func(TxfmType txfmtype) {
    switch (txfmtype) {
    case TXFM_TYPE_DCT4:
        return svt_av1_idct4_new;
    case TXFM_TYPE_DCT8:
        return svt_av1_idct8_new;
    case TXFM_TYPE_DCT16:
        return svt_av1_idct16_new;
    case TXFM_TYPE_DCT32:
        return svt_av1_idct32_new;
    case TXFM_TYPE_DCT64:
        return svt_av1_idct64_new;
    case TXFM_TYPE_ADST4:
        return svt_av1_iadst4_new;
    case TXFM_TYPE_ADST8:
        return svt_av1_iadst8_new;
    case TXFM_TYPE_ADST16:
        return svt_av1_iadst16_new;
    case TXFM_TYPE_ADST32:
        return av1_iadst32_new;
    case TXFM_TYPE_IDENTITY4:
        return svt_av1_iidentity4_c;
    case TXFM_TYPE_IDENTITY8:
        return svt_av1_iidentity8_c;
    case TXFM_TYPE_IDENTITY16:
        return svt_av1_iidentity16_c;
    case TXFM_TYPE_IDENTITY32:
        return svt_av1_iidentity32_c;
    case TXFM_TYPE_IDENTITY64:
        return av1_iidentity64_c;
    default:
        assert(0);
        return NULL;
    }
}

static INLINE TranHigh check_range(TranHigh input, int32_t bd) {
    // AV1 TX case
    // - 8 bit: signed 16 bit integer
    // - 10 bit: signed 18 bit integer
    // - 12 bit: signed 20 bit integer
    // - max quantization error = 1828 << (bd - 8)
    const int32_t int_max = (1 << (7 + bd)) - 1 + (914 << (bd - 7));
    const int32_t int_min = -int_max - 1;
#if CONFIG_COEFFICIENT_RANGE_CHECKING
    assert(int_min <= input);
    assert(input <= int_max);
#endif // CONFIG_COEFFICIENT_RANGE_CHECKING
    return (TranHigh)clamp64(input, int_min, int_max);
}

#define HIGHBD_WRAPLOW(x, bd) ((int32_t)check_range((x), bd))

static INLINE uint16_t highbd_clip_pixel_add(uint16_t dest, TranHigh trans, int32_t bd) {
    trans = HIGHBD_WRAPLOW(trans, bd);
    return clip_pixel_highbd(dest + (int32_t)trans, bd);
}

void svt_av1_round_shift_array_c(int32_t* arr, int32_t size, int32_t bit) {
    int32_t i;
    if (bit == 0) {
        return;
    } else {
        if (bit > 0) {
            for (i = 0; i < size; i++) {
                arr[i] = round_shift(arr[i], bit);
            }
        } else {
            for (i = 0; i < size; i++) {
                arr[i] = arr[i] * (1 << (-bit));
            }
        }
    }
}

static const int32_t* cast_to_int32(const TranLow* input) {
    return (const int32_t*)input;
}

void svt_av1_get_inv_txfm_cfg(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg) {
    assert(cfg != NULL);
    cfg->tx_size = tx_size;
    set_flip_cfg(tx_type, cfg);
    av1_zero(cfg->stage_range_col);
    av1_zero(cfg->stage_range_row);
    set_flip_cfg(tx_type, cfg);
    const TxType1D tx_type_1d_col = vtx_tab[tx_type];
    const TxType1D tx_type_1d_row = htx_tab[tx_type];
    cfg->shift                    = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx         = get_txw_idx(tx_size);
    const int32_t txh_idx         = get_txh_idx(tx_size);
    cfg->cos_bit_col              = inv_cos_bit_col[txw_idx][txh_idx];
    cfg->cos_bit_row              = inv_cos_bit_row[txw_idx][txh_idx];
    cfg->txfm_type_col            = av1_txfm_type_ls[txh_idx][tx_type_1d_col];
    if (cfg->txfm_type_col == TXFM_TYPE_ADST4) {
        svt_memcpy_c(cfg->stage_range_col, iadst4_range, sizeof(iadst4_range));
    }
    cfg->txfm_type_row = av1_txfm_type_ls[txw_idx][tx_type_1d_row];
    if (cfg->txfm_type_row == TXFM_TYPE_ADST4) {
        svt_memcpy_c(cfg->stage_range_row, iadst4_range, sizeof(iadst4_range));
    }
    cfg->stage_num_col = av1_txfm_stage_num_list[cfg->txfm_type_col];
    cfg->stage_num_row = av1_txfm_stage_num_list[cfg->txfm_type_row];
}

static INLINE void inv_txfm2d_add_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, Txfm2dFlipCfg* cfg, int32_t* txfm_buf, TxSize tx_size,
                                    int32_t bd) {
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
    int8_t        stage_range_row[MAX_TXFM_STAGE_NUM];
    int8_t        stage_range_col[MAX_TXFM_STAGE_NUM];
    assert(cfg->stage_num_row <= MAX_TXFM_STAGE_NUM);
    assert(cfg->stage_num_col <= MAX_TXFM_STAGE_NUM);
    svt_av1_gen_inv_stage_range(stage_range_col, stage_range_row, cfg, tx_size, bd);

    const int8_t   cos_bit_col   = cfg->cos_bit_col;
    const int8_t   cos_bit_row   = cfg->cos_bit_row;
    const TxfmFunc txfm_func_col = svt_aom_inv_txfm_type_to_func(cfg->txfm_type_col);
    const TxfmFunc txfm_func_row = svt_aom_inv_txfm_type_to_func(cfg->txfm_type_row);
    ASSERT(txfm_func_col);
    ASSERT(txfm_func_row);
    // txfm_buf's length is  txfm_size_row * txfm_size_col + 2 *
    // AOMMAX(txfm_size_row, txfm_size_col)
    // it is used for intermediate data buffering
    const int32_t buf_offset = AOMMAX(txfm_size_row, txfm_size_col);
    int32_t*      temp_in    = txfm_buf;
    int32_t*      temp_out   = temp_in + buf_offset;
    int32_t*      buf        = temp_out + buf_offset;
    int32_t*      buf_ptr    = buf;
    int32_t       c, r;

    // Rows
    for (r = 0; r < txfm_size_row; ++r) {
        if (abs(rect_type) == 1) {
            for (c = 0; c < txfm_size_col; ++c) {
                temp_in[c] = round_shift((int64_t)input[c] * new_inv_sqrt2, new_sqrt2_bits);
            }
            clamp_buf(temp_in, txfm_size_col, (int8_t)(bd + 8));
            txfm_func_row(temp_in, buf_ptr, cos_bit_row, stage_range_row);
        } else {
            for (c = 0; c < txfm_size_col; ++c) {
                temp_in[c] = input[c];
            }
            clamp_buf(temp_in, txfm_size_col, (int8_t)(bd + 8));
            txfm_func_row(temp_in, buf_ptr, cos_bit_row, stage_range_row);
        }
        svt_av1_round_shift_array_c(buf_ptr, txfm_size_col, -shift[0]);
        input += txfm_size_col;
        buf_ptr += txfm_size_col;
    }

    // Columns
    for (c = 0; c < txfm_size_col; ++c) {
        if (cfg->lr_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                temp_in[r] = buf[r * txfm_size_col + c];
            }
        } else {
            // flip left right
            for (r = 0; r < txfm_size_row; ++r) {
                temp_in[r] = buf[r * txfm_size_col + (txfm_size_col - c - 1)];
            }
        }
        clamp_buf(temp_in, txfm_size_row, (int8_t)(AOMMAX(bd + 6, 16)));
        txfm_func_col(temp_in, temp_out, cos_bit_col, stage_range_col);
        svt_av1_round_shift_array_c(temp_out, txfm_size_row, -shift[1]);
        if (cfg->ud_flip == 0) {
            for (r = 0; r < txfm_size_row; ++r) {
                output_w[r * stride_w + c] = highbd_clip_pixel_add(output_r[r * stride_r + c], temp_out[r], bd);
            }
        } else {
            // flip upside down
            for (r = 0; r < txfm_size_row; ++r) {
                output_w[r * stride_w + c] = highbd_clip_pixel_add(
                    output_r[r * stride_r + c], temp_out[txfm_size_row - r - 1], bd);
            }
        }
    }
}

static INLINE void inv_txfm2d_add_facade(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, int32_t* txfm_buf, TxType tx_type, TxSize tx_size,
                                         int32_t bd) {
    Txfm2dFlipCfg cfg;
    svt_av1_get_inv_txfm_cfg(tx_type, tx_size, &cfg);
    // Forward shift sum uses larger square size, to be consistent with what
    // svt_av1_gen_inv_stage_range() does for inverse shifts.
    inv_txfm2d_add_c(input, output_r, stride_r, output_w, stride_w, &cfg, txfm_buf, tx_size, bd);
}

void svt_av1_inv_txfm2d_add_4x4_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                  int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, txfm_buf[4 * 4 + 4 + 4]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_4X4, bd);
}

void svt_av1_inv_txfm2d_add_8x8_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                  int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, txfm_buf[8 * 8 + 8 + 8]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_8X8, bd);
}

void svt_av1_inv_txfm2d_add_16x16_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, txfm_buf[16 * 16 + 16 + 16]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_16X16, bd);
}

void svt_av1_inv_txfm2d_add_32x32_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, txfm_buf[32 * 32 + 32 + 32]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_32X32, bd);
}

void svt_av1_inv_txfm2d_add_64x64_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, int32_t bd) {
    // Remap 32x32 input into a modified 64x64 by:
    // - Copying over these values in top-left 32x32 locations.
    // - Setting the rest of the locations to 0.
    int32_t mod_input[64 * 64];
    for (int32_t row = 0; row < 32; ++row) {
        svt_memcpy_c(mod_input + row * 64, input + row * 32, 32 * sizeof(*mod_input));
        memset(mod_input + row * 64 + 32, 0, 32 * sizeof(*mod_input));
    }
    memset(mod_input + 32 * 64, 0, 32 * 64 * sizeof(*mod_input));
    DECLARE_ALIGNED(32, int32_t, txfm_buf[64 * 64 + 64 + 64]);
    inv_txfm2d_add_facade(mod_input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_64X64, bd);
}

void svt_av1_inv_txfm2d_add_4x8_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                  int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    (void)tx_size;
    DECLARE_ALIGNED(32, int32_t, txfm_buf[4 * 8 + 8 + 8]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_4X8, bd);
}

void svt_av1_inv_txfm2d_add_8x4_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                  int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    (void)tx_size;
    DECLARE_ALIGNED(32, int32_t, txfm_buf[8 * 4 + 8 + 8]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_8X4, bd);
}

void svt_av1_inv_txfm2d_add_8x16_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[8 * 16 + 16 + 16]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_8X16, bd);
}

void svt_av1_inv_txfm2d_add_16x8_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[16 * 8 + 16 + 16]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_16X8, bd);
}

void svt_av1_inv_txfm2d_add_16x32_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[16 * 32 + 32 + 32]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_16X32, bd);
}

void svt_av1_inv_txfm2d_add_32x16_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[32 * 16 + 32 + 32]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_32X16, bd);
}

void svt_av1_inv_txfm2d_add_64x32_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    // Remap 32x32 input into a modified 64x32 by:
    // - Copying over these values in top-left 32x32 locations.
    // - Setting the rest of the locations to 0.
    int32_t mod_input[64 * 32];
    for (int32_t row = 0; row < 32; ++row) {
        svt_memcpy_c(mod_input + row * 64, input + row * 32, 32 * sizeof(*mod_input));
        memset(mod_input + row * 64 + 32, 0, 32 * sizeof(*mod_input));
    }
    DECLARE_ALIGNED(32, int32_t, txfm_buf[64 * 32 + 64 + 64]);
    inv_txfm2d_add_facade(mod_input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_64X32, bd);
}

void svt_av1_inv_txfm2d_add_32x64_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    // Remap 32x32 input into a modified 32x64 input by:
    // - Copying over these values in top-left 32x32 locations.
    // - Setting the rest of the locations to 0.
    int32_t mod_input[32 * 64];
    svt_memcpy_c(mod_input, input, 32 * 32 * sizeof(*mod_input));
    memset(mod_input + 32 * 32, 0, 32 * 32 * sizeof(*mod_input));
    DECLARE_ALIGNED(32, int32_t, txfm_buf[64 * 32 + 64 + 64]);
    inv_txfm2d_add_facade(mod_input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_32X64, bd);
}

void svt_av1_inv_txfm2d_add_16x64_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    // Remap 16x32 input into a modified 16x64 input by:
    // - Copying over these values in top-left 16x32 locations.
    // - Setting the rest of the locations to 0.
    int32_t mod_input[16 * 64];
    svt_memcpy_c(mod_input, input, 16 * 32 * sizeof(*mod_input));
    memset(mod_input + 16 * 32, 0, 16 * 32 * sizeof(*mod_input));
    DECLARE_ALIGNED(32, int32_t, txfm_buf[16 * 64 + 64 + 64]);
    inv_txfm2d_add_facade(mod_input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_16X64, bd);
}

void svt_av1_inv_txfm2d_add_64x16_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    // Remap 32x16 input into a modified 64x16 by:
    // - Copying over these values in top-left 32x16 locations.
    // - Setting the rest of the locations to 0.
    int32_t mod_input[64 * 16];
    for (int32_t row = 0; row < 16; ++row) {
        svt_memcpy_c(mod_input + row * 64, input + row * 32, 32 * sizeof(*mod_input));
        memset(mod_input + row * 64 + 32, 0, 32 * sizeof(*mod_input));
    }
    DECLARE_ALIGNED(32, int32_t, txfm_buf[16 * 64 + 64 + 64]);
    inv_txfm2d_add_facade(mod_input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_64X16, bd);
}

void svt_av1_inv_txfm2d_add_4x16_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    UNUSED(tx_size);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[4 * 16 + 16 + 16]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_4X16, bd);
}

void svt_av1_inv_txfm2d_add_16x4_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    UNUSED(tx_size);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[4 * 16 + 16 + 16]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_16X4, bd);
}

void svt_av1_inv_txfm2d_add_8x32_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[8 * 32 + 32 + 32]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_8X32, bd);
}

void svt_av1_inv_txfm2d_add_32x8_c(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    UNUSED(tx_size);
    UNUSED(eob);
    DECLARE_ALIGNED(32, int32_t, txfm_buf[8 * 32 + 32 + 32]);
    inv_txfm2d_add_facade(input, output_r, stride_r, output_w, stride_w, txfm_buf, tx_type, TX_32X8, bd);
}

static INLINE int32_t range_check_value(int32_t value, int8_t bit) {
#if CONFIG_COEFFICIENT_RANGE_CHECKING
    const int64_t max_value = (1LL << (bit - 1)) - 1;
    const int64_t min_value = -(1LL << (bit - 1));
    if (value < min_value || value > max_value) {
        SVT_ERROR("coeff out of bit range, value: %d bit %d\n", value, bit);
        assert(0);
    }
#endif // CONFIG_COEFFICIENT_RANGE_CHECKING
#if DO_RANGE_CHECK_CLAMP
    bit = AOMMIN(bit, 31);
    return clamp(value, (1 << (bit - 1)) - 1, -(1 << (bit - 1)));
#endif // DO_RANGE_CHECK_CLAMP
    (void)bit;
    return value;
}

void svt_av1_highbd_iwht4x4_16_add_c(const TranLow* input, uint8_t* dest8_r, int32_t stride_r, uint8_t* dest8_w,
                                     int32_t stride_w, int32_t bd) {
    /* 4-point reversible, orthonormal inverse Walsh-Hadamard in 3.5 adds,
       0.5 shifts per pixel. */
    int32_t        i;
    TranLow        output[16];
    TranLow        a1, b1, c1, d1, e1;
    const TranLow* ip     = input;
    TranLow*       op     = output;
    uint16_t*      dest_r = CONVERT_TO_SHORTPTR(dest8_r);
    uint16_t*      dest_w = CONVERT_TO_SHORTPTR(dest8_w);

    for (i = 0; i < 4; i++) {
        a1 = ip[0] >> UNIT_QUANT_SHIFT;
        c1 = ip[1] >> UNIT_QUANT_SHIFT;
        d1 = ip[2] >> UNIT_QUANT_SHIFT;
        b1 = ip[3] >> UNIT_QUANT_SHIFT;
        a1 += c1;
        d1 -= b1;
        e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= b1;
        d1 += c1;
        op[0] = a1;
        op[1] = b1;
        op[2] = c1;
        op[3] = d1;
        ip += 4;
        op += 4;
    }

    ip = output;
    for (i = 0; i < 4; i++) {
        a1 = ip[4 * 0];
        c1 = ip[4 * 1];
        d1 = ip[4 * 2];
        b1 = ip[4 * 3];
        a1 += c1;
        d1 -= b1;
        e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= b1;
        d1 += c1;
        range_check_value(a1, (int8_t)(bd + 1));
        range_check_value(b1, (int8_t)(bd + 1));
        range_check_value(c1, (int8_t)(bd + 1));
        range_check_value(d1, (int8_t)(bd + 1));

        dest_w[stride_w * 0] = highbd_clip_pixel_add(dest_r[stride_r * 0], a1, bd);
        dest_w[stride_w * 1] = highbd_clip_pixel_add(dest_r[stride_r * 1], b1, bd);
        dest_w[stride_w * 2] = highbd_clip_pixel_add(dest_r[stride_r * 2], c1, bd);
        dest_w[stride_w * 3] = highbd_clip_pixel_add(dest_r[stride_r * 3], d1, bd);

        ip++;
        dest_r++;
        dest_w++;
    }
}

void svt_av1_highbd_iwht4x4_1_add_c(const TranLow* in, uint8_t* dest8_r, int32_t dest_stride_r, uint8_t* dest8_w,
                                    int32_t dest_stride_w, int32_t bd) {
    int32_t        i;
    TranLow        a1, e1;
    TranLow        tmp[4];
    const TranLow* ip     = in;
    TranLow*       op     = tmp;
    uint16_t*      dest_r = CONVERT_TO_SHORTPTR(dest8_r);
    uint16_t*      dest_w = CONVERT_TO_SHORTPTR(dest8_w);
    (void)bd;

    a1 = ip[0] >> UNIT_QUANT_SHIFT;
    e1 = a1 >> 1;
    a1 -= e1;
    op[0] = a1;
    op[1] = op[2] = op[3] = e1;

    ip = tmp;
    for (i = 0; i < 4; i++) {
        e1                        = ip[0] >> 1;
        a1                        = ip[0] - e1;
        dest_w[dest_stride_w * 0] = highbd_clip_pixel_add(dest_r[dest_stride_r * 0], a1, bd);
        dest_w[dest_stride_w * 1] = highbd_clip_pixel_add(dest_r[dest_stride_r * 1], e1, bd);
        dest_w[dest_stride_w * 2] = highbd_clip_pixel_add(dest_r[dest_stride_r * 2], e1, bd);
        dest_w[dest_stride_w * 3] = highbd_clip_pixel_add(dest_r[dest_stride_r * 3], e1, bd);
        ip++;
        dest_r++;
        dest_w++;
    }
}

static void highbd_iwht4x4_add(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                               int32_t stride_w, int32_t eob, int32_t bd) {
    if (eob > 1) {
        svt_av1_highbd_iwht4x4_16_add_c(input, dest_r, stride_r, dest_w, stride_w, bd);
    } else {
        svt_av1_highbd_iwht4x4_1_add_c(input, dest_r, stride_r, dest_w, stride_w, bd);
    }
}

void svt_av1_highbd_inv_txfm_add_4x4(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    // assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    int32_t        eob      = txfm_param->eob;
    int32_t        bd       = txfm_param->bd;
    int32_t        lossless = txfm_param->lossless;
    const int32_t* src      = cast_to_int32(input);
    const TxType   tx_type  = txfm_param->tx_type;
    if (lossless) {
        assert(tx_type == DCT_DCT);
        highbd_iwht4x4_add(input, dest_r, stride_r, dest_w, stride_w, eob, bd);
        return;
    }
    svt_av1_inv_txfm2d_add_4x4(
        src, CONVERT_TO_SHORTPTR(dest_r), stride_r, CONVERT_TO_SHORTPTR(dest_w), stride_w, tx_type, bd);
}

static void highbd_inv_txfm_add_8x8(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                    int32_t stride_w, const TxfmParam* txfm_param) {
    int32_t        bd      = txfm_param->bd;
    const TxType   tx_type = txfm_param->tx_type;
    const int32_t* src     = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_8x8(
        src, CONVERT_TO_SHORTPTR(dest_r), stride_r, CONVERT_TO_SHORTPTR(dest_w), stride_w, tx_type, bd);
}

static void highbd_inv_txfm_add_16x16(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    int32_t        bd      = txfm_param->bd;
    const TxType   tx_type = txfm_param->tx_type;
    const int32_t* src     = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_16x16(
        src, CONVERT_TO_SHORTPTR(dest_r), stride_r, CONVERT_TO_SHORTPTR(dest_w), stride_w, tx_type, bd);
}

static void highbd_inv_txfm_add_32x32(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t  bd      = txfm_param->bd;
    const TxType   tx_type = txfm_param->tx_type;
    const int32_t* src     = cast_to_int32(input);
    switch (tx_type) {
    case DCT_DCT:
    case IDTX:
        svt_av1_inv_txfm2d_add_32x32(
            src, CONVERT_TO_SHORTPTR(dest_r), stride_r, CONVERT_TO_SHORTPTR(dest_w), stride_w, tx_type, bd);
        break;
    default:
        assert(0);
    }
}

static void highbd_inv_txfm_add_64x64(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t  bd      = txfm_param->bd;
    const TxType   tx_type = txfm_param->tx_type;
    const int32_t* src     = cast_to_int32(input);
    assert(tx_type == DCT_DCT);
    svt_av1_inv_txfm2d_add_64x64(
        src, CONVERT_TO_SHORTPTR(dest_r), stride_r, CONVERT_TO_SHORTPTR(dest_w), stride_w, tx_type, bd);
}

static void highbd_inv_txfm_add_4x8(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                    int32_t stride_w, const TxfmParam* txfm_param) {
    //TODO: add this assert once we fill tx_set_type    assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_4x8(src,
                               CONVERT_TO_SHORTPTR(dest_r),
                               stride_r,
                               CONVERT_TO_SHORTPTR(dest_w),
                               stride_w,
                               txfm_param->tx_type,
                               txfm_param->tx_size,
                               txfm_param->bd);
}

static void highbd_inv_txfm_add_8x4(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                    int32_t stride_w, const TxfmParam* txfm_param) {
    //TODO: add this assert once we fill tx_set_type    assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_8x4(src,
                               CONVERT_TO_SHORTPTR(dest_r),
                               stride_r,
                               CONVERT_TO_SHORTPTR(dest_w),
                               stride_w,
                               txfm_param->tx_type,
                               txfm_param->tx_size,
                               txfm_param->bd);
}

static void highbd_inv_txfm_add_8x16(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_8x16(src,
                                CONVERT_TO_SHORTPTR(dest_r),
                                stride_r,
                                CONVERT_TO_SHORTPTR(dest_w),
                                stride_w,
                                txfm_param->tx_type,
                                txfm_param->tx_size,
                                txfm_param->eob,
                                txfm_param->bd);
}

static void highbd_inv_txfm_add_16x8(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_16x8(src,
                                CONVERT_TO_SHORTPTR(dest_r),
                                stride_r,
                                CONVERT_TO_SHORTPTR(dest_w),
                                stride_w,
                                txfm_param->tx_type,
                                txfm_param->tx_size,
                                txfm_param->eob,
                                txfm_param->bd);
}

static void highbd_inv_txfm_add_16x32(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_16x32(src,
                                 CONVERT_TO_SHORTPTR(dest_r),
                                 stride_r,
                                 CONVERT_TO_SHORTPTR(dest_w),
                                 stride_w,
                                 txfm_param->tx_type,
                                 txfm_param->tx_size,
                                 txfm_param->eob,
                                 txfm_param->bd);
}

static void highbd_inv_txfm_add_32x16(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_32x16(src,
                                 CONVERT_TO_SHORTPTR(dest_r),
                                 stride_r,
                                 CONVERT_TO_SHORTPTR(dest_w),
                                 stride_w,
                                 txfm_param->tx_type,
                                 txfm_param->tx_size,
                                 txfm_param->eob,
                                 txfm_param->bd);
}

static void highbd_inv_txfm_add_16x4(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_16x4(src,
                                CONVERT_TO_SHORTPTR(dest_r),
                                stride_r,
                                CONVERT_TO_SHORTPTR(dest_w),
                                stride_w,
                                txfm_param->tx_type,
                                txfm_param->tx_size,
                                txfm_param->bd);
}

static void highbd_inv_txfm_add_4x16(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_4x16(src,
                                CONVERT_TO_SHORTPTR(dest_r),
                                stride_r,
                                CONVERT_TO_SHORTPTR(dest_w),
                                stride_w,
                                txfm_param->tx_type,
                                txfm_param->tx_size,
                                txfm_param->bd);
}

static void highbd_inv_txfm_add_32x8(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_32x8(src,
                                CONVERT_TO_SHORTPTR(dest_r),
                                stride_r,
                                CONVERT_TO_SHORTPTR(dest_w),
                                stride_w,
                                txfm_param->tx_type,
                                txfm_param->tx_size,
                                txfm_param->eob,
                                txfm_param->bd);
}

static void highbd_inv_txfm_add_8x32(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                     int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_8x32(src,
                                CONVERT_TO_SHORTPTR(dest_r),
                                stride_r,
                                CONVERT_TO_SHORTPTR(dest_w),
                                stride_w,
                                txfm_param->tx_type,
                                txfm_param->tx_size,
                                txfm_param->eob,
                                txfm_param->bd);
}

static void highbd_inv_txfm_add_32x64(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_32x64(src,
                                 CONVERT_TO_SHORTPTR(dest_r),
                                 stride_r,
                                 CONVERT_TO_SHORTPTR(dest_w),
                                 stride_w,
                                 txfm_param->tx_type,
                                 txfm_param->tx_size,
                                 txfm_param->eob,
                                 txfm_param->bd);
}

static void highbd_inv_txfm_add_64x32(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_64x32(src,
                                 CONVERT_TO_SHORTPTR(dest_r),
                                 stride_r,
                                 CONVERT_TO_SHORTPTR(dest_w),
                                 stride_w,
                                 txfm_param->tx_type,
                                 txfm_param->tx_size,
                                 txfm_param->eob,
                                 txfm_param->bd);
}

static void highbd_inv_txfm_add_16x64(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_16x64(src,
                                 CONVERT_TO_SHORTPTR(dest_r),
                                 stride_r,
                                 CONVERT_TO_SHORTPTR(dest_w),
                                 stride_w,
                                 txfm_param->tx_type,
                                 txfm_param->tx_size,
                                 txfm_param->eob,
                                 txfm_param->bd);
}

static void highbd_inv_txfm_add_64x16(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                      int32_t stride_w, const TxfmParam* txfm_param) {
    const int32_t* src = cast_to_int32(input);
    svt_av1_inv_txfm2d_add_64x16(src,
                                 CONVERT_TO_SHORTPTR(dest_r),
                                 stride_r,
                                 CONVERT_TO_SHORTPTR(dest_w),
                                 stride_w,
                                 txfm_param->tx_type,
                                 txfm_param->tx_size,
                                 txfm_param->eob,
                                 txfm_param->bd);
}

EbErrorType svt_aom_inv_transform_recon8bit(int32_t* coeff_buffer, //1D buffer
                                            uint8_t* recon_buffer_r, uint32_t recon_stride_r, uint8_t* recon_buffer_w,
                                            uint32_t recon_stride_w, TxSize txsize, TxType transform_type,
                                            PlaneType component_type, uint32_t eob, uint8_t lossless) {
    UNUSED(component_type);
    EbErrorType return_error = EB_ErrorNone;
    TxfmParam   txfm_param;
    txfm_param.tx_type  = transform_type;
    txfm_param.tx_size  = txsize;
    txfm_param.eob      = eob;
    txfm_param.lossless = lossless;
    txfm_param.bd       = 8;
    txfm_param.is_hbd   = 1;
    //TxfmParam.tx_set_type = av1_get_ext_tx_set_type(   txfm_param->tx_size, is_inter_block(xd->mi[0]), reduced_tx_set);

    if (recon_buffer_r != recon_buffer_w) {
        /* When output pointers to read and write are differents,
         * then kernel copy also all buffer from read to write,
         * and cannot be limited by End Of Buffer calculations. */
        txfm_param.eob = av1_get_max_eob(txsize);
    }

    svt_av1_inv_txfm_add(
        (const TranLow*)coeff_buffer, recon_buffer_r, recon_stride_r, recon_buffer_w, recon_stride_w, &txfm_param);

    return return_error;
}

static void highbd_inv_txfm_add(const TranLow* input, uint8_t* dest_r, int32_t stride_r, uint8_t* dest_w,
                                int32_t stride_w, const TxfmParam* txfm_param) {
    //assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);
    const TxSize tx_size = txfm_param->tx_size;
    switch (tx_size) {
    case TX_32X32:
        highbd_inv_txfm_add_32x32(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_16X16:
        highbd_inv_txfm_add_16x16(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_8X8:
        highbd_inv_txfm_add_8x8(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_4X8:
        highbd_inv_txfm_add_4x8(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_8X4:
        highbd_inv_txfm_add_8x4(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_8X16:
        highbd_inv_txfm_add_8x16(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_16X8:
        highbd_inv_txfm_add_16x8(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_16X32:
        highbd_inv_txfm_add_16x32(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_32X16:
        highbd_inv_txfm_add_32x16(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_64X64:
        highbd_inv_txfm_add_64x64(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_32X64:
        highbd_inv_txfm_add_32x64(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_64X32:
        highbd_inv_txfm_add_64x32(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_16X64:
        highbd_inv_txfm_add_16x64(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_64X16:
        highbd_inv_txfm_add_64x16(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_4X4:
        // this is like av1_short_idct4x4 but has a special case around eob<=1
        // which is significant (not just an optimization) for the lossless
        // case.
        svt_av1_highbd_inv_txfm_add_4x4(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_16X4:
        highbd_inv_txfm_add_16x4(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_4X16:
        highbd_inv_txfm_add_4x16(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_8X32:
        highbd_inv_txfm_add_8x32(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    case TX_32X8:
        highbd_inv_txfm_add_32x8(input, dest_r, stride_r, dest_w, stride_w, txfm_param);
        break;
    default:
        assert(0 && "Invalid transform size");
        break;
    }
}

EbErrorType svt_aom_inv_transform_recon(int32_t* coeff_buffer, //1D buffer
                                        uint8_t* recon_buffer_r, uint32_t recon_stride_r, uint8_t* recon_buffer_w,
                                        uint32_t recon_stride_w, TxSize txsize, uint32_t bit_depth,
                                        TxType transform_type, PlaneType component_type, uint32_t eob,
                                        uint8_t lossless) {
    UNUSED(component_type);
    EbErrorType return_error = EB_ErrorNone;
    TxfmParam   txfm_param;
    txfm_param.tx_type  = transform_type;
    txfm_param.tx_size  = txsize;
    txfm_param.eob      = eob;
    txfm_param.lossless = lossless;
    txfm_param.bd       = bit_depth;
    txfm_param.is_hbd   = 1;
    //TxfmParam.tx_set_type = av1_get_ext_tx_set_type(   txfm_param->tx_size, is_inter_block(xd->mi[0]), reduced_tx_set);

    if (recon_buffer_r != recon_buffer_w) {
        /* When output pointers to read and write are differents,
         * then kernel copy also all buffer from read to write,
         * and cannot be limited by End Of Buffer calculations. */
        txfm_param.eob = av1_get_max_eob(txsize);
    }

    highbd_inv_txfm_add(
        (const TranLow*)coeff_buffer, recon_buffer_r, recon_stride_r, recon_buffer_w, recon_stride_w, &txfm_param);

    return return_error;
}

void svt_av1_inv_txfm_add_c(const TranLow* dqcoeff, uint8_t* dst_r, int32_t stride_r, uint8_t* dst_w, int32_t stride_w,
                            const TxfmParam* txfm_param) {
    const TxSize tx_size = txfm_param->tx_size;
    DECLARE_ALIGNED(32, uint16_t, tmp[MAX_TX_SQUARE]);
    int32_t tmp_stride = MAX_TX_SIZE;
    int32_t w          = tx_size_wide[tx_size];
    int32_t h          = tx_size_high[tx_size];
    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            tmp[r * tmp_stride + c] = dst_r[r * stride_r + c];
        }
    }

    highbd_inv_txfm_add(dqcoeff, CONVERT_TO_BYTEPTR(tmp), tmp_stride, CONVERT_TO_BYTEPTR(tmp), tmp_stride, txfm_param);

    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            dst_w[r * stride_w + c] = (uint8_t)tmp[r * tmp_stride + c];
        }
    }
}

// av1_cospi_arr[i][j] = (int32_t)round(cos(M_PI*j/128) * (1<<(cos_bit_min+i)));
const int32_t svt_aom_eb_av1_cospi_arr_data[7][64] = {
    {1024, 1024, 1023, 1021, 1019, 1016, 1013, 1009, 1004, 999, 993, 987, 980, 972, 964, 955,
     946,  936,  926,  915,  903,  891,  878,  865,  851,  837, 822, 807, 792, 775, 759, 742,
     724,  706,  688,  669,  650,  630,  610,  590,  569,  548, 526, 505, 483, 460, 438, 415,
     392,  369,  345,  321,  297,  273,  249,  224,  200,  175, 150, 125, 100, 75,  50,  25},
    {2048, 2047, 2046, 2042, 2038, 2033, 2026, 2018, 2009, 1998, 1987, 1974, 1960, 1945, 1928, 1911,
     1892, 1872, 1851, 1829, 1806, 1782, 1757, 1730, 1703, 1674, 1645, 1615, 1583, 1551, 1517, 1483,
     1448, 1412, 1375, 1338, 1299, 1260, 1220, 1179, 1138, 1096, 1053, 1009, 965,  921,  876,  830,
     784,  737,  690,  642,  595,  546,  498,  449,  400,  350,  301,  251,  201,  151,  100,  50},
    {4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3996, 3973, 3948, 3920, 3889, 3857, 3822,
     3784, 3745, 3703, 3659, 3612, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
     2896, 2824, 2751, 2675, 2598, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
     1567, 1474, 1380, 1285, 1189, 1092, 995,  897,  799,  700,  601,  501,  401,  301,  201,  101},
    {8192, 8190, 8182, 8170, 8153, 8130, 8103, 8071, 8035, 7993, 7946, 7895, 7839, 7779, 7713, 7643,
     7568, 7489, 7405, 7317, 7225, 7128, 7027, 6921, 6811, 6698, 6580, 6458, 6333, 6203, 6070, 5933,
     5793, 5649, 5501, 5351, 5197, 5040, 4880, 4717, 4551, 4383, 4212, 4038, 3862, 3683, 3503, 3320,
     3135, 2948, 2760, 2570, 2378, 2185, 1990, 1795, 1598, 1401, 1202, 1003, 803,  603,  402,  201},
    {16384, 16379, 16364, 16340, 16305, 16261, 16207, 16143, 16069, 15986, 15893, 15791, 15679, 15557, 15426, 15286,
     15137, 14978, 14811, 14635, 14449, 14256, 14053, 13842, 13623, 13395, 13160, 12916, 12665, 12406, 12140, 11866,
     11585, 11297, 11003, 10702, 10394, 10080, 9760,  9434,  9102,  8765,  8423,  8076,  7723,  7366,  7005,  6639,
     6270,  5897,  5520,  5139,  4756,  4370,  3981,  3590,  3196,  2801,  2404,  2006,  1606,  1205,  804,   402},
    {32768, 32758, 32729, 32679, 32610, 32522, 32413, 32286, 32138, 31972, 31786, 31581, 31357, 31114, 30853, 30572,
     30274, 29957, 29622, 29269, 28899, 28511, 28106, 27684, 27246, 26791, 26320, 25833, 25330, 24812, 24279, 23732,
     23170, 22595, 22006, 21403, 20788, 20160, 19520, 18868, 18205, 17531, 16846, 16151, 15447, 14733, 14010, 13279,
     12540, 11793, 11039, 10279, 9512,  8740,  7962,  7180,  6393,  5602,  4808,  4011,  3212,  2411,  1608,  804},
    {65536, 65516, 65457, 65358, 65220, 65043, 64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
     60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581, 52639, 51665, 50660, 49624, 48559, 47464,
     46341, 45190, 44011, 42806, 41576, 40320, 39040, 37736, 36410, 35062, 33692, 32303, 30893, 29466, 28020, 26558,
     25080, 23586, 22078, 20557, 19024, 17479, 15924, 14359, 12785, 11204, 9616,  8022,  6424,  4821,  3216,  1608}};

// svt_aom_eb_av1_sinpi_arr_data[i][j] = (int32_t)round((sqrt(2) * sin(j*Pi/9) * 2 / 3) * (1
// << (cos_bit_min + i))) modified so that elements j=1,2 sum to element j=4.
const int32_t svt_aom_eb_av1_sinpi_arr_data[7][5] = {{0, 330, 621, 836, 951},
                                                     {0, 660, 1241, 1672, 1901},
                                                     {0, 1321, 2482, 3344, 3803},
                                                     {0, 2642, 4964, 6689, 7606},
                                                     {0, 5283, 9929, 13377, 15212},
                                                     {0, 10566, 19858, 26755, 30424},
                                                     {0, 21133, 39716, 53510, 60849}};
// Coefficient scaling and quantization with AV1 TX are tailored to
// the AV1 TX transforms.  Regardless of the bit-depth of the input,
// the transform stages scale the coefficient values up by a factor of
// 8 (3 bits) over the scale of the pixel values.  Thus, for 8-bit
// input, the coefficients have effectively 11 bits of scale depth
// (8+3), 10-bit input pixels result in 13-bit coefficient depth
// (10+3) and 12-bit pixels yield 15-bit (12+3) coefficient depth.
// All quantizers are built using this invariant of x8, 3-bit scaling,
// thus the Q3 suffix.

// A partial exception to this rule is large transforms; to avoid
// overflow, TX blocks with > 256 pels (>16x16) are scaled only
// 4-times unity (2 bits) over the pixel depth, and TX blocks with
// over 1024 pixels (>32x32) are scaled up only 2x unity (1 bit).
// This descaling is found via av1_tx_get_scale().  Thus, 16x32, 32x16
// and 32x32 transforms actually return Q2 coefficients, and 32x64,
// 64x32 and 64x64 transforms return Q1 coefficients.  However, the
// quantizers are de-scaled down on-the-fly by the same amount
// (av1_tx_get_scale()) during quantization, and as such the
// dequantized/decoded coefficients, even for large TX blocks, are always
// effectively Q3. Meanwhile, quantized/coded coefficients are Q0
// because Qn quantizers are applied to Qn tx coefficients.

// Note that encoder decision making (which uses the quantizer to
// generate several bespoke lamdas for RDO and other heuristics)
// expects quantizers to be larger for higher-bitdepth input.  In
// addition, the minimum allowable quantizer is 4; smaller values will
// underflow to 0 in the actual quantization routines.
static const int16_t ac_qlookup_QTX[QINDEX_RANGE] = {
    4,    8,    9,    10,   11,   12,   13,   14,   15,   16,   17,   18,   19,   20,   21,   22,   23,   24,   25,
    26,   27,   28,   29,   30,   31,   32,   33,   34,   35,   36,   37,   38,   39,   40,   41,   42,   43,   44,
    45,   46,   47,   48,   49,   50,   51,   52,   53,   54,   55,   56,   57,   58,   59,   60,   61,   62,   63,
    64,   65,   66,   67,   68,   69,   70,   71,   72,   73,   74,   75,   76,   77,   78,   79,   80,   81,   82,
    83,   84,   85,   86,   87,   88,   89,   90,   91,   92,   93,   94,   95,   96,   97,   98,   99,   100,  101,
    102,  104,  106,  108,  110,  112,  114,  116,  118,  120,  122,  124,  126,  128,  130,  132,  134,  136,  138,
    140,  142,  144,  146,  148,  150,  152,  155,  158,  161,  164,  167,  170,  173,  176,  179,  182,  185,  188,
    191,  194,  197,  200,  203,  207,  211,  215,  219,  223,  227,  231,  235,  239,  243,  247,  251,  255,  260,
    265,  270,  275,  280,  285,  290,  295,  300,  305,  311,  317,  323,  329,  335,  341,  347,  353,  359,  366,
    373,  380,  387,  394,  401,  408,  416,  424,  432,  440,  448,  456,  465,  474,  483,  492,  501,  510,  520,
    530,  540,  550,  560,  571,  582,  593,  604,  615,  627,  639,  651,  663,  676,  689,  702,  715,  729,  743,
    757,  771,  786,  801,  816,  832,  848,  864,  881,  898,  915,  933,  951,  969,  988,  1007, 1026, 1046, 1066,
    1087, 1108, 1129, 1151, 1173, 1196, 1219, 1243, 1267, 1292, 1317, 1343, 1369, 1396, 1423, 1451, 1479, 1508, 1537,
    1567, 1597, 1628, 1660, 1692, 1725, 1759, 1793, 1828,
};

static const int16_t ac_qlookup_10_QTX[QINDEX_RANGE] = {
    4,    9,    11,   13,   16,   18,   21,   24,   27,   30,   33,   37,   40,   44,   48,   51,   55,   59,   63,
    67,   71,   75,   79,   83,   88,   92,   96,   100,  105,  109,  114,  118,  122,  127,  131,  136,  140,  145,
    149,  154,  158,  163,  168,  172,  177,  181,  186,  190,  195,  199,  204,  208,  213,  217,  222,  226,  231,
    235,  240,  244,  249,  253,  258,  262,  267,  271,  275,  280,  284,  289,  293,  297,  302,  306,  311,  315,
    319,  324,  328,  332,  337,  341,  345,  349,  354,  358,  362,  367,  371,  375,  379,  384,  388,  392,  396,
    401,  409,  417,  425,  433,  441,  449,  458,  466,  474,  482,  490,  498,  506,  514,  523,  531,  539,  547,
    555,  563,  571,  579,  588,  596,  604,  616,  628,  640,  652,  664,  676,  688,  700,  713,  725,  737,  749,
    761,  773,  785,  797,  809,  825,  841,  857,  873,  889,  905,  922,  938,  954,  970,  986,  1002, 1018, 1038,
    1058, 1078, 1098, 1118, 1138, 1158, 1178, 1198, 1218, 1242, 1266, 1290, 1314, 1338, 1362, 1386, 1411, 1435, 1463,
    1491, 1519, 1547, 1575, 1603, 1631, 1663, 1695, 1727, 1759, 1791, 1823, 1859, 1895, 1931, 1967, 2003, 2039, 2079,
    2119, 2159, 2199, 2239, 2283, 2327, 2371, 2415, 2459, 2507, 2555, 2603, 2651, 2703, 2755, 2807, 2859, 2915, 2971,
    3027, 3083, 3143, 3203, 3263, 3327, 3391, 3455, 3523, 3591, 3659, 3731, 3803, 3876, 3952, 4028, 4104, 4184, 4264,
    4348, 4432, 4516, 4604, 4692, 4784, 4876, 4972, 5068, 5168, 5268, 5372, 5476, 5584, 5692, 5804, 5916, 6032, 6148,
    6268, 6388, 6512, 6640, 6768, 6900, 7036, 7172, 7312,
};

static const int16_t ac_qlookup_12_QTX[QINDEX_RANGE] = {
    4,     13,    19,    27,    35,    44,    54,    64,    75,    87,    99,    112,   126,   139,   154,   168,
    183,   199,   214,   230,   247,   263,   280,   297,   314,   331,   349,   366,   384,   402,   420,   438,
    456,   475,   493,   511,   530,   548,   567,   586,   604,   623,   642,   660,   679,   698,   716,   735,
    753,   772,   791,   809,   828,   846,   865,   884,   902,   920,   939,   957,   976,   994,   1012,  1030,
    1049,  1067,  1085,  1103,  1121,  1139,  1157,  1175,  1193,  1211,  1229,  1246,  1264,  1282,  1299,  1317,
    1335,  1352,  1370,  1387,  1405,  1422,  1440,  1457,  1474,  1491,  1509,  1526,  1543,  1560,  1577,  1595,
    1627,  1660,  1693,  1725,  1758,  1791,  1824,  1856,  1889,  1922,  1954,  1987,  2020,  2052,  2085,  2118,
    2150,  2183,  2216,  2248,  2281,  2313,  2346,  2378,  2411,  2459,  2508,  2556,  2605,  2653,  2701,  2750,
    2798,  2847,  2895,  2943,  2992,  3040,  3088,  3137,  3185,  3234,  3298,  3362,  3426,  3491,  3555,  3619,
    3684,  3748,  3812,  3876,  3941,  4005,  4069,  4149,  4230,  4310,  4390,  4470,  4550,  4631,  4711,  4791,
    4871,  4967,  5064,  5160,  5256,  5352,  5448,  5544,  5641,  5737,  5849,  5961,  6073,  6185,  6297,  6410,
    6522,  6650,  6778,  6906,  7034,  7162,  7290,  7435,  7579,  7723,  7867,  8011,  8155,  8315,  8475,  8635,
    8795,  8956,  9132,  9308,  9484,  9660,  9836,  10028, 10220, 10412, 10604, 10812, 11020, 11228, 11437, 11661,
    11885, 12109, 12333, 12573, 12813, 13053, 13309, 13565, 13821, 14093, 14365, 14637, 14925, 15213, 15502, 15806,
    16110, 16414, 16734, 17054, 17390, 17726, 18062, 18414, 18766, 19134, 19502, 19886, 20270, 20670, 21070, 21486,
    21902, 22334, 22766, 23214, 23662, 24126, 24590, 25070, 25551, 26047, 26559, 27071, 27599, 28143, 28687, 29247,
};

static const int16_t dc_qlookup_QTX[QINDEX_RANGE] = {
    4,   8,   8,   9,   10,  11,  12,  12,  13,   14,   15,   16,   17,   18,   19,   19,   20,  21,  22,  23,
    24,  25,  26,  26,  27,  28,  29,  30,  31,   32,   32,   33,   34,   35,   36,   37,   38,  38,  39,  40,
    41,  42,  43,  43,  44,  45,  46,  47,  48,   48,   49,   50,   51,   52,   53,   53,   54,  55,  56,  57,
    57,  58,  59,  60,  61,  62,  62,  63,  64,   65,   66,   66,   67,   68,   69,   70,   70,  71,  72,  73,
    74,  74,  75,  76,  77,  78,  78,  79,  80,   81,   81,   82,   83,   84,   85,   85,   87,  88,  90,  92,
    93,  95,  96,  98,  99,  101, 102, 104, 105,  107,  108,  110,  111,  113,  114,  116,  117, 118, 120, 121,
    123, 125, 127, 129, 131, 134, 136, 138, 140,  142,  144,  146,  148,  150,  152,  154,  156, 158, 161, 164,
    166, 169, 172, 174, 177, 180, 182, 185, 187,  190,  192,  195,  199,  202,  205,  208,  211, 214, 217, 220,
    223, 226, 230, 233, 237, 240, 243, 247, 250,  253,  257,  261,  265,  269,  272,  276,  280, 284, 288, 292,
    296, 300, 304, 309, 313, 317, 322, 326, 330,  335,  340,  344,  349,  354,  359,  364,  369, 374, 379, 384,
    389, 395, 400, 406, 411, 417, 423, 429, 435,  441,  447,  454,  461,  467,  475,  482,  489, 497, 505, 513,
    522, 530, 539, 549, 559, 569, 579, 590, 602,  614,  626,  640,  654,  668,  684,  700,  717, 736, 755, 775,
    796, 819, 843, 869, 896, 925, 955, 988, 1022, 1058, 1098, 1139, 1184, 1232, 1282, 1336,
};

static const int16_t dc_qlookup_10_QTX[QINDEX_RANGE] = {
    4,    9,    10,   13,   15,   17,   20,   22,   25,   28,   31,   34,   37,   40,   43,   47,   50,   53,   57,
    60,   64,   68,   71,   75,   78,   82,   86,   90,   93,   97,   101,  105,  109,  113,  116,  120,  124,  128,
    132,  136,  140,  143,  147,  151,  155,  159,  163,  166,  170,  174,  178,  182,  185,  189,  193,  197,  200,
    204,  208,  212,  215,  219,  223,  226,  230,  233,  237,  241,  244,  248,  251,  255,  259,  262,  266,  269,
    273,  276,  280,  283,  287,  290,  293,  297,  300,  304,  307,  310,  314,  317,  321,  324,  327,  331,  334,
    337,  343,  350,  356,  362,  369,  375,  381,  387,  394,  400,  406,  412,  418,  424,  430,  436,  442,  448,
    454,  460,  466,  472,  478,  484,  490,  499,  507,  516,  525,  533,  542,  550,  559,  567,  576,  584,  592,
    601,  609,  617,  625,  634,  644,  655,  666,  676,  687,  698,  708,  718,  729,  739,  749,  759,  770,  782,
    795,  807,  819,  831,  844,  856,  868,  880,  891,  906,  920,  933,  947,  961,  975,  988,  1001, 1015, 1030,
    1045, 1061, 1076, 1090, 1105, 1120, 1137, 1153, 1170, 1186, 1202, 1218, 1236, 1253, 1271, 1288, 1306, 1323, 1342,
    1361, 1379, 1398, 1416, 1436, 1456, 1476, 1496, 1516, 1537, 1559, 1580, 1601, 1624, 1647, 1670, 1692, 1717, 1741,
    1766, 1791, 1817, 1844, 1871, 1900, 1929, 1958, 1990, 2021, 2054, 2088, 2123, 2159, 2197, 2236, 2276, 2319, 2363,
    2410, 2458, 2508, 2561, 2616, 2675, 2737, 2802, 2871, 2944, 3020, 3102, 3188, 3280, 3375, 3478, 3586, 3702, 3823,
    3953, 4089, 4236, 4394, 4559, 4737, 4929, 5130, 5347,
};

static const int16_t dc_qlookup_12_QTX[QINDEX_RANGE] = {
    4,     12,    18,    25,    33,    41,    50,    60,    70,    80,    91,    103,   115,   127,   140,   153,
    166,   180,   194,   208,   222,   237,   251,   266,   281,   296,   312,   327,   343,   358,   374,   390,
    405,   421,   437,   453,   469,   484,   500,   516,   532,   548,   564,   580,   596,   611,   627,   643,
    659,   674,   690,   706,   721,   737,   752,   768,   783,   798,   814,   829,   844,   859,   874,   889,
    904,   919,   934,   949,   964,   978,   993,   1008,  1022,  1037,  1051,  1065,  1080,  1094,  1108,  1122,
    1136,  1151,  1165,  1179,  1192,  1206,  1220,  1234,  1248,  1261,  1275,  1288,  1302,  1315,  1329,  1342,
    1368,  1393,  1419,  1444,  1469,  1494,  1519,  1544,  1569,  1594,  1618,  1643,  1668,  1692,  1717,  1741,
    1765,  1789,  1814,  1838,  1862,  1885,  1909,  1933,  1957,  1992,  2027,  2061,  2096,  2130,  2165,  2199,
    2233,  2267,  2300,  2334,  2367,  2400,  2434,  2467,  2499,  2532,  2575,  2618,  2661,  2704,  2746,  2788,
    2830,  2872,  2913,  2954,  2995,  3036,  3076,  3127,  3177,  3226,  3275,  3324,  3373,  3421,  3469,  3517,
    3565,  3621,  3677,  3733,  3788,  3843,  3897,  3951,  4005,  4058,  4119,  4181,  4241,  4301,  4361,  4420,
    4479,  4546,  4612,  4677,  4742,  4807,  4871,  4942,  5013,  5083,  5153,  5222,  5291,  5367,  5442,  5517,
    5591,  5665,  5745,  5825,  5905,  5984,  6063,  6149,  6234,  6319,  6404,  6495,  6587,  6678,  6769,  6867,
    6966,  7064,  7163,  7269,  7376,  7483,  7599,  7715,  7832,  7958,  8085,  8214,  8352,  8492,  8635,  8788,
    8945,  9104,  9275,  9450,  9639,  9832,  10031, 10245, 10465, 10702, 10946, 11210, 11482, 11776, 12081, 12409,
    12750, 13118, 13501, 13913, 14343, 14807, 15290, 15812, 16356, 16943, 17575, 18237, 18949, 19718, 20521, 21387,
};

// In AV1 TX, the coefficients are always scaled up a factor of 8 (3 bits), so qtx == Q3.
int16_t svt_aom_dc_quant_qtx(int qindex, int delta, EbBitDepth bit_depth) {
    const int q_clamped = clamp(qindex + delta, 0, MAXQ);
    switch (bit_depth) {
    case EB_EIGHT_BIT:
        return dc_qlookup_QTX[q_clamped];
    case EB_TEN_BIT:
        return dc_qlookup_10_QTX[q_clamped];
    case EB_TWELVE_BIT:
        return dc_qlookup_12_QTX[q_clamped];
    default:
        assert(0 && "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT or EB_TWELVE_BIT");
        return -1;
    }
}

int16_t svt_aom_ac_quant_qtx(int qindex, int delta, EbBitDepth bit_depth) {
    const int q_clamped = clamp(qindex + delta, 0, MAXQ);
    switch (bit_depth) {
    case EB_EIGHT_BIT:
        return ac_qlookup_QTX[q_clamped];
    case EB_TEN_BIT:
        return ac_qlookup_10_QTX[q_clamped];
    case EB_TWELVE_BIT:
        return ac_qlookup_12_QTX[q_clamped];
    default:
        assert(0 && "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT or EB_TWELVE_BIT");
        return -1;
    }
}

int32_t svt_aom_get_qzbin_factor(int32_t q, EbBitDepth bit_depth) {
    const int32_t quant = svt_aom_dc_quant_qtx(q, 0, bit_depth);
    switch (bit_depth) {
    case EB_EIGHT_BIT:
        return q == 0 ? 64 : (quant < 148 ? 84 : 80);
    case EB_TEN_BIT:
        return q == 0 ? 64 : (quant < 592 ? 84 : 80);
    case EB_TWELVE_BIT:
        return q == 0 ? 64 : (quant < 2368 ? 84 : 80);
    default:
        assert(0 && "bit_depth should be EB_EIGHT_BIT, EB_TEN_BIT or EB_TWELVE_BIT");
        return -1;
    }
}

void svt_aom_invert_quant(int16_t* quant, int16_t* shift, int32_t d) {
    uint32_t t;
    int32_t  l, m;
    t = d;
    for (l = 0; t > 1; l++) {
        t >>= 1;
    }
    m      = 1 + (1 << (16 + l)) / d;
    *quant = (int16_t)(m - (1 << 16));
    *shift = 1 << (16 - l);
}
