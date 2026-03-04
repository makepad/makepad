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

#ifndef EbInvTransforms_h
#define EbInvTransforms_h

#include "common_utils.h"
#include "coefficients.h"

#ifdef __cplusplus
extern "C" {
#endif

#define UNIT_QUANT_SHIFT 2
#define INV_COS_BIT 12
#define MAX_TXFM_STAGE_NUM 12
#define MAX_TXWH_IDX 5
#define AOM_QM_BITS 5
#define MAX_TX_SCALE 1

#define IS_2D_TRANSFORM(tx_type) (tx_type < IDTX)

static const int8_t inv_cos_bit_col[MAX_TXWH_IDX][MAX_TXWH_IDX] = {
    {INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, 0, 0},
    {INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, 0},
    {INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT},
    {0, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT},
    {0, 0, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT}};
static const int8_t inv_cos_bit_row[MAX_TXWH_IDX][MAX_TXWH_IDX] = {
    {INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, 0, 0},
    {INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, 0},
    {INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT},
    {0, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT},
    {0, 0, INV_COS_BIT, INV_COS_BIT, INV_COS_BIT}};

static const TxType1D vtx_tab[TX_TYPES] = {
    DCT_1D,
    ADST_1D,
    DCT_1D,
    ADST_1D,
    FLIPADST_1D,
    DCT_1D,
    FLIPADST_1D,
    ADST_1D,
    FLIPADST_1D,
    IDTX_1D,
    DCT_1D,
    IDTX_1D,
    ADST_1D,
    IDTX_1D,
    FLIPADST_1D,
    IDTX_1D,
};
static const TxType1D htx_tab[TX_TYPES] = {
    DCT_1D,
    DCT_1D,
    ADST_1D,
    ADST_1D,
    DCT_1D,
    FLIPADST_1D,
    FLIPADST_1D,
    FLIPADST_1D,
    ADST_1D,
    IDTX_1D,
    IDTX_1D,
    DCT_1D,
    IDTX_1D,
    ADST_1D,
    IDTX_1D,
    FLIPADST_1D,
};

// Utility function that returns the log of the ratio of the col and row
// sizes.
typedef enum TxfmType {
    TXFM_TYPE_DCT4,
    TXFM_TYPE_DCT8,
    TXFM_TYPE_DCT16,
    TXFM_TYPE_DCT32,
    TXFM_TYPE_DCT64,
    TXFM_TYPE_ADST4,
    TXFM_TYPE_ADST8,
    TXFM_TYPE_ADST16,
    TXFM_TYPE_ADST32,
    TXFM_TYPE_IDENTITY4,
    TXFM_TYPE_IDENTITY8,
    TXFM_TYPE_IDENTITY16,
    TXFM_TYPE_IDENTITY32,
    TXFM_TYPE_IDENTITY64,
    TXFM_TYPES,
    TXFM_TYPE_INVALID,
} TxfmType;

typedef struct Txfm2dFlipCfg {
    TxSize        tx_size;
    int32_t       ud_flip; // flip upside down
    int32_t       lr_flip; // flip left to right
    const int8_t* shift;
    int8_t        cos_bit_col;
    int8_t        cos_bit_row;
    int8_t        stage_range_col[MAX_TXFM_STAGE_NUM];
    int8_t        stage_range_row[MAX_TXFM_STAGE_NUM];
    TxfmType      txfm_type_col;
    TxfmType      txfm_type_row;
    int32_t       stage_num_col;
    int32_t       stage_num_row;
} Txfm2dFlipCfg;

EbErrorType svt_aom_inv_transform_recon(int32_t* coeff_buffer, //1D buffer
                                        uint8_t* recon_buffer_r, uint32_t recon_stride_r, uint8_t* recon_buffer_w,
                                        uint32_t recon_stride_w, TxSize txsize, uint32_t bit_increment,
                                        TxType transform_type, PlaneType component_type, uint32_t eob,
                                        uint8_t lossless);

EbErrorType svt_aom_inv_transform_recon8bit(int32_t* coeff_buffer, //1D buffer
                                            uint8_t* recon_buffer_r, uint32_t recon_stride_r, uint8_t* recon_buffer_w,
                                            uint32_t recon_stride_w, TxSize txsize, TxType transform_type,
                                            PlaneType component_type, uint32_t eob, uint8_t lossless);

static INLINE int32_t av1_get_max_eob(TxSize tx_size) {
    if (tx_size == TX_64X64 || tx_size == TX_64X32 || tx_size == TX_32X64) {
        return 1024;
    }
    if (tx_size == TX_16X64 || tx_size == TX_64X16) {
        return 512;
    }
    return tx_size_2d[tx_size];
}

static INLINE void get_flip_cfg(TxType tx_type, int32_t* ud_flip, int32_t* lr_flip) {
    switch (tx_type) {
    case DCT_DCT:
    case ADST_DCT:
    case DCT_ADST:
    case ADST_ADST:
        *ud_flip = 0;
        *lr_flip = 0;
        break;
    case IDTX:
    case V_DCT:
    case H_DCT:
    case V_ADST:
    case H_ADST:
        *ud_flip = 0;
        *lr_flip = 0;
        break;
    case FLIPADST_DCT:
    case FLIPADST_ADST:
    case V_FLIPADST:
        *ud_flip = 1;
        *lr_flip = 0;
        break;
    case DCT_FLIPADST:
    case ADST_FLIPADST:
    case H_FLIPADST:
        *ud_flip = 0;
        *lr_flip = 1;
        break;
    case FLIPADST_FLIPADST:
        *ud_flip = 1;
        *lr_flip = 1;
        break;
    default:
        *ud_flip = 0;
        *lr_flip = 0;
        assert(0);
    }
}

static INLINE void set_flip_cfg(TxType tx_type, Txfm2dFlipCfg* cfg) {
    get_flip_cfg(tx_type, &cfg->ud_flip, &cfg->lr_flip);
}

static INLINE int32_t get_txw_idx(TxSize tx_size) {
    return tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
}

static INLINE int32_t get_txh_idx(TxSize tx_size) {
    return tx_size_high_log2[tx_size] - tx_size_high_log2[0];
}

static const TxfmType av1_txfm_type_ls[5][TX_TYPES_1D] = {
    {TXFM_TYPE_DCT4, TXFM_TYPE_ADST4, TXFM_TYPE_ADST4, TXFM_TYPE_IDENTITY4},
    {TXFM_TYPE_DCT8, TXFM_TYPE_ADST8, TXFM_TYPE_ADST8, TXFM_TYPE_IDENTITY8},
    {TXFM_TYPE_DCT16, TXFM_TYPE_ADST16, TXFM_TYPE_ADST16, TXFM_TYPE_IDENTITY16},
    {TXFM_TYPE_DCT32, TXFM_TYPE_ADST32, TXFM_TYPE_ADST32, TXFM_TYPE_IDENTITY32},
    {TXFM_TYPE_DCT64, TXFM_TYPE_INVALID, TXFM_TYPE_INVALID, TXFM_TYPE_IDENTITY64}};
static const int8_t av1_txfm_stage_num_list[TXFM_TYPES] = {
    4, // TXFM_TYPE_DCT4
    6, // TXFM_TYPE_DCT8
    8, // TXFM_TYPE_DCT16
    10, // TXFM_TYPE_DCT32
    12, // TXFM_TYPE_DCT64
    7, // TXFM_TYPE_ADST4
    8, // TXFM_TYPE_ADST8
    10, // TXFM_TYPE_ADST16
    12, // TXFM_TYPE_ADST32
    1, // TXFM_TYPE_IDENTITY4
    1, // TXFM_TYPE_IDENTITY8
    1, // TXFM_TYPE_IDENTITY16
    1, // TXFM_TYPE_IDENTITY32
    1, // TXFM_TYPE_IDENTITY64
};

static const int8_t iadst4_range[7] = {0, 1, 0, 0, 0, 0, 0};

// sum of fwd_shift_##
static const int8_t inv_start_range[TX_SIZES_ALL] = {
    5, // 4x4 transform
    6, // 8x8 transform
    7, // 16x16 transform
    7, // 32x32 transform
    7, // 64x64 transform
    5, // 4x8 transform
    5, // 8x4 transform
    6, // 8x16 transform
    6, // 16x8 transform
    6, // 16x32 transform
    6, // 32x16 transform
    6, // 32x64 transform
    6, // 64x32 transform
    6, // 4x16 transform
    6, // 16x4 transform
    7, // 8x32 transform
    7, // 32x8 transform
    7, // 16x64 transform
    7, // 64x16 transform
};

extern const int32_t svt_aom_eb_av1_cospi_arr_data[7][64];
extern const int32_t svt_aom_eb_av1_sinpi_arr_data[7][5];
extern const int8_t* svt_aom_inv_txfm_shift_ls[TX_SIZES_ALL];

static const int32_t cos_bit_min = 10;

static INLINE const int32_t* cospi_arr(int32_t n) {
    return svt_aom_eb_av1_cospi_arr_data[n - cos_bit_min];
}

static INLINE const int32_t* sinpi_arr(int32_t n) {
    return svt_aom_eb_av1_sinpi_arr_data[n - cos_bit_min];
}

#define new_sqrt2_bits ((int32_t)12)
// 2^12 * sqrt(2)
static const int32_t new_sqrt2 = 5793;
// 2^12 / sqrt(2)
static const int32_t new_inv_sqrt2 = 2896;

typedef void (*TxfmFunc)(const int32_t* input, int32_t* output, int8_t cos_bit, const int8_t* stage_range);

// Note:
// TranHigh is the datatype used for intermediate transform stages.
typedef int64_t TranHigh;

static INLINE int32_t round_shift(int64_t value, int32_t bit) {
    assert(bit >= 1);
    return (int32_t)((value + (1ll << (bit - 1))) >> bit);
}

static INLINE int32_t half_btf(int32_t w0, int32_t in0, int32_t w1, int32_t in1, int bit) {
    int64_t result_64    = (int64_t)(w0 * in0) + (int64_t)(w1 * in1);
    int64_t intermediate = result_64 + (1LL << (bit - 1));
    // NOTE(david.barker): The value 'result_64' may not necessarily fit
    // into 32 bits. However, the result of this function is nominally
    // ROUND_POWER_OF_TWO_64(result_64, bit)
    // and that is required to fit into stage_range[stage] many bits
    // (checked by range_check_buf()).
    //
    // Here we've unpacked that rounding operation, and it can be shown
    // that the value of 'intermediate' here *does* fit into 32 bits
    // for any conformant bitstream.
    // The upshot is that, if you do all this calculation using
    // wrapping 32-bit arithmetic instead of (non-wrapping) 64-bit arithmetic,
    // then you'll still get the correct result.
    // To provide a check on this logic, we assert that 'intermediate'
    // would fit into an int32 if range checking is enabled.
#if CONFIG_COEFFICIENT_RANGE_CHECKING
    assert(intermediate >= INT32_MIN && intermediate <= INT32_MAX);
#endif
    return (int32_t)(intermediate >> bit);
}

static INLINE int32_t get_rect_tx_log_ratio(int32_t col, int32_t row) {
    if (col == row) {
        return 0;
    }
    if (col > row) {
        if (col == row * 2) {
            return 1;
        }
        if (col == row * 4) {
            return 2;
        }
        assert(0 && "Unsupported transform size");
    } else {
        if (row == col * 2) {
            return -1;
        }
        if (row == col * 4) {
            return -2;
        }
        assert(0 && "Unsupported transform size");
    }
    return 0; // Invalid
}

void svt_av1_round_shift_array_c(int32_t* arr, int32_t size, int32_t bit);

static const BlockSize txsize_to_bsize[TX_SIZES_ALL] = {
    BLOCK_4X4, // TX_4X4
    BLOCK_8X8, // TX_8X8
    BLOCK_16X16, // TX_16X16
    BLOCK_32X32, // TX_32X32
    BLOCK_64X64, // TX_64X64
    BLOCK_4X8, // TX_4X8
    BLOCK_8X4, // TX_8X4
    BLOCK_8X16, // TX_8X16
    BLOCK_16X8, // TX_16X8
    BLOCK_16X32, // TX_16X32
    BLOCK_32X16, // TX_32X16
    BLOCK_32X64, // TX_32X64
    BLOCK_64X32, // TX_64X32
    BLOCK_4X16, // TX_4X16
    BLOCK_16X4, // TX_16X4
    BLOCK_8X32, // TX_8X32
    BLOCK_32X8, // TX_32X8
    BLOCK_16X64, // TX_16X64
    BLOCK_64X16, // TX_64X16
};

static const int8_t txsize_log2_minus4[TX_SIZES_ALL] = {
    0, // TX_4X4
    2, // TX_8X8
    4, // TX_16X16
    6, // TX_32X32
    6, // TX_64X64
    1, // TX_4X8
    1, // TX_8X4
    3, // TX_8X16
    3, // TX_16X8
    5, // TX_16X32
    5, // TX_32X16
    6, // TX_32X64
    6, // TX_64X32
    2, // TX_4X16
    2, // TX_16X4
    4, // TX_8X32
    4, // TX_32X8
    5, // TX_16X64
    5, // TX_64X16
};

int32_t  svt_aom_get_qzbin_factor(int32_t q, EbBitDepth bit_depth);
void     svt_aom_invert_quant(int16_t* quant, int16_t* shift, int32_t d);
int16_t  svt_aom_dc_quant_qtx(int32_t qindex, int32_t delta, EbBitDepth bit_depth);
int16_t  svt_aom_ac_quant_qtx(int32_t qindex, int32_t delta, EbBitDepth bit_depth);
TxfmFunc svt_aom_inv_txfm_type_to_func(TxfmType txfmtype);
#ifdef __cplusplus
}
#endif

#endif // EbInvTransforms_h
