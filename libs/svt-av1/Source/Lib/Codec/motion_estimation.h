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

#ifndef EbMotionEstimation_h
#define EbMotionEstimation_h

#include "definitions.h"
#include "coding_unit.h"

#include "me_process.h"
#include "me_context.h"
#include "pic_buffer_desc.h"
#include "reference_object.h"
#include "pd_process.h"
#include "utility.h"
#include "aom_dsp_rtcd.h"

#ifdef __cplusplus
extern "C" {
#endif

EbErrorType svt_aom_motion_estimation_b64(PictureParentControlSet* pcs, uint32_t b64_index, uint32_t b64_origin_x,
                                          uint32_t b64_origin_y, MeContext* me_ctx, EbPictureBufferDesc* input_ptr);

void svt_aom_downsample_2d_c(uint8_t* input_samples, uint32_t input_stride, uint32_t input_area_width,
                             uint32_t input_area_height, uint8_t* decim_samples, uint32_t decim_stride,
                             uint32_t decim_step);

EbErrorType open_loop_intra_search_sb(PictureParentControlSet* pcs, uint32_t sb_index,
                                      MotionEstimationContext_t* me_context_ptr, EbPictureBufferDesc* input_ptr);

EbErrorType av1_open_loop_intra_search(PictureParentControlSet* pcs, MotionEstimationContext_t* me_context_ptr,
                                       EbPictureBufferDesc* input_ptr);
#define a_b_c 0
#define a_c_b 1
#define b_a_c 2
#define b_c_a 3
#define c_a_b 4
#define c_b_a 5

#define TOP_LEFT_POSITION 0
#define TOP_POSITION 1
#define TOP_RIGHT_POSITION 2
#define RIGHT_POSITION 3
#define BOTTOM_RIGHT_POSITION 4
#define BOTTOM_POSITION 5
#define BOTTOM_LEFT_POSITION 6
#define LEFT_POSITION 7

// The interpolation is performed using a set of three 4 tap filters
#define IFShiftAvcStyle 1
#define F0 0
#define F1 1
#define F2 2
#define MAX_SSE_VALUE 128 * 128 * 255 * 255
#define MAX_SAD_VALUE 128 * 128 * 255
// Thresholds used for determining level of motion (used in sparse search)
#define MEDIUM_TEMPORAL_MV_TH 2048
#define LOW_TEMPORAL_MV_TH 1024

#define HIGH_SPATIAL_MV_TH 2048
#define MEDIUM_SPATIAL_MV_TH 512
#define LOW_SPATIAL_MV_TH 256

// Interpolation Filters
// clang-format off
static const int32_t me_if_coeff[3][4] = {
    { -4, 54, 16, -2 }, // F0
    { -4, 36, 36, -4 }, // F1
    { -2, 16, 54, -4 }, // F2
};

static const uint32_t tab16x16[16] = {
    0, 1, 4, 5,
    2, 3, 6, 7,
    8, 9, 12, 13,
    10, 11, 14, 15
};
static const uint32_t tab8x8[64] = {
    0, 1, 4, 5, 16, 17, 20, 21,
    2, 3, 6, 7, 18, 19, 22, 23,
    8, 9, 12, 13, 24, 25, 28, 29,
    10, 11, 14, 15, 26, 27, 30, 31,
    32, 33, 36, 37, 48, 49, 52, 53,
    34, 35, 38, 39, 50, 51, 54, 55,
    40, 41, 44, 45, 56, 57, 60, 61,
    42, 43, 46, 47, 58, 59, 62, 63
};
static const uint32_t partition_width[SQUARE_PU_COUNT] = {
    64,                                                                          // (1)
    32, 32, 32, 32,                                                              // (4)
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
};
static const uint32_t partition_height[SQUARE_PU_COUNT] = {
    64,                                                                          // (1)
    32, 32, 32, 32,                                                              // (4)
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
    8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,                              // (16)
};
static const uint32_t pu_search_index_map[SQUARE_PU_COUNT][2] = {
    { 0, 0 },
    { 0, 0 }, { 32, 0 }, { 0, 32 }, { 32, 32 },
    { 0, 0 }, { 16, 0 }, { 32, 0 }, { 48, 0 },
    { 0, 16 }, { 16, 16 }, { 32, 16 }, { 48, 16 },
    { 0, 32 }, { 16, 32 }, { 32, 32 }, { 48, 32 },
    { 0, 48 }, { 16, 48 }, { 32, 48 }, { 48, 48 },
    { 0, 0 }, { 8, 0 }, { 16, 0 }, { 24, 0 }, { 32, 0 }, { 40, 0 }, { 48, 0 }, { 56, 0 },
    { 0, 8 }, { 8, 8 }, { 16, 8 }, { 24, 8 }, { 32, 8 }, { 40, 8 }, { 48, 8 }, { 56, 8 },
    { 0, 16 }, { 8, 16 }, { 16, 16 }, { 24, 16 }, { 32, 16 }, { 40, 16 }, { 48, 16 }, { 56, 16 },
    { 0, 24 }, { 8, 24 }, { 16, 24 }, { 24, 24 }, { 32, 24 }, { 40, 24 }, { 48, 24 }, { 56, 24 },
    { 0, 32 }, { 8, 32 }, { 16, 32 }, { 24, 32 }, { 32, 32 }, { 40, 32 }, { 48, 32 }, { 56, 32 },
    { 0, 40 }, { 8, 40 }, { 16, 40 }, { 24, 40 }, { 32, 40 }, { 40, 40 }, { 48, 40 }, { 56, 40 },
    { 0, 48 }, { 8, 48 }, { 16, 48 }, { 24, 48 }, { 32, 48 }, { 40, 48 }, { 48, 48 }, { 56, 48 },
    { 0, 56 }, { 8, 56 }, { 16, 56 }, { 24, 56 }, { 32, 56 }, { 40, 56 }, { 48, 56 }, { 56, 56 },
};

static const uint8_t sub_position_type[16] = { 0, 2, 1, 2, 2, 2, 2, 2, 1, 2, 1, 2, 2, 2, 2, 2 };
// clang-format on

uint32_t svt_aom_compute8x4_sad_kernel_c(uint8_t* src, // input parameter, source samples Ptr
                                         uint32_t src_stride, // input parameter, source stride
                                         uint8_t* ref, // input parameter, reference samples Ptr
                                         uint32_t ref_stride);
void     svt_ext_all_sad_calculation_8x8_16x16_c(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                 uint32_t mv, uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
                                                 uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
                                                 uint32_t p_eight_sad16x16[16][8], uint32_t p_eight_sad8x8[64][8],
                                                 bool sub_sad);

/*******************************************
    Calculate SAD for 32x32,64x64 from 16x16
    and check if there is improvment, if yes keep
    the best SAD+MV
    *******************************************/
void svt_ext_eight_sad_calculation_32x32_64x64_c(const uint32_t p_sad16x16[16][8], uint32_t* p_best_sad_32x32,
                                                 uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                                 uint32_t* p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]);

// factor to slowdown the ME search region growth to MAX
uint16_t svt_aom_get_scaled_picture_distance(uint16_t dist);

#ifdef __cplusplus
}
#endif
#endif
