/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbMotionEstimationContext_h
#define EbMotionEstimationContext_h

#include "definitions.h"
#include "md_rate_estimation.h"
#include "coding_unit.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
// 1-D interpolation shift value
#define if_shift 6
#define NUMBER_OF_SB_QUAD 4
#define VARIANCE_PRECISION 16
#define MEAN_PRECISION (VARIANCE_PRECISION >> 1)
#define HME_DECIM_FILTER_TAP 9

typedef struct MePredictionUnit {
    uint64_t distortion;
    int16_t  x_mv;
    int16_t  y_mv;
    uint32_t sub_pel_direction;
} MePredictionUnit;

typedef enum EbMeType {
    ME_CLOSE_LOOP  = 0,
    ME_MCTF        = 1,
    ME_TPL         = 2,
    ME_OPEN_LOOP   = 3,
    ME_FIRST_PASS  = 4,
    ME_DG_DETECTOR = 5
} EbMeType;

typedef enum EbMeTierZeroPu {
    // 2Nx2N [85 partitions]
    ME_TIER_ZERO_PU_64x64    = 0,
    ME_TIER_ZERO_PU_32x32_0  = 1,
    ME_TIER_ZERO_PU_32x32_1  = 2,
    ME_TIER_ZERO_PU_32x32_2  = 3,
    ME_TIER_ZERO_PU_32x32_3  = 4,
    ME_TIER_ZERO_PU_16x16_0  = 5,
    ME_TIER_ZERO_PU_16x16_1  = 6,
    ME_TIER_ZERO_PU_16x16_2  = 7,
    ME_TIER_ZERO_PU_16x16_3  = 8,
    ME_TIER_ZERO_PU_16x16_4  = 9,
    ME_TIER_ZERO_PU_16x16_5  = 10,
    ME_TIER_ZERO_PU_16x16_6  = 11,
    ME_TIER_ZERO_PU_16x16_7  = 12,
    ME_TIER_ZERO_PU_16x16_8  = 13,
    ME_TIER_ZERO_PU_16x16_9  = 14,
    ME_TIER_ZERO_PU_16x16_10 = 15,
    ME_TIER_ZERO_PU_16x16_11 = 16,
    ME_TIER_ZERO_PU_16x16_12 = 17,
    ME_TIER_ZERO_PU_16x16_13 = 18,
    ME_TIER_ZERO_PU_16x16_14 = 19,
    ME_TIER_ZERO_PU_16x16_15 = 20,
    ME_TIER_ZERO_PU_8x8_0    = 21,
    ME_TIER_ZERO_PU_8x8_1    = 22,
    ME_TIER_ZERO_PU_8x8_2    = 23,
    ME_TIER_ZERO_PU_8x8_3    = 24,
    ME_TIER_ZERO_PU_8x8_4    = 25,
    ME_TIER_ZERO_PU_8x8_5    = 26,
    ME_TIER_ZERO_PU_8x8_6    = 27,
    ME_TIER_ZERO_PU_8x8_7    = 28,
    ME_TIER_ZERO_PU_8x8_8    = 29,
    ME_TIER_ZERO_PU_8x8_9    = 30,
    ME_TIER_ZERO_PU_8x8_10   = 31,
    ME_TIER_ZERO_PU_8x8_11   = 32,
    ME_TIER_ZERO_PU_8x8_12   = 33,
    ME_TIER_ZERO_PU_8x8_13   = 34,
    ME_TIER_ZERO_PU_8x8_14   = 35,
    ME_TIER_ZERO_PU_8x8_15   = 36,
    ME_TIER_ZERO_PU_8x8_16   = 37,
    ME_TIER_ZERO_PU_8x8_17   = 38,
    ME_TIER_ZERO_PU_8x8_18   = 39,
    ME_TIER_ZERO_PU_8x8_19   = 40,
    ME_TIER_ZERO_PU_8x8_20   = 41,
    ME_TIER_ZERO_PU_8x8_21   = 42,
    ME_TIER_ZERO_PU_8x8_22   = 43,
    ME_TIER_ZERO_PU_8x8_23   = 44,
    ME_TIER_ZERO_PU_8x8_24   = 45,
    ME_TIER_ZERO_PU_8x8_25   = 46,
    ME_TIER_ZERO_PU_8x8_26   = 47,
    ME_TIER_ZERO_PU_8x8_27   = 48,
    ME_TIER_ZERO_PU_8x8_28   = 49,
    ME_TIER_ZERO_PU_8x8_29   = 50,
    ME_TIER_ZERO_PU_8x8_30   = 51,
    ME_TIER_ZERO_PU_8x8_31   = 52,
    ME_TIER_ZERO_PU_8x8_32   = 53,
    ME_TIER_ZERO_PU_8x8_33   = 54,
    ME_TIER_ZERO_PU_8x8_34   = 55,
    ME_TIER_ZERO_PU_8x8_35   = 56,
    ME_TIER_ZERO_PU_8x8_36   = 57,
    ME_TIER_ZERO_PU_8x8_37   = 58,
    ME_TIER_ZERO_PU_8x8_38   = 59,
    ME_TIER_ZERO_PU_8x8_39   = 60,
    ME_TIER_ZERO_PU_8x8_40   = 61,
    ME_TIER_ZERO_PU_8x8_41   = 62,
    ME_TIER_ZERO_PU_8x8_42   = 63,
    ME_TIER_ZERO_PU_8x8_43   = 64,
    ME_TIER_ZERO_PU_8x8_44   = 65,
    ME_TIER_ZERO_PU_8x8_45   = 66,
    ME_TIER_ZERO_PU_8x8_46   = 67,
    ME_TIER_ZERO_PU_8x8_47   = 68,
    ME_TIER_ZERO_PU_8x8_48   = 69,
    ME_TIER_ZERO_PU_8x8_49   = 70,
    ME_TIER_ZERO_PU_8x8_50   = 71,
    ME_TIER_ZERO_PU_8x8_51   = 72,
    ME_TIER_ZERO_PU_8x8_52   = 73,
    ME_TIER_ZERO_PU_8x8_53   = 74,
    ME_TIER_ZERO_PU_8x8_54   = 75,
    ME_TIER_ZERO_PU_8x8_55   = 76,
    ME_TIER_ZERO_PU_8x8_56   = 77,
    ME_TIER_ZERO_PU_8x8_57   = 78,
    ME_TIER_ZERO_PU_8x8_58   = 79,
    ME_TIER_ZERO_PU_8x8_59   = 80,
    ME_TIER_ZERO_PU_8x8_60   = 81,
    ME_TIER_ZERO_PU_8x8_61   = 82,
    ME_TIER_ZERO_PU_8x8_62   = 83,
    ME_TIER_ZERO_PU_8x8_63   = 84,
    // H  [42 partitions]
    ME_TIER_ZERO_PU_64x32_0 = 85,
    ME_TIER_ZERO_PU_64x32_1 = 86,
    ME_TIER_ZERO_PU_32x16_0 = 87,
    ME_TIER_ZERO_PU_32x16_1 = 88,
    ME_TIER_ZERO_PU_32x16_2 = 89,
    ME_TIER_ZERO_PU_32x16_3 = 90,
    ME_TIER_ZERO_PU_32x16_4 = 91,
    ME_TIER_ZERO_PU_32x16_5 = 92,
    ME_TIER_ZERO_PU_32x16_6 = 93,
    ME_TIER_ZERO_PU_32x16_7 = 94,
    ME_TIER_ZERO_PU_16x8_0  = 95,
    ME_TIER_ZERO_PU_16x8_1  = 96,
    ME_TIER_ZERO_PU_16x8_2  = 97,
    ME_TIER_ZERO_PU_16x8_3  = 98,
    ME_TIER_ZERO_PU_16x8_4  = 99,
    ME_TIER_ZERO_PU_16x8_5  = 100,
    ME_TIER_ZERO_PU_16x8_6  = 101,
    ME_TIER_ZERO_PU_16x8_7  = 102,
    ME_TIER_ZERO_PU_16x8_8  = 103,
    ME_TIER_ZERO_PU_16x8_9  = 104,
    ME_TIER_ZERO_PU_16x8_10 = 105,
    ME_TIER_ZERO_PU_16x8_11 = 106,
    ME_TIER_ZERO_PU_16x8_12 = 107,
    ME_TIER_ZERO_PU_16x8_13 = 108,
    ME_TIER_ZERO_PU_16x8_14 = 109,
    ME_TIER_ZERO_PU_16x8_15 = 110,
    ME_TIER_ZERO_PU_16x8_16 = 111,
    ME_TIER_ZERO_PU_16x8_17 = 112,
    ME_TIER_ZERO_PU_16x8_18 = 113,
    ME_TIER_ZERO_PU_16x8_19 = 114,
    ME_TIER_ZERO_PU_16x8_20 = 115,
    ME_TIER_ZERO_PU_16x8_21 = 116,
    ME_TIER_ZERO_PU_16x8_22 = 117,
    ME_TIER_ZERO_PU_16x8_23 = 118,
    ME_TIER_ZERO_PU_16x8_24 = 119,
    ME_TIER_ZERO_PU_16x8_25 = 120,
    ME_TIER_ZERO_PU_16x8_26 = 121,
    ME_TIER_ZERO_PU_16x8_27 = 122,
    ME_TIER_ZERO_PU_16x8_28 = 123,
    ME_TIER_ZERO_PU_16x8_29 = 124,
    ME_TIER_ZERO_PU_16x8_30 = 125,
    ME_TIER_ZERO_PU_16x8_31 = 126,
    // V  [42 partitions]
    ME_TIER_ZERO_PU_32x64_0 = 127,
    ME_TIER_ZERO_PU_32x64_1 = 128,
    ME_TIER_ZERO_PU_16x32_0 = 129,
    ME_TIER_ZERO_PU_16x32_1 = 130,
    ME_TIER_ZERO_PU_16x32_2 = 131,
    ME_TIER_ZERO_PU_16x32_3 = 132,
    ME_TIER_ZERO_PU_16x32_4 = 133,
    ME_TIER_ZERO_PU_16x32_5 = 134,
    ME_TIER_ZERO_PU_16x32_6 = 135,
    ME_TIER_ZERO_PU_16x32_7 = 136,
    ME_TIER_ZERO_PU_8x16_0  = 137,
    ME_TIER_ZERO_PU_8x16_1  = 138,
    ME_TIER_ZERO_PU_8x16_2  = 139,
    ME_TIER_ZERO_PU_8x16_3  = 140,
    ME_TIER_ZERO_PU_8x16_4  = 141,
    ME_TIER_ZERO_PU_8x16_5  = 142,
    ME_TIER_ZERO_PU_8x16_6  = 143,
    ME_TIER_ZERO_PU_8x16_7  = 144,
    ME_TIER_ZERO_PU_8x16_8  = 145,
    ME_TIER_ZERO_PU_8x16_9  = 146,
    ME_TIER_ZERO_PU_8x16_10 = 147,
    ME_TIER_ZERO_PU_8x16_11 = 148,
    ME_TIER_ZERO_PU_8x16_12 = 149,
    ME_TIER_ZERO_PU_8x16_13 = 150,
    ME_TIER_ZERO_PU_8x16_14 = 151,
    ME_TIER_ZERO_PU_8x16_15 = 152,
    ME_TIER_ZERO_PU_8x16_16 = 153,
    ME_TIER_ZERO_PU_8x16_17 = 154,
    ME_TIER_ZERO_PU_8x16_18 = 155,
    ME_TIER_ZERO_PU_8x16_19 = 156,
    ME_TIER_ZERO_PU_8x16_20 = 157,
    ME_TIER_ZERO_PU_8x16_21 = 158,
    ME_TIER_ZERO_PU_8x16_22 = 159,
    ME_TIER_ZERO_PU_8x16_23 = 160,
    ME_TIER_ZERO_PU_8x16_24 = 161,
    ME_TIER_ZERO_PU_8x16_25 = 162,
    ME_TIER_ZERO_PU_8x16_26 = 163,
    ME_TIER_ZERO_PU_8x16_27 = 164,
    ME_TIER_ZERO_PU_8x16_28 = 165,
    ME_TIER_ZERO_PU_8x16_29 = 166,
    ME_TIER_ZERO_PU_8x16_30 = 167,
    ME_TIER_ZERO_PU_8x16_31 = 168,
    // H4 [16 partitions]
    ME_TIER_ZERO_PU_32x8_0  = 169,
    ME_TIER_ZERO_PU_32x8_1  = 170,
    ME_TIER_ZERO_PU_32x8_2  = 171,
    ME_TIER_ZERO_PU_32x8_3  = 172,
    ME_TIER_ZERO_PU_32x8_4  = 173,
    ME_TIER_ZERO_PU_32x8_5  = 174,
    ME_TIER_ZERO_PU_32x8_6  = 175,
    ME_TIER_ZERO_PU_32x8_7  = 176,
    ME_TIER_ZERO_PU_32x8_8  = 177,
    ME_TIER_ZERO_PU_32x8_9  = 178,
    ME_TIER_ZERO_PU_32x8_10 = 179,
    ME_TIER_ZERO_PU_32x8_11 = 180,
    ME_TIER_ZERO_PU_32x8_12 = 181,
    ME_TIER_ZERO_PU_32x8_13 = 182,
    ME_TIER_ZERO_PU_32x8_14 = 183,
    ME_TIER_ZERO_PU_32x8_15 = 184,
    // V4 [16 partitions]
    ME_TIER_ZERO_PU_8x32_0  = 185,
    ME_TIER_ZERO_PU_8x32_1  = 186,
    ME_TIER_ZERO_PU_8x32_2  = 187,
    ME_TIER_ZERO_PU_8x32_3  = 188,
    ME_TIER_ZERO_PU_8x32_4  = 189,
    ME_TIER_ZERO_PU_8x32_5  = 190,
    ME_TIER_ZERO_PU_8x32_6  = 191,
    ME_TIER_ZERO_PU_8x32_7  = 192,
    ME_TIER_ZERO_PU_8x32_8  = 193,
    ME_TIER_ZERO_PU_8x32_9  = 194,
    ME_TIER_ZERO_PU_8x32_10 = 195,
    ME_TIER_ZERO_PU_8x32_11 = 196,
    ME_TIER_ZERO_PU_8x32_12 = 197,
    ME_TIER_ZERO_PU_8x32_13 = 198,
    ME_TIER_ZERO_PU_8x32_14 = 199,
    ME_TIER_ZERO_PU_8x32_15 = 200,
    ME_TIER_ZERO_PU_64x16_0 = 201,
    ME_TIER_ZERO_PU_64x16_1 = 202,
    ME_TIER_ZERO_PU_64x16_2 = 203,
    ME_TIER_ZERO_PU_64x16_3 = 204,
    ME_TIER_ZERO_PU_16x64_0 = 205,
    ME_TIER_ZERO_PU_16x64_1 = 206,
    ME_TIER_ZERO_PU_16x64_2 = 207,
    ME_TIER_ZERO_PU_16x64_3 = 208
} EbMeTierZeroPu;

typedef struct MeHmeRefPruneCtrls {
    bool enable_me_hme_ref_pruning;
    // TH used to prune references based on hme sad deviation
    uint16_t prune_ref_if_hme_sad_dev_bigger_than_th;
    // TH used to prune references based on me sad deviation
    uint16_t prune_ref_if_me_sad_dev_bigger_than_th;
    uint32_t zz_sad_th; // enable zz based ref pruning if zz sad < this threshold. set to zero to disable.
    uint16_t zz_sad_pct; // prune the ref that has sad-deviation-to-best > th_percentage
    uint32_t phme_sad_th; // enable preHme based ref pruning if prehme sad < this threshold. set to zero to disable.
    uint16_t phme_sad_pct; // prune the ref that has sad-deviation-to-best > th_percentage
} MeHmeRefPruneCtrls;

typedef struct MeSrCtrls {
    uint8_t enable_me_sr_adjustment;
    // reduce the ME search region if HME MVs and HME sad are small
    uint16_t reduce_me_sr_based_on_mv_length_th;
    // reduce the ME search region if HME MVs and HME sad are small
    uint16_t stationary_hme_sad_abs_th;
    // Reduction factor for the ME search region if HME MVs and HME sad are small
    uint16_t stationary_me_sr_divisor;
    // reduce the ME search region if HME sad is small
    uint16_t reduce_me_sr_based_on_hme_sad_abs_th;
    // Reduction factor for the ME search region if HME sad is small
    uint16_t me_sr_divisor_for_low_hme_sad;
    uint8_t  distance_based_hme_resizing; // scale down the HME search area for high ref-indices
} MeSrCtrls;

/* Me8x8VarCtrls will adjust the ME search area based on the 8x8 SAD variance of the search centre. The minimum
* search dimensions will be limited to height=8, width=3 when the algorithm is used.  Consequently, this
* algorithm will be bypassed if height * width <= 24.
*/
typedef struct Me8x8VarCtrls {
    // If true, adjust the ME search area based on the variance of the 8x8 SAD of the search centre
    uint8_t enabled;
    // If ME 8x8 SAD variance is below me_sr_div4_th, divide the search area width/height by 4
    uint32_t me_sr_div4_th;
    // If ME 8x8 SAD variance is below me_sr_div2_th, divide the search area width/height by 2
    uint32_t me_sr_div2_th;
    // If ME 8x8 SAD variance is above me_sr_mult2_th, multiply the search area width/height by 2
    uint32_t me_sr_mult2_th;
} Me8x8VarCtrls;

#define SEARCH_REGION_COUNT 2

typedef struct SearchArea {
    uint16_t width; // search area width
    uint16_t height; // search area height
} SearchArea;

typedef struct SearchAreaMinMax {
    SearchArea sa_min; // min search area
    SearchArea sa_max; // max search area
} SearchAreaMinMax;

typedef struct SearchInfo {
    SearchArea sa; // search area sizes
    Mv         best_mv; // best mv
    uint64_t   sad; // best sad
    uint8_t    valid; //1 if the mv+sad are valid; invalid for some pruned references.
} SearchInfo;

typedef struct PreHmeCtrls {
    uint8_t          enable;
    SearchAreaMinMax prehme_sa_cfg[SEARCH_REGION_COUNT];
    uint8_t          skip_search_line; //if 1 skips every other search region line
    uint8_t          l1_early_exit;
} PreHmeCtrls;

typedef struct SearchResults {
    uint8_t  list_i; // list index of this ref
    uint8_t  ref_i; // ref list index of this ref
    int16_t  hme_sc_x; // hme search centre x
    int16_t  hme_sc_y; // hme search centre y
    uint64_t hme_sad; // hme sad
    uint8_t  do_ref; // to process this ref in ME or not
} SearchResults;

typedef struct MvBasedSearchAdj {
    bool enabled;
    // if true, apply search area increase to nearest ref frame only (ref_idx == 0)
    bool nearest_ref_only;
    // If an MV component of the ME search centre is larger than the TH, increase the search area
    // of that component by the sa_multiplier
    uint16_t mv_size_th;
    uint16_t sa_multiplier;
} MvBasedSearchAdj;

typedef struct MeContext {
    EbDctor dctor;
    // Search region stride
    uint32_t interpolated_full_stride[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX];
    uint32_t me_distortion[SQUARE_PU_COUNT];

    uint32_t   tf_decay_factor_fp16[MAX_MB_PLANE];
    double     tf_decay_factor[3];
    TfControls tf_ctrls;

    uint8_t* b64_src_ptr;
    uint32_t b64_src_stride;

    uint8_t*           quarter_b64_buffer;
    uint32_t           quarter_b64_buffer_stride;
    uint8_t*           sixteenth_b64_buffer;
    uint32_t           sixteenth_b64_buffer_stride;
    uint8_t*           integer_buffer_ptr[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX];
    uint32_t*          p_best_sad_8x8;
    uint32_t*          p_best_sad_16x16;
    uint32_t*          p_best_sad_32x32;
    uint32_t*          p_best_sad_64x64;
    uint32_t*          p_best_mv8x8;
    uint32_t*          p_best_mv16x16;
    uint32_t*          p_best_mv32x32;
    uint32_t*          p_best_mv64x64;
    uint32_t           p_sad32x32[4];
    uint32_t           p_sad16x16[16];
    uint32_t           p_sad8x8[64];
    uint32_t           p_sb_best_sad[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][SQUARE_PU_COUNT];
    uint32_t           p_sb_best_mv[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][SQUARE_PU_COUNT];
    uint32_t*          p_best_full_pel_mv8x8;
    uint32_t*          p_best_full_pel_mv16x16;
    uint32_t*          p_best_full_pel_mv32x32;
    uint32_t*          p_best_full_pel_mv64x64;
    uint16_t*          p_eight_pos_sad16x16;
    uint32_t           p_eight_sad32x32[4][8];
    uint32_t           p_eight_sad16x16[16][8];
    uint32_t           p_eight_sad8x8[64][8];
    uint8_t            hme_search_method;
    uint8_t            me_search_method;
    bool               enable_hme_flag;
    bool               enable_hme_level0_flag;
    bool               enable_hme_level1_flag;
    bool               enable_hme_level2_flag;
    MeHmeRefPruneCtrls me_hme_prune_ctrls;
    MeSrCtrls          me_sr_adjustment_ctrls;
    Me8x8VarCtrls      me_8x8_var_ctrls;
    MvBasedSearchAdj   mv_based_sa_adj;
    // ME
    uint8_t          best_list_idx;
    uint8_t          best_ref_idx;
    SearchAreaMinMax me_sa;

    // HME
    uint16_t         num_hme_sa_w;
    uint16_t         num_hme_sa_h;
    SearchAreaMinMax hme_l0_sa; // Total HME Level-0 search area
    SearchAreaMinMax hme_l0_sa_default_tf; // Total HME Level-0 search area TF
    SearchArea       hme_l1_sa; // HME Level-1 search area per HME-L0 search centre
    SearchArea       hme_l2_sa; // HME Level-2 search area per HME-L1 search centre
    SearchResults    search_results[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    uint32_t         reduce_me_sr_divisor[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];

    SearchInfo  prehme_data[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][SEARCH_REGION_COUNT];
    PreHmeCtrls prehme_ctrl;
    int16_t     x_hme_level0_search_center[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                                      [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    int16_t y_hme_level0_search_center[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                                      [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    uint64_t hme_level0_sad[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                           [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    int16_t x_hme_level1_search_center[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                                      [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    int16_t y_hme_level1_search_center[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                                      [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    uint64_t hme_level1_sad[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                           [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    int16_t x_hme_level2_search_center[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                                      [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    int16_t y_hme_level2_search_center[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                                      [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    uint64_t hme_level2_sad[MAX_NUM_OF_REF_PIC_LIST][MAX_REF_IDX][EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT]
                           [EB_HME_SEARCH_AREA_ROW_MAX_COUNT];
    // ------- Context for Alt-Ref ME ------
    void* alt_ref_reference_ptr;
    // Open Loop ME
    EbMeType me_type;

    uint8_t num_of_list_to_search;
    uint8_t num_of_ref_pic_to_search[2];
    uint8_t temporal_layer_index;
    // Flag will be true if the current frame is used as a reference picture by other frames.
    bool                        is_ref;
    EbDownScaledBufDescPtrArray me_ds_ref_array[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    // tf
    uint8_t  tf_chroma;
    int      tf_frame_index;
    int      tf_index_center;
    int16_t  tf_64x64_mv_x;
    int16_t  tf_64x64_mv_y;
    uint64_t tf_64x64_block_error;
    int16_t  tf_16x16_mv_x[16];
    int16_t  tf_16x16_mv_y[16];
    uint64_t tf_16x16_block_error[16];
    int16_t  tf_8x8_mv_x[64];
    int16_t  tf_8x8_mv_y[64];
    uint64_t tf_8x8_block_error[64];
    int      tf_16x16_block_split_flag[4][4];
    int16_t  tf_32x32_mv_x[4];
    int16_t  tf_32x32_mv_y[4];
    uint64_t tf_32x32_block_error[4];
    int      tf_32x32_block_split_flag[4];
    int      tf_block_row;
    int      tf_block_col;
    uint32_t idx_32x32;
    uint16_t tf_mv_dist_th;
    int32_t  prune_me_candidates_th;
    uint8_t  use_best_unipred_cand_only; // Use only the best unipred candidate when MRP is off
    uint8_t  sc_class_me_boost;
    uint8_t  reduce_hme_l0_sr_th_min;
    uint8_t  reduce_hme_l0_sr_th_max;
    uint16_t tf_me_exit_th;
    uint8_t  tf_use_pred_64x64_only_th;
    uint8_t  tf_subpel_early_exit_th;
    uint32_t zz_sad[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
    uint32_t me_early_exit_th;
    uint32_t me_safe_limit_zz_th;
    uint32_t tf_tot_vert_blks; //total vertical motion blocks in TF
    uint32_t tf_tot_horz_blks; //total horizontal motion blocks in TF
    uint32_t b64_width;
    uint32_t b64_height;
    uint8_t  performed_phme[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH][2];
    uint32_t prev_me_stage_based_exit_th;
} MeContext;

typedef uint64_t (*EB_ME_DISTORTION_FUNC)(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                          uint32_t width, uint32_t height);
EbErrorType svt_aom_me_context_ctor(MeContext* object_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbMotionEstimationContext_h
