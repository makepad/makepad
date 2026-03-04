/*
 * Copyright(c) 2019 Netflix, Inc.
 * Copyright(c) 2019 Intel Corporation
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 3-Clause Clear License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <assert.h>
#include "temporal_filtering.h"
#include "compute_sad.h"
#include "motion_estimation.h"
#include "me_process.h"
#include "me_context.h"
#include "lambda_rate_tables.h"
#include "pic_analysis_process.h"
#include "md_process.h"
#include "av1me.h"
#ifdef ARCH_X86_64
#include <xmmintrin.h>
#endif
#include "object.h"
#include "enc_inter_prediction.h"
#include "svt_log.h"
#include <limits.h>
#include "pack_unpack_c.h"
#include "pic_operators.h"
#include "ac_bias.h"

#undef _MM_HINT_T2
#define _MM_HINT_T2 1

#include "pd_results.h"
#include "utility.h"

// clang-format off
static const uint32_t subblock_xy_16x16[N_16X16_BLOCKS][2] = {{0, 0},
                                                              {0, 1},
                                                              {0, 2},
                                                              {0, 3},
                                                              {1, 0},
                                                              {1, 1},
                                                              {1, 2},
                                                              {1, 3},
                                                              {2, 0},
                                                              {2, 1},
                                                              {2, 2},
                                                              {2, 3},
                                                              {3, 0},
                                                              {3, 1},
                                                              {3, 2},
                                                              {3, 3}};
static const uint32_t idx_32x32_to_idx_16x16[4][4]         = {
    {0, 1, 4, 5}, {2, 3, 6, 7}, {8, 9, 12, 13}, {10, 11, 14, 15}};

static const uint32_t subblock_xy_8x8[N_8X8_BLOCKS][2] = {
    {0, 0}, {0, 1}, {0, 2}, {0, 3}, {0, 4}, {0, 5}, {0, 6}, {0, 7},
    {1, 0}, {1, 1}, {1, 2}, {1, 3}, {1, 4}, {1, 5}, {1, 6}, {1, 7},
    {2, 0}, {2, 1}, {2, 2}, {2, 3}, {2, 4}, {2, 5}, {2, 6}, {2, 7},
    {3, 0}, {3, 1}, {3, 2}, {3, 3}, {3, 4}, {3, 5}, {3, 6}, {3, 7},
    {4, 0}, {4, 1}, {4, 2}, {4, 3}, {4, 4}, {4, 5}, {4, 6}, {4, 7},
    {5, 0}, {5, 1}, {5, 2}, {5, 3}, {5, 4}, {5, 5}, {5, 6}, {5, 7},
    {6, 0}, {6, 1}, {6, 2}, {6, 3}, {6, 4}, {6, 5}, {6, 6}, {6, 7},
    {7, 0}, {7, 1}, {7, 2}, {7, 3}, {7, 4}, {7, 5}, {7, 6}, {7, 7}
};

static const uint32_t idx_32x32_to_idx_8x8[4][4][4] = {
    { {0, 1, 8, 9}, {2, 3, 10, 11}, {16, 17, 24, 25}, {18, 19, 26, 27} },
    { {4, 5, 12, 13}, {6, 7, 14, 15}, {20, 21, 28, 29}, {22, 23, 30, 31} },
    { {32, 33, 40, 41}, {34, 35, 42, 43}, {48, 49, 56, 57}, {50, 51, 58, 59} },
    { {36, 37, 44, 45}, {38, 39, 46, 47}, {52, 53, 60, 61}, {54, 55, 62, 63} }
};
// clang-format on

int32_t svt_aom_get_frame_update_type(SequenceControlSet* scs, PictureParentControlSet* pcs);
#if DEBUG_SCALING
// save YUV to file - auxiliary function for debug
void save_YUV_to_file(char* filename, EbByte buffer_y, EbByte buffer_u, EbByte buffer_v, uint16_t width,
                      uint16_t height, uint16_t stride_y, uint16_t stride_u, uint16_t stride_v, uint16_t org_y,
                      uint16_t org_x, uint32_t ss_x, uint32_t ss_y) {
    FILE* fid;

    // save current source picture to a YUV file
    FOPEN(fid, filename, "wb");

    if (!fid) {
        SVT_LOG("Unable to open file %s to write.\n", "temp_picture.yuv");
    } else {
        // the source picture saved in the enchanced_picture_ptr contains a border in x and y dimensions
        EbByte pic_point = buffer_y + (org_y * stride_y) + org_x;
        for (int h = 0; h < height; h++) {
            fwrite(pic_point, 1, (size_t)width, fid);
            pic_point = pic_point + stride_y;
        }
        pic_point = buffer_u + ((org_y >> ss_y) * stride_u) + (org_x >> ss_x);
        for (int h = 0; h < (height >> ss_y); h++) {
            fwrite(pic_point, 1, (size_t)width >> ss_x, fid);
            pic_point = pic_point + stride_u;
        }
        pic_point = buffer_v + ((org_y >> ss_y) * stride_v) + (org_x >> ss_x);
        for (int h = 0; h < (height >> ss_y); h++) {
            fwrite(pic_point, 1, (size_t)width >> ss_x, fid);
            pic_point = pic_point + stride_v;
        }
        fclose(fid);
    }
}

// save YUV to file - auxiliary function for debug
void save_YUV_to_file_highbd(char* filename, uint16_t* buffer_y, uint16_t* buffer_u, uint16_t* buffer_v, uint16_t width,
                             uint16_t height, uint16_t stride_y, uint16_t stride_u, uint16_t stride_v, uint16_t org_y,
                             uint16_t org_x, uint32_t ss_x, uint32_t ss_y) {
    FILE* fid;

    // save current source picture to a YUV file
    FOPEN(fid, filename, "wb");

    if (!fid) {
        SVT_LOG("Unable to open file %s to write.\n", "temp_picture.yuv");
    } else {
        // the source picture saved in the enchanced_picture_ptr contains a border in x and y dimensions
        uint16_t* pic_point = buffer_y + (org_y * stride_y) + org_x;
        for (int h = 0; h < height; h++) {
            fwrite(pic_point, 2, (size_t)width, fid);
            pic_point = pic_point + stride_y;
        }
        pic_point = buffer_u + ((org_y >> ss_y) * stride_u) + (org_x >> ss_x);
        for (int h = 0; h < (height >> ss_y); h++) {
            fwrite(pic_point, 2, (size_t)width >> ss_x, fid);

            pic_point = pic_point + stride_u;
        }
        pic_point = buffer_v + ((org_y >> ss_y) * stride_v) + (org_x >> ss_x);
        for (int h = 0; h < (height >> ss_y); h++) {
            fwrite(pic_point, 2, (size_t)width >> ss_x, fid);
            pic_point = pic_point + stride_v;
        }
        fclose(fid);
    }
}
#endif

static void derive_tf_32x32_block_split_flag(MeContext* me_ctx) {
    int      subblock_errors[4];
    uint32_t idx_32x32   = me_ctx->idx_32x32;
    int      block_error = (int)me_ctx->tf_32x32_block_error[idx_32x32];

    // `block_error` is initialized as INT_MAX and will be overwritten after
    // motion search with reference frame, therefore INT_MAX can ONLY be accessed
    // by to-filter frame.
    if (block_error == INT_MAX) {
        me_ctx->tf_32x32_block_split_flag[idx_32x32] = 0;
        memset(&me_ctx->tf_16x16_block_split_flag[idx_32x32][0],
               0,
               sizeof(me_ctx->tf_16x16_block_split_flag[idx_32x32][0]) * 4);
    }

    int min_subblock_error = INT_MAX;
    int max_subblock_error = INT_MIN;
    int sum_subblock_error = 0;
    for (int i = 0; i < 4; ++i) {
        subblock_errors[i] = (int)me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i];

        if (me_ctx->tf_ctrls.enable_8x8_pred) {
            // check 8x8
            int error_8x8 = 0;
            for (int idx_8x8 = 0; idx_8x8 < 4; ++idx_8x8) {
                error_8x8 += (int)me_ctx->tf_8x8_block_error[idx_32x32 * 16 + 4 * i + idx_8x8];
            }

            // Determine if 16x16 should be split into 8x8
            if (subblock_errors[i] * 8 < error_8x8 * 16) { // No split.
                me_ctx->tf_16x16_block_split_flag[idx_32x32][i] = 0;
            } else { // Do split.
                me_ctx->tf_16x16_block_split_flag[idx_32x32][i] = 1;
                me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] = error_8x8;
                subblock_errors[i]                              = error_8x8;
            }
        } else {
            me_ctx->tf_16x16_block_split_flag[idx_32x32][i] = 0;
        }

        sum_subblock_error += subblock_errors[i];
        min_subblock_error = AOMMIN(min_subblock_error, subblock_errors[i]);
        max_subblock_error = AOMMAX(max_subblock_error, subblock_errors[i]);
    }
    if (block_error * 14 < sum_subblock_error * 16) { // No split.
        me_ctx->tf_32x32_block_split_flag[idx_32x32] = 0;
    } else { // Do split.
        me_ctx->tf_32x32_block_split_flag[idx_32x32] = 1;
    }
}

// Create and initialize all necessary ME context structures
static void create_me_context_and_picture_control(MotionEstimationContext_t* me_context_ptr,
                                                  PictureParentControlSet*   picture_control_set_ptr_frame,
                                                  PictureParentControlSet*   centre_pcs,
                                                  EbPictureBufferDesc* input_picture_ptr_central, int blk_row,
                                                  int blk_col, uint32_t ss_x, uint32_t ss_y) {
    // set reference picture for alt-refs
    me_context_ptr->me_ctx->alt_ref_reference_ptr = (EbPaReferenceObject*)
                                                        picture_control_set_ptr_frame->pa_ref_pic_wrapper->object_ptr;
    me_context_ptr->me_ctx->me_type = ME_MCTF;

    // set the buffers with the original, quarter and sixteenth pixels version of the source frame
    EbPaReferenceObject* src_object     = (EbPaReferenceObject*)centre_pcs->pa_ref_pic_wrapper->object_ptr;
    EbPictureBufferDesc* padded_pic_ptr = src_object->input_padded_pic;
    // Set 1/4 and 1/16 ME reference buffer(s); filtered or decimated
    EbPictureBufferDesc* quarter_pic_ptr   = src_object->quarter_downsampled_picture_ptr;
    EbPictureBufferDesc* sixteenth_pic_ptr = src_object->sixteenth_downsampled_picture_ptr;
    // Parts from MotionEstimationKernel()
    uint32_t b64_origin_x = (uint32_t)(blk_col * BW);
    uint32_t b64_origin_y = (uint32_t)(blk_row * BH);

    // Load the SB from the input to the intermediate SB buffer
    int buffer_index = (input_picture_ptr_central->org_y + b64_origin_y) * input_picture_ptr_central->stride_y +
        input_picture_ptr_central->org_x + b64_origin_x;

    // set search method
    me_context_ptr->me_ctx->hme_search_method = FULL_SAD_SEARCH;
#ifdef ARCH_X86_64
    {
        uint8_t* src_ptr    = &(padded_pic_ptr->buffer_y[buffer_index]);
        uint32_t b64_height = (input_picture_ptr_central->height - b64_origin_y) < BLOCK_SIZE_64
            ? input_picture_ptr_central->height - b64_origin_y
            : BLOCK_SIZE_64;
        //_MM_HINT_T0     //_MM_HINT_T1    //_MM_HINT_T2    //_MM_HINT_NTA
        uint32_t i;
        for (i = 0; i < b64_height; i++) {
            char const* p = (char const*)(src_ptr + i * padded_pic_ptr->stride_y);

            _mm_prefetch(p, _MM_HINT_T2);
        }
    }
#endif
    me_context_ptr->me_ctx->b64_src_ptr    = &(padded_pic_ptr->buffer_y[buffer_index]);
    me_context_ptr->me_ctx->b64_src_stride = padded_pic_ptr->stride_y;

    // Load the 1/4 decimated SB from the 1/4 decimated input to the 1/4 intermediate SB buffer
    buffer_index = (quarter_pic_ptr->org_y + (b64_origin_y >> ss_y)) * quarter_pic_ptr->stride_y +
        quarter_pic_ptr->org_x + (b64_origin_x >> ss_x);

    me_context_ptr->me_ctx->quarter_b64_buffer        = &quarter_pic_ptr->buffer_y[buffer_index];
    me_context_ptr->me_ctx->quarter_b64_buffer_stride = quarter_pic_ptr->stride_y;

    // Load the 1/16 decimated SB from the 1/16 decimated input to the 1/16 intermediate SB buffer
    buffer_index = (sixteenth_pic_ptr->org_y + (b64_origin_y >> 2)) * sixteenth_pic_ptr->stride_y +
        sixteenth_pic_ptr->org_x + (b64_origin_x >> 2);

    me_context_ptr->me_ctx->sixteenth_b64_buffer        = &sixteenth_pic_ptr->buffer_y[buffer_index];
    me_context_ptr->me_ctx->sixteenth_b64_buffer_stride = sixteenth_pic_ptr->stride_y;
}

// Apply filtering to the central picture
void svt_aom_apply_filtering_central_c(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central, EbByte* src,
                                       uint32_t** accum, uint16_t** count, uint16_t blk_width, uint16_t blk_height,
                                       uint32_t ss_x, uint32_t ss_y) {
    uint16_t blk_height_y  = blk_height;
    uint16_t blk_width_y   = blk_width;
    uint16_t blk_height_ch = blk_height >> ss_y;
    uint16_t blk_width_ch  = blk_width >> ss_x;
    uint16_t src_stride_y  = input_picture_ptr_central->stride_y;
    uint16_t src_stride_ch = src_stride_y >> ss_x;

    const int modifier = TF_PLANEWISE_FILTER_WEIGHT_SCALE;

    // Luma
    for (uint16_t k = 0, i = 0; i < blk_height_y; i++) {
        for (uint16_t j = 0; j < blk_width_y; j++) {
            accum[C_Y][k] = modifier * src[C_Y][i * src_stride_y + j];
            count[C_Y][k] = modifier;
            ++k;
        }
    }

    // Chroma
    if (me_ctx->tf_chroma) {
        for (uint16_t k = 0, i = 0; i < blk_height_ch; i++) {
            for (uint16_t j = 0; j < blk_width_ch; j++) {
                accum[C_U][k] = modifier * src[C_U][i * src_stride_ch + j];
                count[C_U][k] = modifier;

                accum[C_V][k] = modifier * src[C_V][i * src_stride_ch + j];
                count[C_V][k] = modifier;
                ++k;
            }
        }
    }
}

// Apply filtering to the central picture
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_apply_filtering_central_highbd_c(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
                                              uint16_t** src_16bit, uint32_t** accum, uint16_t** count,
                                              uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y) {
    uint16_t blk_height_y  = blk_height;
    uint16_t blk_width_y   = blk_width;
    uint16_t blk_height_ch = blk_height >> ss_y;
    uint16_t blk_width_ch  = blk_width >> ss_x;
    uint16_t src_stride_y  = input_picture_ptr_central->stride_y;
    uint16_t src_stride_ch = src_stride_y >> ss_x;

    const int modifier = TF_PLANEWISE_FILTER_WEIGHT_SCALE;

    // Luma
    for (uint16_t k = 0, i = 0; i < blk_height_y; i++) {
        for (uint16_t j = 0; j < blk_width_y; j++) {
            accum[C_Y][k] = modifier * src_16bit[C_Y][i * src_stride_y + j];
            count[C_Y][k] = modifier;
            ++k;
        }
    }

    // Chroma
    if (me_ctx->tf_chroma) {
        for (uint16_t k = 0, i = 0; i < blk_height_ch; i++) {
            for (uint16_t j = 0; j < blk_width_ch; j++) {
                accum[C_U][k] = modifier * src_16bit[C_U][i * src_stride_ch + j];
                count[C_U][k] = modifier;

                accum[C_V][k] = modifier * src_16bit[C_V][i * src_stride_ch + j];
                count[C_V][k] = modifier;
                ++k;
            }
        }
    }
}
#endif

// clang-format off
//log1p(x) for x in [-1..6], step 1/32 values in Fixed Points shift 16
static const int32_t log1p_tab_fp16[] = {
    -2147483647 - 1,
    -227130,
    -181704,
    -155131,
    -136278,
    -121654,
    -109705,
    -99603,
    -90852,
    -83133,
    -76228,
    -69982,
    -64279,
    -59033,
    -54177,
    -49655,
    -45426,
    -41452,
    -37707,
    -34163,
    -30802,
    -27604,
    -24555,
    -21642,
    -18853,
    -16178,
    -13607,
    -11134,
    -8751,
    -6451,
    -4229,
    -2080,
    0,
    2016,
    3973,
    5872,
    7719,
    9514,
    11262,
    12964,
    14623,
    16242,
    17821,
    19363,
    20870,
    22342,
    23783,
    25192,
    26572,
    27923,
    29247,
    30545,
    31818,
    33066,
    34291,
    35494,
    36674,
    37834,
    38974,
    40095,
    41196,
    42279,
    43345,
    44394,
    45426,
    46442,
    47442,
    48428,
    49399,
    50355,
    51298,
    52228,
    53145,
    54049,
    54940,
    55820,
    56688,
    57545,
    58390,
    59225,
    60050,
    60864,
    61668,
    62462,
    63247,
    64023,
    64789,
    65547,
    66296,
    67036,
    67769,
    68493,
    69209,
    69917,
    70618,
    71312,
    71998,
    72677,
    73349,
    74015,
    74673,
    75326,
    75971,
    76611,
    77244,
    77871,
    78492,
    79108,
    79717,
    80321,
    80920,
    81513,
    82101,
    82683,
    83261,
    83833,
    84400,
    84963,
    85521,
    86074,
    86622,
    87166,
    87705,
    88240,
    88771,
    89297,
    89820,
    90338,
    90852,
    91362,
    91868,
    92370,
    92868,
    93363,
    93854,
    94341,
    94825,
    95305,
    95782,
    96255,
    96725,
    97191,
    97654,
    98114,
    98571,
    99024,
    99475,
    99922,
    100366,
    100808,
    101246,
    101681,
    102114,
    102544,
    102971,
    103395,
    103816,
    104235,
    104651,
    105065,
    105476,
    105884,
    106290,
    106693,
    107094,
    107492,
    107888,
    108282,
    108673,
    109062,
    109449,
    109833,
    110215,
    110595,
    110973,
    111348,
    111722,
    112093,
    112462,
    112830,
    113195,
    113558,
    113919,
    114278,
    114635,
    114990,
    115344,
    115695,
    116044,
    116392,
    116738,
    117082,
    117424,
    117765,
    118103,
    118440,
    118776,
    119109,
    119441,
    119771,
    120100,
    120426,
    120752,
    121075,
    121397,
    121718,
    122037,
    122354,
    122670,
    122984,
    123297,
    123608,
    123918,
    124227,
    124534,
    124839,
    125143,
    125446,
    125747,
    126047,
    126346,
    126643,
    126939,
    127233,
    127527,
};
// clang-format on

int32_t svt_aom_noise_log1p_fp16(int32_t noise_level_fp16) {
    int32_t base_fp16 = (65536 /*1:fp16*/ + noise_level_fp16);

    if (base_fp16 <= 0) {
        return (-2147483647 - 1) /*-MAX*/;
    } else if (base_fp16 < (458752) /*7:fp16*/) {
        //Aproximate value:
        int32_t id = base_fp16 >> 11; // //step 1/32 so multiple by 32 is reduce shift of 5
        FP_ASSERT(((size_t)id + 1) < sizeof(log1p_tab_fp16) / sizeof(log1p_tab_fp16[0]));
        int32_t rest     = base_fp16 & 0x7FF; //11 bits
        int32_t diff     = ((rest * (log1p_tab_fp16[id + 1] - log1p_tab_fp16[id])) >> 11);
        int32_t val_fp16 = log1p_tab_fp16[id] + diff; // + (rest*(log_tab_fp16[id + 1] - log_tab_fp16[id]))>>11;
        return val_fp16;
    } else {
        //approximation to line(fp16): y=1860*x + 116456
        FP_ASSERT((int64_t)(noise_level_fp16 >> 8) * 1860 < ((int64_t)1 << 31));
        return ((1860 * (noise_level_fp16 >> 8)) >> 8) + 116456;
    }
}

// Calculate decay factor for temporal filtering
static inline void svt_av1_calculate_decay_factor(uint32_t* tf_decay_factor_fp16, int32_t* n_decay_fp10,
                                                  uint32_t q_decay_fp8, int decay_control_cu, int decay_control_cv,
                                                  const int32_t  const_0dot7_fp16,
                                                  const int32_t* noise_levels_log1p_fp16, const uint8_t shift_factor,
                                                  uint8_t tf_chroma) {
    *(tf_decay_factor_fp16 +
      C_Y) = (uint32_t)((((((int64_t)*n_decay_fp10) * ((int64_t)*n_decay_fp10))) * q_decay_fp8) >> shift_factor);

    if (tf_chroma) {
        *n_decay_fp10 = (decay_control_cu * (const_0dot7_fp16 + noise_levels_log1p_fp16[C_U])) / ((int32_t)1 << 6);
        *(tf_decay_factor_fp16 +
          C_U) = (uint32_t)((((((int64_t)*n_decay_fp10) * ((int64_t)*n_decay_fp10))) * q_decay_fp8) >> shift_factor);
        *n_decay_fp10 = (decay_control_cv * (const_0dot7_fp16 + noise_levels_log1p_fp16[C_V])) / ((int32_t)1 << 6);
        *(tf_decay_factor_fp16 +
          C_V) = (uint32_t)((((((int64_t)*n_decay_fp10) * ((int64_t)*n_decay_fp10))) * q_decay_fp8) >> shift_factor);
    }
}

// calculate TF shift based on 64x64 block error
static uint8_t calculate_tf_shift_factor(MeContext* ctx) {
    const uint64_t block_err = ctx->tf_64x64_block_error >> 12;
    // This conditional may benefit from further refinement
    if (block_err < LOW_ERROR_THRESHOLD) {
        return 14;
    } else if (block_err < MED_ERROR_THRESHOLD) {
        return 13;
    }
    return 12; // Default value
}

// clang-format off

// T[X] =  exp(-(X)/16)  for x in [0..7], step 1/16 values in Fixed Points shift 16
static const int32_t expf_tab_fp16[] = {
    65536, 61565, 57835, 54331, 51039, 47947, 45042, 42313, 39749, 37341, 35078, 32953, 30957,
    29081, 27319, 25664, 24109, 22648, 21276, 19987, 18776, 17638, 16570, 15566, 14623, 13737,
    12904, 12122, 11388, 10698, 10050, 9441,  8869,  8331,  7827,  7352,  6907,  6488,  6095,
    5726,  5379,  5053,  4747,  4459,  4189,  3935,  3697,  3473,  3262,  3065,  2879,  2704,
    2541,  2387,  2242,  2106,  1979,  1859,  1746,  1640,  1541,  1447,  1360,  1277,  1200,
    1127,  1059,  995,   934,   878,   824,   774,   728,   683,   642,   603,   566,   532,
    500,   470,   441,   414,   389,   366,   343,   323,   303,   285,   267,   251,   236,
    222,   208,   195,   184,   172,   162,   152,   143,   134,   126,   118,   111,   104,
    98,    92,    86,    81,    76,    72,    67,    63,    59,    56,    52,    49,    46,
    43,    41,    38,    36,    34,    31,    30,    28,    26,    24,    23,    21};

/*value [i:0-15] (sqrt((float)i)*65536.0*/
static const uint32_t sqrt_array_fp16[16] = {0,
                                             65536,
                                             92681,
                                             113511,
                                             131072,
                                             146542,
                                             160529,
                                             173391,
                                             185363,
                                             196608,
                                             207243,
                                             217358,
                                             227023,
                                             236293,
                                             245213,
                                             253819};

/*Calc sqrt linear max error 10%*/
static uint32_t sqrt_fast(uint32_t x) {
    if (x > 15) {
        const int log2_half = svt_log2f(x) >> 1;
        const int mul2      = log2_half << 1;
        int       base      = x >> (mul2 - 2);
        assert(base < 16);
        return sqrt_array_fp16[base] >> (17 - log2_half);
    }
    return sqrt_array_fp16[x] >> 16;
}
//exp(-x) for x in [0..7]
double svt_aom_expf_tab[] = {1,        0.904837, 0.818731, 0.740818, 0.67032,  0.606531, 0.548812, 0.496585,
                     0.449329, 0.40657,  0.367879, 0.332871, 0.301194, 0.272532, 0.246597, 0.22313,
                     0.201896, 0.182683, 0.165299, 0.149569, 0.135335, 0.122456, 0.110803, 0.100259,
                     0.090718, 0.082085, 0.074274, 0.067206, 0.06081,  0.055023, 0.049787, 0.045049,
                     0.040762, 0.036883, 0.033373, 0.030197, 0.027324, 0.024724, 0.022371, 0.020242,
                     0.018316, 0.016573, 0.014996, 0.013569, 0.012277, 0.011109, 0.010052, 0.009095,
                     0.00823,  0.007447, 0.006738, 0.006097, 0.005517, 0.004992, 0.004517, 0.004087,
                     0.003698, 0.003346, 0.003028, 0.002739, 0.002479, 0.002243, 0.002029, 0.001836,
                     0.001662, 0.001503, 0.00136,  0.001231, 0.001114, 0.001008, 0.000912, 0.000825,
                     0.000747, 0.000676, 0.000611, 0.000553, 0.0005,   0.000453, 0.00041,  0.000371,
                     0.000335
};
// clang-format on

/***************************************************************************************************
* Applies temporal filter plane by plane.
* Inputs:
*   y_src, u_src, v_src : Pointers to the frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   block_width: Width of the block.
*   block_height: Height of the block
*   noise_levels: Pointer to the noise levels of the to-filter frame, estimated
*                 with each plane (in Y, U, V order).
*   y_pre, r_pre, v_pre: Pointers to the well-built predictors.
*   accum: Pointer to the pixel-wise accumulator for filtering.
*   count: Pointer to the pixel-wise counter fot filtering.
* Returns:
*   Nothing will be returned. But the content to which `accum` and `pred`
*   point will be modified.
***************************************************************************************************/
/* calculates SSE*/
static uint32_t calculate_squared_errors_sum(const uint8_t* s, int s_stride, const uint8_t* p, int p_stride,
                                             unsigned int w, unsigned int h) {
    unsigned int i, j;
    uint32_t     sum = 0;
    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j++) {
            sum += SQR(s[i * s_stride + j] - p[i * p_stride + j]);
        }
    }
    return sum;
}

/* calculates SSE for 10bit*/
static uint32_t calculate_squared_errors_sum_highbd(const uint16_t* s, int s_stride, const uint16_t* p, int p_stride,
                                                    unsigned int w, unsigned int h, int shift_factor) {
    unsigned int i, j;
    uint32_t     sum = 0;
    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j++) {
            sum += SQR(s[i * s_stride + j] - p[i * p_stride + j]);
        }
    }
    return (sum >> shift_factor);
}

/***************************************************************************************************
* Applies zero motion temporal filter plane by plane.
* Inputs:
*   y_src, u_src, v_src : Pointers to the frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   block_width: Width of the block.
*   block_height: Height of the block
*   noise_levels: Pointer to the noise levels of the to-filter frame, estimated
*                 with each plane (in Y, U, V order).
*   y_pre, r_pre, v_pre: Pointers to the well-built predictors.
*   accum: Pointer to the pixel-wise accumulator for filtering.
*   count: Pointer to the pixel-wise counter fot filtering.
* Returns:
*   Nothing will be returned. But the content to which `accum` and `pred`
*   point will be modified.
***************************************************************************************************/
static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_c(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor_fp16) {
    unsigned int i, j, subblock_idx;

    // Decay factors for non-local mean approach.
    // Larger noise -> larger filtering weight.
    int32_t idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;

    //Calculation for every quarter
    uint32_t block_error_fp8[4];

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        for (i = 0; i < 4; ++i) {
            block_error_fp8[i] = (uint32_t)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i]);
        }
    } else {
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 2);
    }

    //Calculation for every quarter
    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16 = AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor_fp16 >> 10), 1), 7 * 16);
        //int adjusted_weight = (int)(expf((float)(-scaled_diff)) * TF_WEIGHT_SCALE);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17;

        int x_offset = (subblock_idx % 2) * block_width / 2;
        int y_offset = (subblock_idx / 2) * block_height / 2;

        for (i = 0; i < block_height / 2; i++) {
            for (j = 0; j < block_width / 2; j++) {
                const int k           = (i + y_offset) * y_pre_stride + j + x_offset;
                const int pixel_value = y_pre[k];
                y_count[k] += adjusted_weight;
                y_accum[k] += adjusted_weight * pixel_value;
            }
        }
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_c(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_c(me_ctx,
                                                                      y_pre,
                                                                      y_pre_stride,
                                                                      (unsigned int)block_width,
                                                                      (unsigned int)block_height,
                                                                      y_accum,
                                                                      y_count,
                                                                      me_ctx->tf_decay_factor_fp16[C_Y]);
    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_c(me_ctx,
                                                                          u_pre,
                                                                          uv_pre_stride,
                                                                          (unsigned int)block_width >> ss_x,
                                                                          (unsigned int)block_height >> ss_y,
                                                                          u_accum,
                                                                          u_count,
                                                                          me_ctx->tf_decay_factor_fp16[C_U]);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_c(me_ctx,
                                                                          v_pre,
                                                                          uv_pre_stride,
                                                                          (unsigned int)block_width >> ss_x,
                                                                          (unsigned int)block_height >> ss_y,
                                                                          v_accum,
                                                                          v_count,
                                                                          me_ctx->tf_decay_factor_fp16[C_V]);
    }
}

/***************************************************************************************************
* Applies zero motion based temporal filter plane by plane for hbd
* Inputs:
*   y_src, u_src, v_src : Pointers to the frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   block_width: Width of the block.
*   block_height: Height of the block
*   noise_levels: Pointer to the noise levels of the to-filter frame, estimated
*                 with each plane (in Y, U, V order).
*   y_pre, r_pre, v_pre: Pointers to the well-built predictors.
*   accum: Pointer to the pixel-wise accumulator for filtering.
*   count: Pointer to the pixel-wise counter fot filtering.
* Returns:
*   Nothing will be returned. But the content to which `accum` and `pred`
*   point will be modified.
***************************************************************************************************/
static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_c(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor_fp16, uint32_t encoder_bit_depth) {
    unsigned int i, j, subblock_idx;
    // Decay factors for non-local mean approach.
    // Larger noise -> larger filtering weight.
    int idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
    (void)encoder_bit_depth;

    //Calculation for every quarter
    uint32_t block_error_fp8[4];
    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        for (i = 0; i < 4; ++i) {
            block_error_fp8[i] = (uint32_t)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] >> 4);
        }
    } else {
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 6);
    }

    //Calculation for every quarter
    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16 = AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor_fp16 >> 10), 1), 7 * 16);
        //int adjusted_weight = (int)(expf((float)(-scaled_diff)) * TF_WEIGHT_SCALE);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17;

        int x_offset = (subblock_idx % 2) * block_width / 2;
        int y_offset = (subblock_idx / 2) * block_height / 2;

        for (i = 0; i < block_height / 2; i++) {
            for (j = 0; j < block_width / 2; j++) {
                const int k           = (i + y_offset) * y_pre_stride + j + x_offset;
                const int pixel_value = y_pre[k];
                y_count[k] += adjusted_weight;
                y_accum[k] += adjusted_weight * pixel_value;
            }
        }
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_c(me_ctx,
                                                                          y_pre,
                                                                          y_pre_stride,
                                                                          (unsigned int)block_width,
                                                                          (unsigned int)block_height,
                                                                          y_accum,
                                                                          y_count,
                                                                          me_ctx->tf_decay_factor_fp16[C_Y],
                                                                          encoder_bit_depth);

    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_c(me_ctx,
                                                                              u_pre,
                                                                              uv_pre_stride,
                                                                              (unsigned int)block_width >> ss_x,
                                                                              (unsigned int)block_height >> ss_y,
                                                                              u_accum,
                                                                              u_count,
                                                                              me_ctx->tf_decay_factor_fp16[C_U],
                                                                              encoder_bit_depth);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_c(me_ctx,
                                                                              v_pre,
                                                                              uv_pre_stride,
                                                                              (unsigned int)block_width >> ss_x,
                                                                              (unsigned int)block_height >> ss_y,
                                                                              v_accum,
                                                                              v_count,
                                                                              me_ctx->tf_decay_factor_fp16[C_V],
                                                                              encoder_bit_depth);
    }
}

/***************************************************************************************************
* Applies temporal filter plane by plane.
* Inputs:
*   y_src, u_src, v_src : Pointers to the frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   block_width: Width of the block.
*   block_height: Height of the block
*   noise_levels: Pointer to the noise levels of the to-filter frame, estimated
*                 with each plane (in Y, U, V order).
*   y_pre, r_pre, v_pre: Pointers to the well-built predictors.
*   accum: Pointer to the pixel-wise accumulator for filtering.
*   count: Pointer to the pixel-wise counter fot filtering.
* Returns:
*   Nothing will be returned. But the content to which `accum` and `pred`
*   point will be modified.
***************************************************************************************************/
static void svt_av1_apply_temporal_filter_planewise_medium_partial_c(
    MeContext* me_ctx, const uint8_t* y_src, int y_src_stride, const uint8_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count,
    uint32_t tf_decay_factor_fp16, uint32_t luma_window_error_quad_fp8[4], int is_chroma) {
    unsigned int i, j, subblock_idx;

    // Decay factors for non-local mean approach.
    // Larger noise -> larger filtering weight.
    int32_t idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;

    //double distance_threshold = (double)AOMMAX(me_ctx->tf_mv_dist_th * TF_SEARCH_DISTANCE_THRESHOLD, 1);
    uint32_t distance_threshold_fp16 = AOMMAX((me_ctx->tf_mv_dist_th << 16) / 10, 1 << 16);

    //Calculation for every quarter
    uint32_t  d_factor_fp8[4];
    uint32_t  block_error_fp8[4];
    uint32_t  chroma_window_error_quad_fp8[4];
    uint32_t* window_error_quad_fp8 = is_chroma ? chroma_window_error_quad_fp8 : luma_window_error_quad_fp8;

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        for (i = 0; i < 4; ++i) {
            int32_t col = me_ctx->tf_16x16_mv_x[idx_32x32 * 4 + i];
            int32_t row = me_ctx->tf_16x16_mv_y[idx_32x32 * 4 + i];
            //const float  distance = sqrtf((float)col*col + row*row);
            uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
            d_factor_fp8[i]       = AOMMAX((distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
            FP_ASSERT(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] < ((uint64_t)1 << 31));
            //block_error[i] = (double)me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] / 256;
            block_error_fp8[i] = (uint32_t)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i]);
        }
    } else {
        tf_decay_factor_fp16 <<= 1;
        int32_t col = me_ctx->tf_32x32_mv_x[idx_32x32];
        int32_t row = me_ctx->tf_32x32_mv_y[idx_32x32];

        uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
        //d_factor[0] = d_factor[1] = d_factor[2] = d_factor[3] = AOMMAX(distance / distance_threshold, 1);
        d_factor_fp8[0] = d_factor_fp8[1] = d_factor_fp8[2] = d_factor_fp8[3] = AOMMAX(
            (distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
        FP_ASSERT(me_ctx->tf_32x32_block_error[idx_32x32] < ((uint64_t)1 << 30));
        //block_error[0] = block_error[1] = block_error[2] = block_error[3] = (double)me_ctx->tf_32x32_block_error[idx_32x32] / 1024;
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 2);
    }
    const uint32_t bw_half = (block_width >> 1);
    const uint32_t bh_half = (block_height >> 1);
    uint32_t       sum;
    sum = calculate_squared_errors_sum(y_src, y_src_stride, y_pre, y_pre_stride, bw_half, bh_half);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[0] = (((sum << 4) / bw_half) << 4) / bh_half;

    sum = calculate_squared_errors_sum(y_src + bw_half, y_src_stride, y_pre + bw_half, y_pre_stride, bw_half, bh_half);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[1] = (((sum << 4) / bw_half) << 4) / bh_half;

    sum = calculate_squared_errors_sum(y_src + y_src_stride * (bh_half),
                                       y_src_stride,
                                       y_pre + y_pre_stride * (bh_half),
                                       y_pre_stride,
                                       bw_half,
                                       bh_half);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[2] = (((sum << 4) / bw_half) << 4) / bh_half;

    sum = calculate_squared_errors_sum(y_src + y_src_stride * (bh_half) + bw_half,
                                       y_src_stride,
                                       y_pre + y_pre_stride * (bh_half) + bw_half,
                                       y_pre_stride,
                                       bw_half,
                                       bh_half);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[3] = (((sum << 4) / bw_half) << 4) / bh_half;

    if (is_chroma) {
        for (i = 0; i < 4; ++i) {
            FP_ASSERT(((int64_t)window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) < ((int64_t)1 << 31));
            window_error_quad_fp8[i] = (window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) / 6;
        }
    }

    //Calculation for every quarter
    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        FP_ASSERT(((int64_t)window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                   block_error_fp8[subblock_idx]) < ((int64_t)1 << 31));
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10 = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        //double scaled_diff = AOMMIN(combined_error * d_factor[subblock_idx] / (FP2FLOAT(tf_decay_factor_fp16)), 7);
        uint32_t scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor_fp16 >> 10), 1), 7 * 16);
        //int adjusted_weight = (int)(expf((float)(-scaled_diff)) * TF_WEIGHT_SCALE);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        int x_offset = (subblock_idx % 2) * block_width / 2;
        int y_offset = (subblock_idx / 2) * block_height / 2;

        for (i = 0; i < block_height / 2; i++) {
            for (j = 0; j < block_width / 2; j++) {
                const int k           = (i + y_offset) * y_pre_stride + j + x_offset;
                const int pixel_value = y_pre[k];
                y_count[k] += adjusted_weight;
                y_accum[k] += adjusted_weight * pixel_value;
            }
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_c(MeContext* me_ctx, const uint8_t* y_src, int y_src_stride,
                                                      const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_src,
                                                      const uint8_t* v_src, int uv_src_stride, const uint8_t* u_pre,
                                                      const uint8_t* v_pre, int uv_pre_stride, unsigned int block_width,
                                                      unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
                                                      uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count,
                                                      uint32_t* v_accum, uint16_t* v_count) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_partial_c(me_ctx,
                                                             y_src,
                                                             y_src_stride,
                                                             y_pre,
                                                             y_pre_stride,
                                                             (unsigned int)block_width,
                                                             (unsigned int)block_height,
                                                             y_accum,
                                                             y_count,
                                                             me_ctx->tf_decay_factor_fp16[C_Y],
                                                             luma_window_error_quad_fp8,
                                                             0);
    if (me_ctx->tf_chroma) {
        svt_av1_apply_temporal_filter_planewise_medium_partial_c(me_ctx,
                                                                 u_src,
                                                                 uv_src_stride,
                                                                 u_pre,
                                                                 uv_pre_stride,
                                                                 (unsigned int)block_width >> ss_x,
                                                                 (unsigned int)block_height >> ss_y,
                                                                 u_accum,
                                                                 u_count,
                                                                 me_ctx->tf_decay_factor_fp16[C_U],
                                                                 luma_window_error_quad_fp8,
                                                                 1);

        svt_av1_apply_temporal_filter_planewise_medium_partial_c(me_ctx,
                                                                 v_src,
                                                                 uv_src_stride,
                                                                 v_pre,
                                                                 uv_pre_stride,
                                                                 (unsigned int)block_width >> ss_x,
                                                                 (unsigned int)block_height >> ss_y,
                                                                 v_accum,
                                                                 v_count,
                                                                 me_ctx->tf_decay_factor_fp16[C_V],
                                                                 luma_window_error_quad_fp8,
                                                                 1);
    }
}

/***************************************************************************************************
* Applies temporal filter plane by plane for hbd
* Inputs:
*   y_src, u_src, v_src : Pointers to the frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   block_width: Width of the block.
*   block_height: Height of the block
*   noise_levels: Pointer to the noise levels of the to-filter frame, estimated
*                 with each plane (in Y, U, V order).
*   y_pre, r_pre, v_pre: Pointers to the well-built predictors.
*   accum: Pointer to the pixel-wise accumulator for filtering.
*   count: Pointer to the pixel-wise counter fot filtering.
* Returns:
*   Nothing will be returned. But the content to which `accum` and `pred`
*   point will be modified.
***************************************************************************************************/
static void svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_c(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count,
    uint32_t tf_decay_factor_fp16, uint32_t luma_window_error_quad_fp8[4], int is_chroma, uint32_t encoder_bit_depth) {
    unsigned int i, j, subblock_idx;
    // Decay factors for non-local mean approach.
    // Larger noise -> larger filtering weight.
    int idx_32x32    = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
    int shift_factor = ((encoder_bit_depth - 8) * 2);
    //const double distance_threshold = (double)AOMMAX(me_ctx->tf_mv_dist_th * TF_SEARCH_DISTANCE_THRESHOLD, 1);
    uint32_t distance_threshold_fp16 = AOMMAX((me_ctx->tf_mv_dist_th << 16) / 10, 1 << 16);

    //Calculation for every quarter
    uint32_t  d_factor_fp8[4];
    uint32_t  block_error_fp8[4];
    uint32_t  chroma_window_error_quad_fp8[4];
    uint32_t* window_error_quad_fp8 = is_chroma ? chroma_window_error_quad_fp8 : luma_window_error_quad_fp8;

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        for (i = 0; i < 4; ++i) {
            int32_t col = me_ctx->tf_16x16_mv_x[idx_32x32 * 4 + i];
            int32_t row = me_ctx->tf_16x16_mv_y[idx_32x32 * 4 + i];
            //const float  distance = sqrtf((float)col*col + row*row);
            uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
            //d_factor[i] = AOMMAX(distance / distance_threshold, 1);
            d_factor_fp8[i] = AOMMAX((distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
            FP_ASSERT(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] < ((uint64_t)1 << 35));
            //block_error[i] = (double)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] >>4) / 256;
            block_error_fp8[i] = (uint32_t)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] >> 4);
        }
    } else {
        tf_decay_factor_fp16 <<= 1;
        int32_t col = me_ctx->tf_32x32_mv_x[idx_32x32];
        int32_t row = me_ctx->tf_32x32_mv_y[idx_32x32];
        //const float  distance = sqrtf((float)col*col + row*row);
        uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
        //d_factor[i] = AOMMAX(distance / distance_threshold, 1);
        d_factor_fp8[0] = d_factor_fp8[1] = d_factor_fp8[2] = d_factor_fp8[3] = AOMMAX(
            (distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
        FP_ASSERT(me_ctx->tf_32x32_block_error[idx_32x32] < ((uint64_t)1 << 35));
        //= (double)(me_ctx->tf_32x32_block_error[idx_32x32]>> 4) / 1024;
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 6);
    }
    const uint32_t bw_half = (block_width >> 1);
    const uint32_t bh_half = (block_height >> 1);
    uint32_t       sum;

    sum = calculate_squared_errors_sum_highbd(y_src, y_src_stride, y_pre, y_pre_stride, bw_half, bh_half, shift_factor);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[0] = (((sum << 4) / bw_half) << 4) / bh_half;

    sum = calculate_squared_errors_sum_highbd(
        y_src + bw_half, y_src_stride, y_pre + bw_half, y_pre_stride, bw_half, bh_half, shift_factor);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[1] = (((sum << 4) / bw_half) << 4) / bh_half;
    sum                      = calculate_squared_errors_sum_highbd(y_src + y_src_stride * (bh_half),
                                              y_src_stride,
                                              y_pre + y_pre_stride * (bh_half),
                                              y_pre_stride,
                                              bw_half,
                                              bh_half,
                                              shift_factor);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[2] = (((sum << 4) / bw_half) << 4) / bh_half;
    sum                      = calculate_squared_errors_sum_highbd(y_src + y_src_stride * (bh_half) + bw_half,
                                              y_src_stride,
                                              y_pre + y_pre_stride * (bh_half) + bw_half,
                                              y_pre_stride,
                                              bw_half,
                                              bh_half,
                                              shift_factor);
    FP_ASSERT(sum <= (1 << (26)));
    window_error_quad_fp8[3] = (((sum << 4) / bw_half) << 4) / bh_half;

    if (is_chroma) {
        for (i = 0; i < 4; ++i) {
            FP_ASSERT(((int64_t)window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) < ((int64_t)1 << 31));
            window_error_quad_fp8[i] = (window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) / 6;
        }
    }

    //Calculation for every quarter
    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        FP_ASSERT(((int64_t)window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                   block_error_fp8[subblock_idx]) < ((int64_t)1 << 31));
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10 = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        //double scaled_diff = AOMMIN(combined_error * d_factor[subblock_idx] / (FP2FLOAT(tf_decay_factor_fp16)), 7);
        uint32_t scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor_fp16 >> 10), 1), 7 * 16);
        //int adjusted_weight = (int)(expf((float)(-scaled_diff)) * TF_WEIGHT_SCALE);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        int x_offset = (subblock_idx % 2) * block_width / 2;
        int y_offset = (subblock_idx / 2) * block_height / 2;

        for (i = 0; i < block_height / 2; i++) {
            for (j = 0; j < block_width / 2; j++) {
                const int k           = (i + y_offset) * y_pre_stride + j + x_offset;
                const int pixel_value = y_pre[k];
                y_count[k] += adjusted_weight;
                y_accum[k] += adjusted_weight * pixel_value;
            }
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    const uint16_t* u_src, const uint16_t* v_src, int uv_src_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_c(me_ctx,
                                                                 y_src,
                                                                 y_src_stride,
                                                                 y_pre,
                                                                 y_pre_stride,
                                                                 (unsigned int)block_width,
                                                                 (unsigned int)block_height,
                                                                 y_accum,
                                                                 y_count,
                                                                 me_ctx->tf_decay_factor_fp16[C_Y],
                                                                 luma_window_error_quad_fp8,
                                                                 0,
                                                                 encoder_bit_depth);

    if (me_ctx->tf_chroma) {
        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_c(me_ctx,
                                                                     u_src,
                                                                     uv_src_stride,
                                                                     u_pre,
                                                                     uv_pre_stride,
                                                                     (unsigned int)block_width >> ss_x,
                                                                     (unsigned int)block_height >> ss_y,
                                                                     u_accum,
                                                                     u_count,
                                                                     me_ctx->tf_decay_factor_fp16[C_U],
                                                                     luma_window_error_quad_fp8,
                                                                     1,
                                                                     encoder_bit_depth);

        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_c(me_ctx,
                                                                     v_src,
                                                                     uv_src_stride,
                                                                     v_pre,
                                                                     uv_pre_stride,
                                                                     (unsigned int)block_width >> ss_x,
                                                                     (unsigned int)block_height >> ss_y,
                                                                     v_accum,
                                                                     v_count,
                                                                     me_ctx->tf_decay_factor_fp16[C_V],
                                                                     luma_window_error_quad_fp8,
                                                                     1,
                                                                     encoder_bit_depth);
    }
}

/***************************************************************************************************
* Applies temporal filter for each block plane by plane. Passes the right inputs
* for 8 bit  and HBD path
* Inputs:
*   src : Pointer to the 8 bit frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   src_16bit : Pointer to the hbd frame to be filtered, which is used as
*                    reference to compute squared differece from the predictor.
*   block_width: Width of the block.
*   block_height: Height of the block.
*   noise_levels: Pointer to the noise levels of the to-filter frame, estimated
*                 with each plane (in Y, U, V order).
*   pred: Pointers to the well-built 8 bit predictors.
*   pred_16bit: Pointers to the well-built hbd predictors.
*   accum: Pointer to the pixel-wise accumulator for filtering.
*   count: Pointer to the pixel-wise counter fot filtering.
* Returns:
*   Nothing will be returned. But the content to which `accum` and `pred`
*   point will be modified.
***************************************************************************************************/
static void apply_filtering_block_plane_wise(MeContext* me_ctx, int block_row, int block_col, EbByte* src,
                                             uint16_t** src_16bit, EbByte* pred, uint16_t** pred_16bit,
                                             uint32_t** accum, uint16_t** count, uint32_t* stride,
                                             uint32_t* stride_pred, int block_width, int block_height, uint32_t ss_x,
                                             uint32_t ss_y, uint32_t encoder_bit_depth) {
    int blk_h               = block_height;
    int blk_w               = block_width;
    int offset_src_buffer_Y = block_row * blk_h * stride[C_Y] + block_col * blk_w;
    int offset_src_buffer_U = block_row * (blk_h >> ss_y) * stride[C_U] + block_col * (blk_w >> ss_x);
    int offset_src_buffer_V = block_row * (blk_h >> ss_y) * stride[C_V] + block_col * (blk_w >> ss_x);

    int offset_block_buffer_Y = block_row * blk_h * stride_pred[C_Y] + block_col * blk_w;
    int offset_block_buffer_U = block_row * (blk_h >> ss_y) * stride_pred[C_U] + block_col * (blk_w >> ss_x);
    int offset_block_buffer_V = block_row * (blk_h >> ss_y) * stride_pred[C_V] + block_col * (blk_w >> ss_x);

    uint32_t* accum_ptr[COLOR_CHANNELS];
    uint16_t* count_ptr[COLOR_CHANNELS];

    accum_ptr[C_Y] = accum[C_Y] + offset_block_buffer_Y;
    accum_ptr[C_U] = accum[C_U] + offset_block_buffer_U;
    accum_ptr[C_V] = accum[C_V] + offset_block_buffer_V;

    count_ptr[C_Y] = count[C_Y] + offset_block_buffer_Y;
    count_ptr[C_U] = count[C_U] + offset_block_buffer_U;
    count_ptr[C_V] = count[C_V] + offset_block_buffer_V;

    if (encoder_bit_depth == 8) {
        uint8_t* pred_ptr[COLOR_CHANNELS] = {
            pred[C_Y] + offset_block_buffer_Y,
            pred[C_U] + offset_block_buffer_U,
            pred[C_V] + offset_block_buffer_V,
        };
        if (me_ctx->tf_ctrls.use_zz_based_filter) {
            svt_av1_apply_zz_based_temporal_filter_planewise_medium(me_ctx,
                                                                    pred_ptr[C_Y],
                                                                    stride_pred[C_Y],
                                                                    pred_ptr[C_U],
                                                                    pred_ptr[C_V],
                                                                    stride_pred[C_U],
                                                                    (unsigned int)block_width,
                                                                    (unsigned int)block_height,
                                                                    ss_x,
                                                                    ss_y,
                                                                    accum_ptr[C_Y],
                                                                    count_ptr[C_Y],
                                                                    accum_ptr[C_U],
                                                                    count_ptr[C_U],
                                                                    accum_ptr[C_V],
                                                                    count_ptr[C_V]);
        } else {
            uint8_t* src_ptr[COLOR_CHANNELS] = {
                src[C_Y] + offset_src_buffer_Y,
                src[C_U] + offset_src_buffer_U,
                src[C_V] + offset_src_buffer_V,
            };
            svt_av1_apply_temporal_filter_planewise_medium(me_ctx,
                                                           src_ptr[C_Y],
                                                           stride[C_Y],
                                                           pred_ptr[C_Y],
                                                           stride_pred[C_Y],
                                                           src_ptr[C_U],
                                                           src_ptr[C_V],
                                                           stride[C_U],
                                                           pred_ptr[C_U],
                                                           pred_ptr[C_V],
                                                           stride_pred[C_U],
                                                           (unsigned int)block_width,
                                                           (unsigned int)block_height,
                                                           ss_x,
                                                           ss_y,
                                                           accum_ptr[C_Y],
                                                           count_ptr[C_Y],
                                                           accum_ptr[C_U],
                                                           count_ptr[C_U],
                                                           accum_ptr[C_V],
                                                           count_ptr[C_V]);
        }
    } else {
        uint16_t* pred_ptr_16bit[COLOR_CHANNELS] = {
            pred_16bit[C_Y] + offset_block_buffer_Y,
            pred_16bit[C_U] + offset_block_buffer_U,
            pred_16bit[C_V] + offset_block_buffer_V,
        };

        // Apply the temporal filtering strategy
        // TODO(any): avx2 version should also support high bit-depth.
        if (me_ctx->tf_ctrls.use_zz_based_filter) {
            svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd(me_ctx,
                                                                        pred_ptr_16bit[C_Y],
                                                                        stride_pred[C_Y],
                                                                        pred_ptr_16bit[C_U],
                                                                        pred_ptr_16bit[C_V],
                                                                        stride_pred[C_U],
                                                                        (unsigned int)block_width,
                                                                        (unsigned int)block_height,
                                                                        ss_x,
                                                                        ss_y,
                                                                        accum_ptr[C_Y],
                                                                        count_ptr[C_Y],
                                                                        accum_ptr[C_U],
                                                                        count_ptr[C_U],
                                                                        accum_ptr[C_V],
                                                                        count_ptr[C_V],
                                                                        encoder_bit_depth);
        } else {
            uint16_t* src_ptr_16bit[COLOR_CHANNELS] = {
                src_16bit[C_Y] + offset_src_buffer_Y,
                src_16bit[C_U] + offset_src_buffer_U,
                src_16bit[C_V] + offset_src_buffer_V,
            };

            svt_av1_apply_temporal_filter_planewise_medium_hbd(me_ctx,
                                                               src_ptr_16bit[C_Y],
                                                               stride[C_Y],
                                                               pred_ptr_16bit[C_Y],
                                                               stride_pred[C_Y],
                                                               src_ptr_16bit[C_U],
                                                               src_ptr_16bit[C_V],
                                                               stride[C_U],
                                                               pred_ptr_16bit[C_U],
                                                               pred_ptr_16bit[C_V],
                                                               stride_pred[C_U],
                                                               (unsigned int)block_width,
                                                               (unsigned int)block_height,
                                                               ss_x,
                                                               ss_y,
                                                               accum_ptr[C_Y],
                                                               count_ptr[C_Y],
                                                               accum_ptr[C_U],
                                                               count_ptr[C_U],
                                                               accum_ptr[C_V],
                                                               count_ptr[C_V],
                                                               encoder_bit_depth);
        }
    }
}

/*
 * Perform compensation and compute variance for a single block; used in TF subpel search.
 * If the searched MV has a better distortion than the passed best_dist, update best_mv_x,
 * best_mv_y, and best_dist.
 */
static void svt_check_position(TF_SUBPEL_SEARCH_PARAMS* tf_sp_param, PictureParentControlSet* pcs, MeContext* me_ctx,
                               BlkStruct* blk_ptr, EbPictureBufferDesc* pic_ptr_ref,
                               EbPictureBufferDesc* prediction_ptr, EbByte* pred, uint16_t** pred_16bit,
                               uint32_t* stride_pred, EbByte* src, uint16_t** src_16bit, uint32_t* stride_src,
                               uint64_t* best_dist, int16_t* best_mv_x, int16_t* best_mv_y) {
    if (tf_sp_param->subpel_pel_mode >= 2 && tf_sp_param->xd != 0 && tf_sp_param->yd != 0) {
        return;
    }

    // If the best distortion is already 0, the new point cannot beat it, so no need to test
    if (*best_dist == 0) {
        return;
    }

    // If previously checked position is good enough then quit
    if (me_ctx->tf_subpel_early_exit_th) {
        uint64_t dist = *best_dist;
        if (dist <
            (((tf_sp_param->bsize * tf_sp_param->bsize) * me_ctx->tf_subpel_early_exit_th) << tf_sp_param->is_highbd)) {
            return;
        }
    }

    SequenceControlSet* scs = pcs->scs;
    Mv                  mv;
    mv.x = tf_sp_param->mv_x + tf_sp_param->xd;
    mv.y = tf_sp_param->mv_y + tf_sp_param->yd;

    svt_aom_simple_luma_unipred(scs,
                                scs->sf_identity,
                                tf_sp_param->interp_filters,
                                blk_ptr,
                                mv,
                                tf_sp_param->pu_origin_x,
                                tf_sp_param->pu_origin_y,
                                tf_sp_param->bsize,
                                tf_sp_param->bsize,
                                pic_ptr_ref,
                                prediction_ptr,
                                tf_sp_param->local_origin_x,
                                tf_sp_param->local_origin_y,
                                (uint8_t)tf_sp_param->encoder_bit_depth,
                                tf_sp_param->xd == 0 && tf_sp_param->yd == 0 ? tf_sp_param->subsampling_shift : 0);

    BlockSize block_size = BLOCK_64X64;
    switch (tf_sp_param->bsize) {
    case 64:
        block_size = tf_sp_param->subsampling_shift ? BLOCK_64X32 : BLOCK_64X64;
        break;
    case 32:
        block_size = tf_sp_param->subsampling_shift ? BLOCK_32X16 : BLOCK_32X32;
        break;
    case 16:
        block_size = tf_sp_param->subsampling_shift ? BLOCK_16X8 : BLOCK_16X16;
        break;
    case 8:
        block_size = tf_sp_param->subsampling_shift ? BLOCK_8X4 : BLOCK_8X8;
        break;
    default:
        assert(0);
    }
    uint64_t distortion;
    if (!tf_sp_param->is_highbd) {
        uint8_t* pred_y_ptr = pred[C_Y] + tf_sp_param->bsize * tf_sp_param->idx_y * stride_pred[C_Y] +
            tf_sp_param->bsize * tf_sp_param->idx_x;
        uint8_t* src_y_ptr = src[C_Y] + tf_sp_param->bsize * tf_sp_param->idx_y * stride_src[C_Y] +
            tf_sp_param->bsize * tf_sp_param->idx_x;
        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[block_size];
        unsigned int            sse;
        distortion = fn_ptr->vf(pred_y_ptr,
                                stride_pred[C_Y] << tf_sp_param->subsampling_shift,
                                src_y_ptr,
                                stride_src[C_Y] << tf_sp_param->subsampling_shift,
                                &sse)
            << tf_sp_param->subsampling_shift;
    } else {
        uint16_t* pred_y_ptr = pred_16bit[C_Y] + tf_sp_param->bsize * tf_sp_param->idx_y * stride_pred[C_Y] +
            tf_sp_param->bsize * tf_sp_param->idx_x;
        uint16_t* src_y_ptr = src_16bit[C_Y] + tf_sp_param->bsize * tf_sp_param->idx_y * stride_src[C_Y] +
            tf_sp_param->bsize * tf_sp_param->idx_x;
        const AomVarianceFnPtr* fn_ptr = &svt_aom_mefn_ptr[block_size];

        unsigned int sse;

        distortion = fn_ptr->vf_hbd_10(CONVERT_TO_BYTEPTR(pred_y_ptr),
                                       stride_pred[C_Y] << tf_sp_param->subsampling_shift,
                                       CONVERT_TO_BYTEPTR(src_y_ptr),
                                       stride_src[C_Y] << tf_sp_param->subsampling_shift,
                                       &sse)
            << tf_sp_param->subsampling_shift;
    }

    // If the new point is better than the old, update MV and distortion
    if (distortion < *best_dist) {
        *best_dist = distortion;
        *best_mv_x = mv.x;
        *best_mv_y = mv.y;
    }
}

/*
 * Perform subpel search for a given block.  The starting MV is passed as
 * (best_mv_x, best_mv_y).  The starting point will always be searched, so the
 * starting distortion (passed through best_dist) can be MAX.
 */
static void tf_subpel_search(TF_SUBPEL_SEARCH_PARAMS* tf_sp_param, PictureParentControlSet* pcs, MeContext* me_ctx,
                             BlkStruct* blk_ptr, EbPictureBufferDesc* pic_ptr_ref, EbPictureBufferDesc* prediction_ptr,
                             EbByte* pred, uint16_t** pred_16bit, uint32_t* stride_pred, EbByte* src,
                             uint16_t** src_16bit, uint32_t* stride_src, uint64_t* best_dist, int16_t* best_mv_x,
                             int16_t* best_mv_y) {
    // Check centre position
    tf_sp_param->subpel_pel_mode = pcs->tf_ctrls.half_pel_mode;
    tf_sp_param->mv_x            = *best_mv_x;
    tf_sp_param->mv_y            = *best_mv_y;
    tf_sp_param->xd              = 0;
    tf_sp_param->yd              = 0;
    svt_check_position(tf_sp_param,
                       pcs,
                       me_ctx,
                       blk_ptr,
                       pic_ptr_ref,
                       prediction_ptr,
                       pred,
                       pred_16bit,
                       stride_pred,
                       src,
                       src_16bit,
                       stride_src,
                       best_dist,
                       best_mv_x,
                       best_mv_y);

    // Perform 1/2 Pel MV Refinement
    if (pcs->tf_ctrls.half_pel_mode) {
        tf_sp_param->subpel_pel_mode = pcs->tf_ctrls.half_pel_mode;
        tf_sp_param->mv_x            = *best_mv_x;
        tf_sp_param->mv_y            = *best_mv_y;
        for (signed short i = -4; i <= 4; i = i + 4) {
            for (signed short j = -4; j <= 4; j = j + 4) {
                if (i == 0 && j == 0) { // point already searched
                    continue;
                }

                tf_sp_param->xd = i;
                tf_sp_param->yd = j;
                svt_check_position(tf_sp_param,
                                   pcs,
                                   me_ctx,
                                   blk_ptr,
                                   pic_ptr_ref,
                                   prediction_ptr,
                                   pred,
                                   pred_16bit,
                                   stride_pred,
                                   src,
                                   src_16bit,
                                   stride_src,
                                   best_dist,
                                   best_mv_x,
                                   best_mv_y);
            }
        }
    }

    // Perform 1/4 Pel MV Refinement
    if (pcs->tf_ctrls.quarter_pel_mode) {
        tf_sp_param->subpel_pel_mode = pcs->tf_ctrls.quarter_pel_mode;
        tf_sp_param->mv_x            = *best_mv_x;
        tf_sp_param->mv_y            = *best_mv_y;
        for (signed short i = -2; i <= 2; i = i + 2) {
            for (signed short j = -2; j <= 2; j = j + 2) {
                if (i == 0 && j == 0) { // point already searched
                    continue;
                }

                tf_sp_param->xd = i;
                tf_sp_param->yd = j;
                svt_check_position(tf_sp_param,
                                   pcs,
                                   me_ctx,
                                   blk_ptr,
                                   pic_ptr_ref,
                                   prediction_ptr,
                                   pred,
                                   pred_16bit,
                                   stride_pred,
                                   src,
                                   src_16bit,
                                   stride_src,
                                   best_dist,
                                   best_mv_x,
                                   best_mv_y);
            }
        }
    }

    // Perform 1/8 Pel MV Refinement
    if (pcs->tf_ctrls.eight_pel_mode) {
        tf_sp_param->subpel_pel_mode = pcs->tf_ctrls.eight_pel_mode;
        tf_sp_param->mv_x            = *best_mv_x;
        tf_sp_param->mv_y            = *best_mv_y;
        for (signed short i = -1; i <= 1; i++) {
            for (signed short j = -1; j <= 1; j++) {
                if (i == 0 && j == 0) { // point already searched
                    continue;
                }

                tf_sp_param->xd = i;
                tf_sp_param->yd = j;
                svt_check_position(tf_sp_param,
                                   pcs,
                                   me_ctx,
                                   blk_ptr,
                                   pic_ptr_ref,
                                   prediction_ptr,
                                   pred,
                                   pred_16bit,
                                   stride_pred,
                                   src,
                                   src_16bit,
                                   stride_src,
                                   best_dist,
                                   best_mv_x,
                                   best_mv_y);
            }
        }
    }
}

static void tf_64x64_sub_pel_search(PictureParentControlSet* pcs, MeContext* me_ctx, PictureParentControlSet* pcs_ref,
                                    EbPictureBufferDesc* pic_ptr_ref, EbByte* pred, uint16_t** pred_16bit,
                                    uint32_t* stride_pred, EbByte* src, uint16_t** src_16bit, uint32_t* stride_src,
                                    uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t ss_x, int encoder_bit_depth) {
    InterpFilters interp_filters;
    if (me_ctx->tf_ctrls.use_2tap) {
        interp_filters = av1_make_interp_filters(BILINEAR, BILINEAR);
    } else {
        interp_filters = av1_make_interp_filters(EIGHTTAP_REGULAR, EIGHTTAP_REGULAR);
    }

    bool        is_highbd = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;
    BlkStruct   blk_struct;
    MacroBlockD av1xd;
    blk_struct.av1xd = &av1xd;
    EbPictureBufferDesc reference_ptr;
    EbPictureBufferDesc prediction_ptr;

    prediction_ptr.org_x     = 0;
    prediction_ptr.org_y     = 0;
    prediction_ptr.stride_y  = BW;
    prediction_ptr.stride_cb = (uint16_t)BW >> ss_x;
    prediction_ptr.stride_cr = (uint16_t)BW >> ss_x;
    if (!is_highbd) {
        assert(src[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src[C_V] != NULL));
        prediction_ptr.buffer_y  = pred[C_Y];
        prediction_ptr.buffer_cb = pred[C_U];
        prediction_ptr.buffer_cr = pred[C_V];
    } else {
        assert(src_16bit[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_V] != NULL));
        prediction_ptr.buffer_y         = (uint8_t*)pred_16bit[C_Y];
        prediction_ptr.buffer_cb        = (uint8_t*)pred_16bit[C_U];
        prediction_ptr.buffer_cr        = (uint8_t*)pred_16bit[C_V];
        reference_ptr.buffer_y          = (uint8_t*)pcs_ref->altref_buffer_highbd[C_Y];
        reference_ptr.buffer_cb         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_U];
        reference_ptr.buffer_cr         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_V];
        reference_ptr.org_x             = pic_ptr_ref->org_x;
        reference_ptr.org_y             = pic_ptr_ref->org_y;
        reference_ptr.stride_y          = pic_ptr_ref->stride_y;
        reference_ptr.stride_cb         = pic_ptr_ref->stride_cb;
        reference_ptr.stride_cr         = pic_ptr_ref->stride_cr;
        reference_ptr.width             = pic_ptr_ref->width;
        reference_ptr.height            = pic_ptr_ref->height;
        reference_ptr.buffer_bit_inc_y  = NULL;
        reference_ptr.buffer_bit_inc_cb = NULL;
        reference_ptr.buffer_bit_inc_cr = NULL;
    }
    uint32_t bsize = 64;

    uint16_t local_origin_x = 0;
    uint16_t local_origin_y = 0;
    uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
    uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
    int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
    int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

    const int32_t bw                    = mi_size_wide[BLOCK_64X64];
    const int32_t bh                    = mi_size_high[BLOCK_64X64];
    blk_struct.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
    blk_struct.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
    blk_struct.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
    blk_struct.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;
    BlkStruct* blk_ptr                  = &blk_struct;

    // Set the starting MV and distortion
    me_ctx->tf_64x64_block_error = INT_MAX;
    me_ctx->tf_64x64_mv_x        = (me_ctx->tf_use_pred_64x64_only_th == (uint8_t)~0)
               ? me_ctx->search_results[0][0].hme_sc_x << 3
               : (_MVXT(me_ctx->p_best_mv64x64[0])) << 3;
    me_ctx->tf_64x64_mv_y        = (me_ctx->tf_use_pred_64x64_only_th == (uint8_t)~0)
               ? me_ctx->search_results[0][0].hme_sc_y << 3
               : (_MVYT(me_ctx->p_best_mv64x64[0])) << 3;

    TF_SUBPEL_SEARCH_PARAMS tf_sp_param;
    tf_sp_param.subsampling_shift = pcs->tf_ctrls.sub_sampling_shift;
    tf_sp_param.interp_filters    = (uint32_t)interp_filters;
    tf_sp_param.pu_origin_x       = pu_origin_x;
    tf_sp_param.pu_origin_y       = pu_origin_y;
    tf_sp_param.local_origin_x    = local_origin_x;
    tf_sp_param.local_origin_y    = local_origin_y;
    tf_sp_param.bsize             = bsize;
    tf_sp_param.is_highbd         = is_highbd;
    tf_sp_param.encoder_bit_depth = encoder_bit_depth;
    tf_sp_param.idx_x             = 0;
    tf_sp_param.idx_y             = 0;

    // Check centre position
    tf_subpel_search(&tf_sp_param,
                     pcs,
                     me_ctx,
                     blk_ptr,
                     !is_highbd ? pic_ptr_ref : &reference_ptr,
                     &prediction_ptr,
                     pred,
                     pred_16bit,
                     stride_pred,
                     src,
                     src_16bit,
                     stride_src,
                     &me_ctx->tf_64x64_block_error,
                     &me_ctx->tf_64x64_mv_x,
                     &me_ctx->tf_64x64_mv_y);
}

static void tf_32x32_sub_pel_search(PictureParentControlSet* pcs, MeContext* me_ctx, PictureParentControlSet* pcs_ref,
                                    EbPictureBufferDesc* pic_ptr_ref, EbByte* pred, uint16_t** pred_16bit,
                                    uint32_t* stride_pred, EbByte* src, uint16_t** src_16bit, uint32_t* stride_src,
                                    uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t ss_x, int encoder_bit_depth) {
    InterpFilters interp_filters;
    if (me_ctx->tf_ctrls.use_2tap) {
        interp_filters = av1_make_interp_filters(BILINEAR, BILINEAR);
    } else {
        interp_filters = av1_make_interp_filters(EIGHTTAP_REGULAR, EIGHTTAP_REGULAR);
    }

    bool        is_highbd = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;
    BlkStruct   blk_struct;
    MacroBlockD av1xd;
    blk_struct.av1xd = &av1xd;
    EbPictureBufferDesc reference_ptr;
    EbPictureBufferDesc prediction_ptr;

    prediction_ptr.org_x     = 0;
    prediction_ptr.org_y     = 0;
    prediction_ptr.stride_y  = BW;
    prediction_ptr.stride_cb = (uint16_t)BW >> ss_x;
    prediction_ptr.stride_cr = (uint16_t)BW >> ss_x;
    if (!is_highbd) {
        assert(src[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src[C_V] != NULL));
        prediction_ptr.buffer_y  = pred[C_Y];
        prediction_ptr.buffer_cb = pred[C_U];
        prediction_ptr.buffer_cr = pred[C_V];
    } else {
        assert(src_16bit[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_V] != NULL));
        prediction_ptr.buffer_y         = (uint8_t*)pred_16bit[C_Y];
        prediction_ptr.buffer_cb        = (uint8_t*)pred_16bit[C_U];
        prediction_ptr.buffer_cr        = (uint8_t*)pred_16bit[C_V];
        reference_ptr.buffer_y          = (uint8_t*)pcs_ref->altref_buffer_highbd[C_Y];
        reference_ptr.buffer_cb         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_U];
        reference_ptr.buffer_cr         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_V];
        reference_ptr.org_x             = pic_ptr_ref->org_x;
        reference_ptr.org_y             = pic_ptr_ref->org_y;
        reference_ptr.stride_y          = pic_ptr_ref->stride_y;
        reference_ptr.stride_cb         = pic_ptr_ref->stride_cb;
        reference_ptr.stride_cr         = pic_ptr_ref->stride_cr;
        reference_ptr.width             = pic_ptr_ref->width;
        reference_ptr.height            = pic_ptr_ref->height;
        reference_ptr.buffer_bit_inc_y  = NULL;
        reference_ptr.buffer_bit_inc_cb = NULL;
        reference_ptr.buffer_bit_inc_cr = NULL;
    }
    uint32_t bsize          = 32;
    uint32_t idx_32x32      = me_ctx->idx_32x32;
    uint32_t idx_x          = idx_32x32 & 0x1;
    uint32_t idx_y          = idx_32x32 >> 1;
    uint16_t local_origin_x = idx_x * bsize;
    uint16_t local_origin_y = idx_y * bsize;
    uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
    uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
    int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
    int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

    const int32_t bw                    = mi_size_wide[BLOCK_32X32];
    const int32_t bh                    = mi_size_high[BLOCK_32X32];
    blk_struct.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
    blk_struct.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
    blk_struct.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
    blk_struct.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;
    BlkStruct* blk_ptr                  = &blk_struct;

    const uint32_t mv_index = idx_32x32;
    // Set starting MV and distortion
    // AV1 MVs are always in 1/8th pel precision.
    me_ctx->tf_32x32_block_error[idx_32x32] = INT_MAX;
    me_ctx->tf_32x32_mv_x[idx_32x32]        = (_MVXT(me_ctx->p_best_mv32x32[mv_index])) << 3;
    me_ctx->tf_32x32_mv_y[idx_32x32]        = (_MVYT(me_ctx->p_best_mv32x32[mv_index])) << 3;

    TF_SUBPEL_SEARCH_PARAMS tf_sp_param;
    tf_sp_param.subsampling_shift = pcs->tf_ctrls.sub_sampling_shift;
    tf_sp_param.interp_filters    = (uint32_t)interp_filters;
    tf_sp_param.pu_origin_x       = pu_origin_x;
    tf_sp_param.pu_origin_y       = pu_origin_y;
    tf_sp_param.local_origin_x    = local_origin_x;
    tf_sp_param.local_origin_y    = local_origin_y;
    tf_sp_param.bsize             = bsize;
    tf_sp_param.is_highbd         = is_highbd;
    tf_sp_param.encoder_bit_depth = encoder_bit_depth;
    tf_sp_param.idx_x             = idx_x;
    tf_sp_param.idx_y             = idx_y;

    // Perform subpel search for this block
    tf_subpel_search(&tf_sp_param,
                     pcs,
                     me_ctx,
                     blk_ptr,
                     !is_highbd ? pic_ptr_ref : &reference_ptr,
                     &prediction_ptr,
                     pred,
                     pred_16bit,
                     stride_pred,
                     src,
                     src_16bit,
                     stride_src,
                     &me_ctx->tf_32x32_block_error[me_ctx->idx_32x32],
                     &me_ctx->tf_32x32_mv_x[idx_32x32],
                     &me_ctx->tf_32x32_mv_y[idx_32x32]);
}

static void tf_16x16_sub_pel_search(PictureParentControlSet* pcs, MeContext* me_ctx, PictureParentControlSet* pcs_ref,
                                    EbPictureBufferDesc* pic_ptr_ref, EbByte* pred, uint16_t** pred_16bit,
                                    uint32_t* stride_pred, EbByte* src, uint16_t** src_16bit, uint32_t* stride_src,
                                    uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t ss_x, int encoder_bit_depth) {
    InterpFilters interp_filters = av1_make_interp_filters(EIGHTTAP_REGULAR, EIGHTTAP_REGULAR);

    bool is_highbd = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;

    BlkStruct   blk_ptr;
    MacroBlockD av1xd;
    blk_ptr.av1xd = &av1xd;

    EbPictureBufferDesc reference_ptr;
    EbPictureBufferDesc prediction_ptr;

    UNUSED(ss_x);

    prediction_ptr.org_x     = 0;
    prediction_ptr.org_y     = 0;
    prediction_ptr.stride_y  = BW;
    prediction_ptr.stride_cb = (uint16_t)BW >> ss_x;
    prediction_ptr.stride_cr = (uint16_t)BW >> ss_x;

    if (!is_highbd) {
        assert(src[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src[C_V] != NULL));
        prediction_ptr.buffer_y  = pred[C_Y];
        prediction_ptr.buffer_cb = pred[C_U];
        prediction_ptr.buffer_cr = pred[C_V];
    } else {
        assert(src_16bit[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_V] != NULL));
        prediction_ptr.buffer_y  = (uint8_t*)pred_16bit[C_Y];
        prediction_ptr.buffer_cb = (uint8_t*)pred_16bit[C_U];
        prediction_ptr.buffer_cr = (uint8_t*)pred_16bit[C_V];

        reference_ptr.buffer_y          = (uint8_t*)pcs_ref->altref_buffer_highbd[C_Y];
        reference_ptr.buffer_cb         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_U];
        reference_ptr.buffer_cr         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_V];
        reference_ptr.org_x             = pic_ptr_ref->org_x;
        reference_ptr.org_y             = pic_ptr_ref->org_y;
        reference_ptr.stride_y          = pic_ptr_ref->stride_y;
        reference_ptr.stride_cb         = pic_ptr_ref->stride_cb;
        reference_ptr.stride_cr         = pic_ptr_ref->stride_cr;
        reference_ptr.width             = pic_ptr_ref->width;
        reference_ptr.height            = pic_ptr_ref->height;
        reference_ptr.buffer_bit_inc_y  = NULL;
        reference_ptr.buffer_bit_inc_cb = NULL;
        reference_ptr.buffer_bit_inc_cr = NULL;
    }

    uint32_t bsize     = 16;
    uint32_t idx_32x32 = me_ctx->idx_32x32;
    for (uint32_t idx_16x16 = 0; idx_16x16 < 4; idx_16x16++) {
        uint32_t pu_index = idx_32x32_to_idx_16x16[idx_32x32][idx_16x16];

        uint32_t idx_y          = subblock_xy_16x16[pu_index][0];
        uint32_t idx_x          = subblock_xy_16x16[pu_index][1];
        uint16_t local_origin_x = idx_x * bsize;
        uint16_t local_origin_y = idx_y * bsize;
        uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
        uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
        int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
        int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

        const int32_t bw                 = mi_size_wide[BLOCK_16X16];
        const int32_t bh                 = mi_size_high[BLOCK_16X16];
        blk_ptr.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
        blk_ptr.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
        blk_ptr.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
        blk_ptr.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;

        TF_SUBPEL_SEARCH_PARAMS tf_sp_param;
        tf_sp_param.subsampling_shift = pcs->tf_ctrls.sub_sampling_shift;
        tf_sp_param.interp_filters    = (uint32_t)interp_filters;
        tf_sp_param.pu_origin_x       = pu_origin_x;
        tf_sp_param.pu_origin_y       = pu_origin_y;
        tf_sp_param.local_origin_x    = local_origin_x;
        tf_sp_param.local_origin_y    = local_origin_y;
        tf_sp_param.bsize             = bsize;
        tf_sp_param.is_highbd         = is_highbd;
        tf_sp_param.encoder_bit_depth = encoder_bit_depth;
        tf_sp_param.idx_x             = idx_x;
        tf_sp_param.idx_y             = idx_y;

        const uint32_t mv_index = tab16x16[pu_index];
        // Set starting MV and distortion
        me_ctx->tf_16x16_block_error[idx_32x32 * 4 + idx_16x16] = INT_MAX;
        // AV1 MVs are always in 1/8th pel precision.
        me_ctx->tf_16x16_mv_x[idx_32x32 * 4 + idx_16x16] = (_MVXT(me_ctx->p_best_mv16x16[mv_index])) << 3;
        me_ctx->tf_16x16_mv_y[idx_32x32 * 4 + idx_16x16] = (_MVYT(me_ctx->p_best_mv16x16[mv_index])) << 3;

        // Perform subpel search for the block
        tf_subpel_search(&tf_sp_param,
                         pcs,
                         me_ctx,
                         &blk_ptr,
                         !is_highbd ? pic_ptr_ref : &reference_ptr,
                         &prediction_ptr,
                         pred,
                         pred_16bit,
                         stride_pred,
                         src,
                         src_16bit,
                         stride_src,
                         &me_ctx->tf_16x16_block_error[idx_32x32 * 4 + idx_16x16],
                         &me_ctx->tf_16x16_mv_x[idx_32x32 * 4 + idx_16x16],
                         &me_ctx->tf_16x16_mv_y[idx_32x32 * 4 + idx_16x16]);
    }
}

static void tf_8x8_sub_pel_search(PictureParentControlSet* pcs, MeContext* me_ctx, PictureParentControlSet* pcs_ref,
                                  EbPictureBufferDesc* pic_ptr_ref, EbByte* pred, uint16_t** pred_16bit,
                                  uint32_t* stride_pred, EbByte* src, uint16_t** src_16bit, uint32_t* stride_src,
                                  uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t ss_x, int encoder_bit_depth) {
    InterpFilters interp_filters = av1_make_interp_filters(EIGHTTAP_REGULAR, EIGHTTAP_REGULAR);

    bool is_highbd = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;

    BlkStruct   blk_ptr;
    MacroBlockD av1xd;
    blk_ptr.av1xd = &av1xd;

    EbPictureBufferDesc reference_ptr;
    EbPictureBufferDesc prediction_ptr;

    prediction_ptr.org_x     = 0;
    prediction_ptr.org_y     = 0;
    prediction_ptr.stride_y  = BW;
    prediction_ptr.stride_cb = (uint16_t)BW >> ss_x;
    prediction_ptr.stride_cr = (uint16_t)BW >> ss_x;

    if (!is_highbd) {
        assert(src[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src[C_V] != NULL));
        prediction_ptr.buffer_y  = pred[C_Y];
        prediction_ptr.buffer_cb = pred[C_U];
        prediction_ptr.buffer_cr = pred[C_V];
    } else {
        assert(src_16bit[C_Y] != NULL);
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_U] != NULL));
        assert(IMPLIES(me_ctx->tf_chroma, src_16bit[C_V] != NULL));
        prediction_ptr.buffer_y  = (uint8_t*)pred_16bit[C_Y];
        prediction_ptr.buffer_cb = (uint8_t*)pred_16bit[C_U];
        prediction_ptr.buffer_cr = (uint8_t*)pred_16bit[C_V];

        reference_ptr.buffer_y          = (uint8_t*)pcs_ref->altref_buffer_highbd[C_Y];
        reference_ptr.buffer_cb         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_U];
        reference_ptr.buffer_cr         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_V];
        reference_ptr.org_x             = pic_ptr_ref->org_x;
        reference_ptr.org_y             = pic_ptr_ref->org_y;
        reference_ptr.stride_y          = pic_ptr_ref->stride_y;
        reference_ptr.stride_cb         = pic_ptr_ref->stride_cb;
        reference_ptr.stride_cr         = pic_ptr_ref->stride_cr;
        reference_ptr.width             = pic_ptr_ref->width;
        reference_ptr.height            = pic_ptr_ref->height;
        reference_ptr.buffer_bit_inc_y  = NULL;
        reference_ptr.buffer_bit_inc_cb = NULL;
        reference_ptr.buffer_bit_inc_cr = NULL;
    }

    uint32_t bsize     = 8;
    uint32_t idx_32x32 = me_ctx->idx_32x32;
    for (uint32_t idx_16x16 = 0; idx_16x16 < 4; idx_16x16++) {
        for (uint32_t idx_8x8 = 0; idx_8x8 < 4; idx_8x8++) {
            uint32_t pu_index = idx_32x32_to_idx_8x8[idx_32x32][idx_16x16][idx_8x8];

            uint32_t idx_y          = subblock_xy_8x8[pu_index][0];
            uint32_t idx_x          = subblock_xy_8x8[pu_index][1];
            uint16_t local_origin_x = idx_x * bsize;
            uint16_t local_origin_y = idx_y * bsize;
            uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
            uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
            int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
            int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

            const int32_t bw                 = mi_size_wide[BLOCK_8X8];
            const int32_t bh                 = mi_size_high[BLOCK_8X8];
            blk_ptr.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
            blk_ptr.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
            blk_ptr.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
            blk_ptr.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;

            TF_SUBPEL_SEARCH_PARAMS tf_sp_param;
            tf_sp_param.subsampling_shift = pcs->tf_ctrls.sub_sampling_shift;
            tf_sp_param.interp_filters    = (uint32_t)interp_filters;
            tf_sp_param.pu_origin_x       = pu_origin_x;
            tf_sp_param.pu_origin_y       = pu_origin_y;
            tf_sp_param.local_origin_x    = local_origin_x;
            tf_sp_param.local_origin_y    = local_origin_y;
            tf_sp_param.bsize             = bsize;
            tf_sp_param.is_highbd         = is_highbd;
            tf_sp_param.encoder_bit_depth = encoder_bit_depth;
            tf_sp_param.idx_x             = idx_x;
            tf_sp_param.idx_y             = idx_y;

            const uint32_t mv_index = tab8x8[pu_index];
            // Set starting MV and distortion
            me_ctx->tf_8x8_block_error[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8] = INT_MAX;
            // AV1 MVs are always in 1/8th pel precision.
            me_ctx->tf_8x8_mv_x[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8] = (_MVXT(me_ctx->p_best_mv8x8[mv_index]))
                << 3;
            me_ctx->tf_8x8_mv_y[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8] = (_MVYT(me_ctx->p_best_mv8x8[mv_index]))
                << 3;

            // Search subpel for this block
            tf_subpel_search(&tf_sp_param,
                             pcs,
                             me_ctx,
                             &blk_ptr,
                             !is_highbd ? pic_ptr_ref : &reference_ptr,
                             &prediction_ptr,
                             pred,
                             pred_16bit,
                             stride_pred,
                             src,
                             src_16bit,
                             stride_src,
                             &me_ctx->tf_8x8_block_error[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8],
                             &me_ctx->tf_8x8_mv_x[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8],
                             &me_ctx->tf_8x8_mv_y[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8]);
        }
    }
}

static void tf_64x64_inter_prediction(PictureParentControlSet* pcs, MeContext* me_ctx, PictureParentControlSet* pcs_ref,
                                      EbPictureBufferDesc* pic_ptr_ref, EbByte* pred, uint16_t** pred_16bit,
                                      uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t ss_x,
                                      int encoder_bit_depth) {
    SequenceControlSet* scs            = pcs->scs;
    const InterpFilters interp_filters = av1_make_interp_filters(MULTITAP_SHARP, MULTITAP_SHARP);

    bool is_highbd = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;

    BlkStruct   blk_ptr;
    MacroBlockD av1xd;
    blk_ptr.av1xd = &av1xd;

    EbPictureBufferDesc reference_ptr;
    EbPictureBufferDesc prediction_ptr;

    prediction_ptr.org_x     = 0;
    prediction_ptr.org_y     = 0;
    prediction_ptr.stride_y  = BW;
    prediction_ptr.stride_cb = (uint16_t)BW >> ss_x;
    prediction_ptr.stride_cr = (uint16_t)BW >> ss_x;

    if (!is_highbd) {
        prediction_ptr.buffer_y  = pred[C_Y];
        prediction_ptr.buffer_cb = pred[C_U];
        prediction_ptr.buffer_cr = pred[C_V];
    } else {
        prediction_ptr.buffer_y         = (uint8_t*)pred_16bit[C_Y];
        prediction_ptr.buffer_cb        = (uint8_t*)pred_16bit[C_U];
        prediction_ptr.buffer_cr        = (uint8_t*)pred_16bit[C_V];
        reference_ptr.buffer_y          = (uint8_t*)pcs_ref->altref_buffer_highbd[C_Y];
        reference_ptr.buffer_cb         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_U];
        reference_ptr.buffer_cr         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_V];
        reference_ptr.org_x             = pic_ptr_ref->org_x;
        reference_ptr.org_y             = pic_ptr_ref->org_y;
        reference_ptr.stride_y          = pic_ptr_ref->stride_y;
        reference_ptr.stride_cb         = pic_ptr_ref->stride_cb;
        reference_ptr.stride_cr         = pic_ptr_ref->stride_cr;
        reference_ptr.width             = pic_ptr_ref->width;
        reference_ptr.height            = pic_ptr_ref->height;
        reference_ptr.buffer_bit_inc_y  = NULL;
        reference_ptr.buffer_bit_inc_cb = NULL;
        reference_ptr.buffer_bit_inc_cr = NULL;
    }

    uint16_t local_origin_x = 0;
    uint16_t local_origin_y = 0;
    uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
    uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
    int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
    int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

    const int32_t bw                 = mi_size_wide[BLOCK_64X64];
    const int32_t bh                 = mi_size_high[BLOCK_64X64];
    blk_ptr.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
    blk_ptr.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
    blk_ptr.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
    blk_ptr.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;

    // Perform final pass using the 1/8 MV
    // AV1 MVs are always in 1/8th pel precision.
    BlockModeInfo block_mi = {.mv[0]              = {{me_ctx->tf_64x64_mv_x, me_ctx->tf_64x64_mv_y}},
                              .ref_frame[0]       = LAST_FRAME,
                              .ref_frame[1]       = NONE_FRAME,
                              .is_interintra_used = 0,
                              .motion_mode        = SIMPLE_TRANSLATION,
                              .interp_filters     = (uint32_t)interp_filters,
                              .mode               = NEWMV,
                              .use_intrabc        = 0};
    svt_aom_inter_prediction(scs,
                             NULL, //pcs,
                             &block_mi,
                             NULL, // wm_params
                             NULL, // wm_params
                             &blk_ptr,
                             BLOCK_64X64,
                             PART_N,
                             false, // use_precomputed_obmc
                             false, // use_precomputed_ii
                             NULL, // ctx
                             NULL,
                             NULL,
                             NULL,
                             !is_highbd ? pic_ptr_ref : &reference_ptr,
                             NULL, //ref_pic_list1,
                             pu_origin_x,
                             pu_origin_y,
                             &prediction_ptr,
                             local_origin_x,
                             local_origin_y,
                             me_ctx->tf_chroma ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
                             (uint8_t)encoder_bit_depth,
                             is_highbd);
}

static void tf_32x32_inter_prediction(PictureParentControlSet* pcs, MeContext* me_ctx, PictureParentControlSet* pcs_ref,
                                      EbPictureBufferDesc* pic_ptr_ref, EbByte* pred, uint16_t** pred_16bit,
                                      uint32_t sb_origin_x, uint32_t sb_origin_y, uint32_t ss_x,
                                      int encoder_bit_depth) {
    SequenceControlSet* scs            = pcs->scs;
    const InterpFilters interp_filters = av1_make_interp_filters(MULTITAP_SHARP, MULTITAP_SHARP);

    bool is_highbd = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;

    BlkStruct   blk_ptr;
    MacroBlockD av1xd;
    blk_ptr.av1xd = &av1xd;

    EbPictureBufferDesc reference_ptr;
    EbPictureBufferDesc prediction_ptr;

    prediction_ptr.org_x     = 0;
    prediction_ptr.org_y     = 0;
    prediction_ptr.stride_y  = BW;
    prediction_ptr.stride_cb = (uint16_t)BW >> ss_x;
    prediction_ptr.stride_cr = (uint16_t)BW >> ss_x;

    if (!is_highbd) {
        prediction_ptr.buffer_y  = pred[C_Y];
        prediction_ptr.buffer_cb = pred[C_U];
        prediction_ptr.buffer_cr = pred[C_V];
    } else {
        prediction_ptr.buffer_y         = (uint8_t*)pred_16bit[C_Y];
        prediction_ptr.buffer_cb        = (uint8_t*)pred_16bit[C_U];
        prediction_ptr.buffer_cr        = (uint8_t*)pred_16bit[C_V];
        reference_ptr.buffer_y          = (uint8_t*)pcs_ref->altref_buffer_highbd[C_Y];
        reference_ptr.buffer_cb         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_U];
        reference_ptr.buffer_cr         = (uint8_t*)pcs_ref->altref_buffer_highbd[C_V];
        reference_ptr.org_x             = pic_ptr_ref->org_x;
        reference_ptr.org_y             = pic_ptr_ref->org_y;
        reference_ptr.stride_y          = pic_ptr_ref->stride_y;
        reference_ptr.stride_cb         = pic_ptr_ref->stride_cb;
        reference_ptr.stride_cr         = pic_ptr_ref->stride_cr;
        reference_ptr.width             = pic_ptr_ref->width;
        reference_ptr.height            = pic_ptr_ref->height;
        reference_ptr.buffer_bit_inc_y  = NULL;
        reference_ptr.buffer_bit_inc_cb = NULL;
        reference_ptr.buffer_bit_inc_cr = NULL;
    }

    uint32_t idx_32x32 = me_ctx->idx_32x32;
    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        for (uint32_t idx_16x16 = 0; idx_16x16 < 4; idx_16x16++) {
            if (me_ctx->tf_16x16_block_split_flag[idx_32x32][idx_16x16]) {
                uint32_t bsize = 8;

                for (uint32_t idx_8x8 = 0; idx_8x8 < 4; idx_8x8++) {
                    uint32_t pu_index = idx_32x32_to_idx_8x8[idx_32x32][idx_16x16][idx_8x8];

                    uint32_t idx_y          = subblock_xy_8x8[pu_index][0];
                    uint32_t idx_x          = subblock_xy_8x8[pu_index][1];
                    uint16_t local_origin_x = idx_x * bsize;
                    uint16_t local_origin_y = idx_y * bsize;
                    uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
                    uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
                    int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
                    int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

                    const int32_t bw                 = mi_size_wide[BLOCK_8X8];
                    const int32_t bh                 = mi_size_high[BLOCK_8X8];
                    blk_ptr.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
                    blk_ptr.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
                    blk_ptr.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
                    blk_ptr.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;
                    // Perform final pass using the 1/8 MV
                    //AV1 MVs are always in 1/8th pel precision.
                    BlockModeInfo block_mi = {
                        .mv[0]              = {{me_ctx->tf_8x8_mv_x[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8],
                                                me_ctx->tf_8x8_mv_y[idx_32x32 * 16 + 4 * idx_16x16 + idx_8x8]}},
                        .ref_frame[0]       = LAST_FRAME,
                        .ref_frame[1]       = NONE_FRAME,
                        .is_interintra_used = 0,
                        .motion_mode        = SIMPLE_TRANSLATION,
                        .interp_filters     = (uint32_t)interp_filters,
                        .mode               = NEWMV,
                        .use_intrabc        = 0};
                    svt_aom_inter_prediction(
                        scs,
                        NULL, //pcs,
                        &block_mi,
                        NULL, // wm_params
                        NULL, // wm_params
                        &blk_ptr,
                        BLOCK_8X8,
                        PART_N,
                        false, // use_precomputed_obmc
                        false, // use_precomputed_ii
                        NULL, // ctx
                        NULL,
                        NULL,
                        NULL,
                        !is_highbd ? pic_ptr_ref : &reference_ptr,
                        NULL, //ref_pic_list1,
                        pu_origin_x,
                        pu_origin_y,
                        &prediction_ptr,
                        local_origin_x,
                        local_origin_y,
                        me_ctx->tf_chroma ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
                        (uint8_t)encoder_bit_depth,
                        is_highbd);
                }
            } else {
                uint32_t bsize    = 16;
                uint32_t pu_index = idx_32x32_to_idx_16x16[idx_32x32][idx_16x16];

                uint32_t idx_y          = subblock_xy_16x16[pu_index][0];
                uint32_t idx_x          = subblock_xy_16x16[pu_index][1];
                uint16_t local_origin_x = idx_x * bsize;
                uint16_t local_origin_y = idx_y * bsize;
                uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
                uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
                int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
                int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

                const int32_t bw                 = mi_size_wide[BLOCK_16X16];
                const int32_t bh                 = mi_size_high[BLOCK_16X16];
                blk_ptr.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
                blk_ptr.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
                blk_ptr.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
                blk_ptr.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;
                // Perform final pass using the 1/8 MV
                //AV1 MVs are always in 1/8th pel precision.
                BlockModeInfo block_mi = {.mv[0]              = {{me_ctx->tf_16x16_mv_x[idx_32x32 * 4 + idx_16x16],
                                                                  me_ctx->tf_16x16_mv_y[idx_32x32 * 4 + idx_16x16]}},
                                          .ref_frame[0]       = LAST_FRAME,
                                          .ref_frame[1]       = NONE_FRAME,
                                          .is_interintra_used = 0,
                                          .motion_mode        = SIMPLE_TRANSLATION,
                                          .interp_filters     = (uint32_t)interp_filters,
                                          .mode               = NEWMV,
                                          .use_intrabc        = 0};
                svt_aom_inter_prediction(
                    scs,
                    NULL, //pcs,
                    &block_mi,
                    NULL, // wm_params
                    NULL, // wm_params
                    &blk_ptr,
                    BLOCK_16X16,
                    PART_N,
                    false, // use_precomputed_obmc
                    false, // use_precomputed_ii
                    NULL, // ctx
                    NULL,
                    NULL,
                    NULL,
                    !is_highbd ? pic_ptr_ref : &reference_ptr,
                    NULL, //ref_pic_list1,
                    pu_origin_x,
                    pu_origin_y,
                    &prediction_ptr,
                    local_origin_x,
                    local_origin_y,
                    me_ctx->tf_chroma ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
                    (uint8_t)encoder_bit_depth,
                    is_highbd);
            }
        }
    } else {
        uint32_t bsize = 32;

        uint32_t idx_x = idx_32x32 & 0x1;
        uint32_t idx_y = idx_32x32 >> 1;

        uint16_t local_origin_x = idx_x * bsize;
        uint16_t local_origin_y = idx_y * bsize;
        uint16_t pu_origin_x    = sb_origin_x + local_origin_x;
        uint16_t pu_origin_y    = sb_origin_y + local_origin_y;
        int32_t  mirow          = pu_origin_y >> MI_SIZE_LOG2;
        int32_t  micol          = pu_origin_x >> MI_SIZE_LOG2;

        const int32_t bw                 = mi_size_wide[BLOCK_32X32];
        const int32_t bh                 = mi_size_high[BLOCK_32X32];
        blk_ptr.av1xd->mb_to_top_edge    = -(int32_t)((mirow * MI_SIZE) * 8);
        blk_ptr.av1xd->mb_to_bottom_edge = ((pcs->av1_cm->mi_rows - bw - mirow) * MI_SIZE) * 8;
        blk_ptr.av1xd->mb_to_left_edge   = -(int32_t)((micol * MI_SIZE) * 8);
        blk_ptr.av1xd->mb_to_right_edge  = ((pcs->av1_cm->mi_cols - bh - micol) * MI_SIZE) * 8;

        // Perform final pass using the 1/8 MV
        //AV1 MVs are always in 1/8th pel precision.
        BlockModeInfo block_mi = {.mv[0] = {{me_ctx->tf_32x32_mv_x[idx_32x32], me_ctx->tf_32x32_mv_y[idx_32x32]}},
                                  .ref_frame[0]       = LAST_FRAME,
                                  .ref_frame[1]       = NONE_FRAME,
                                  .is_interintra_used = 0,
                                  .motion_mode        = SIMPLE_TRANSLATION,
                                  .interp_filters     = (uint32_t)interp_filters,
                                  .mode               = NEWMV,
                                  .use_intrabc        = 0};
        svt_aom_inter_prediction(scs,
                                 NULL, //pcs,
                                 &block_mi,
                                 NULL, // wm_params
                                 NULL, // wm_params
                                 &blk_ptr,
                                 BLOCK_32X32,
                                 PART_N,
                                 false, // use_precomputed_obmc
                                 false, // use_precomputed_ii
                                 NULL, // ctx
                                 NULL,
                                 NULL,
                                 NULL,
                                 !is_highbd ? pic_ptr_ref : &reference_ptr,
                                 NULL, //ref_pic_list1,
                                 pu_origin_x,
                                 pu_origin_y,
                                 &prediction_ptr,
                                 local_origin_x,
                                 local_origin_y,
                                 me_ctx->tf_chroma ? PICTURE_BUFFER_DESC_FULL_MASK : PICTURE_BUFFER_DESC_LUMA_MASK,
                                 (uint8_t)encoder_bit_depth,
                                 is_highbd);
    }
}

void svt_aom_get_final_filtered_pixels_c(MeContext* me_ctx, EbByte* src_center_ptr_start,
                                         uint16_t** altref_buffer_highbd_start, uint32_t** accum, uint16_t** count,
                                         const uint32_t* stride, int blk_y_src_offset, int blk_ch_src_offset,
                                         uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd) {
    int i, j, k;

    if (!is_highbd) {
        // Process luma
        int pos = blk_y_src_offset;
        for (i = 0, k = 0; i < BH; i++) {
            for (j = 0; j < BW; j++, k++) {
                assert(OD_DIVU(accum[C_Y][k] + (count[C_Y][k] >> 1), count[C_Y][k]) < 256);
                src_center_ptr_start[C_Y][pos] = (uint8_t)OD_DIVU(accum[C_Y][k] + (count[C_Y][k] >> 1), count[C_Y][k]);
                pos++;
            }
            pos += stride[C_Y] - BW;
        }
        // Process chroma
        if (me_ctx->tf_chroma) {
            pos = blk_ch_src_offset;
            for (i = 0, k = 0; i < blk_height_ch; i++) {
                for (j = 0; j < blk_width_ch; j++, k++) {
                    assert(OD_DIVU(accum[C_U][k] + (count[C_U][k] >> 1), count[C_U][k]) < 256);
                    src_center_ptr_start[C_U][pos] = (uint8_t)OD_DIVU(accum[C_U][k] + (count[C_U][k] >> 1),
                                                                      count[C_U][k]);
                    assert(OD_DIVU(accum[C_U][k] + (count[C_U][k] >> 1), count[C_U][k]) < 256);
                    src_center_ptr_start[C_V][pos] = (uint8_t)OD_DIVU(accum[C_V][k] + (count[C_V][k] >> 1),
                                                                      count[C_V][k]);
                    pos++;
                }
                pos += stride[C_U] - blk_width_ch;
            }
        }
    } else {
        // Process luma
        int pos = blk_y_src_offset;
        for (i = 0, k = 0; i < BH; i++) {
            for (j = 0; j < BW; j++, k++) {
                altref_buffer_highbd_start[C_Y][pos] = (uint16_t)OD_DIVU(accum[C_Y][k] + (count[C_Y][k] >> 1),
                                                                         count[C_Y][k]);
                pos++;
            }
            pos += stride[C_Y] - BW;
        }
        // Process chroma
        if (me_ctx->tf_chroma) {
            pos = blk_ch_src_offset;
            for (i = 0, k = 0; i < blk_height_ch; i++) {
                for (j = 0; j < blk_width_ch; j++, k++) {
                    altref_buffer_highbd_start[C_U][pos] = (uint16_t)OD_DIVU(accum[C_U][k] + (count[C_U][k] >> 1),
                                                                             count[C_U][k]);
                    altref_buffer_highbd_start[C_V][pos] = (uint16_t)OD_DIVU(accum[C_V][k] + (count[C_V][k] >> 1),
                                                                             count[C_V][k]);
                    pos++;
                }
                pos += stride[C_U] - blk_width_ch;
            }
        }
    }
}

/*
* Check whether to perform 64x64 pred only
*/
int8_t tf_use_64x64_pred(MeContext* me_ctx) {
    uint32_t dist_32x32 = 0;

    // 32x32
    for (unsigned i = 0; i < 4; i++) {
        dist_32x32 += me_ctx->p_best_sad_32x32[i];
    }

    int64_t dev_64x64_to_32x32 =
        (int64_t)(((int64_t)MAX(me_ctx->p_best_sad_64x64[0], 1) - (int64_t)MAX(dist_32x32, 1)) * 100) /
        (int64_t)MAX(dist_32x32, 1);
    if (dev_64x64_to_32x32 < me_ctx->tf_use_pred_64x64_only_th) {
        return 1;
    } else {
        return 0;
    }
}

static void convert_64x64_info_to_32x32_info(PictureParentControlSet* pcs, MeContext* ctx, EbByte* pred,
                                             uint16_t** pred_16bit, uint32_t* stride_pred, EbByte* src,
                                             uint16_t** src_16bit, uint32_t* stride_src, bool is_highbd) {
    ctx->tf_32x32_mv_x[0] = ctx->tf_64x64_mv_x;
    ctx->tf_32x32_mv_y[0] = ctx->tf_64x64_mv_y;

    ctx->tf_32x32_mv_x[1] = ctx->tf_64x64_mv_x;
    ctx->tf_32x32_mv_y[1] = ctx->tf_64x64_mv_y;

    ctx->tf_32x32_mv_x[2] = ctx->tf_64x64_mv_x;
    ctx->tf_32x32_mv_y[2] = ctx->tf_64x64_mv_y;

    ctx->tf_32x32_mv_x[3] = ctx->tf_64x64_mv_x;
    ctx->tf_32x32_mv_y[3] = ctx->tf_64x64_mv_y;

    ctx->tf_32x32_block_split_flag[0] = 0;
    ctx->tf_32x32_block_split_flag[1] = 0;
    ctx->tf_32x32_block_split_flag[2] = 0;
    ctx->tf_32x32_block_split_flag[3] = 0;
    memset(ctx->tf_16x16_block_split_flag, 0, sizeof(ctx->tf_16x16_block_split_flag[0][0]) * 4 * 4);

    // Update the 32x32 block-error
    for (int block_row = 0; block_row < 2; block_row++) {
        for (int block_col = 0; block_col < 2; block_col++) {
            uint32_t bsize = 32;
            uint64_t distortion;
            ctx->idx_32x32 = block_col + (block_row << 1);

            if (!is_highbd) {
                uint8_t* pred_y_ptr = pred[C_Y] + bsize * block_row * stride_pred[C_Y] + bsize * block_col;
                uint8_t* src_y_ptr  = src[C_Y] + bsize * block_row * stride_src[C_Y] + bsize * block_col;

                const AomVarianceFnPtr* fn_ptr = pcs->tf_ctrls.sub_sampling_shift ? &svt_aom_mefn_ptr[BLOCK_32X16]
                                                                                  : &svt_aom_mefn_ptr[BLOCK_32X32];
                unsigned int            sse;
                distortion = fn_ptr->vf(pred_y_ptr,
                                        stride_pred[C_Y] << pcs->tf_ctrls.sub_sampling_shift,
                                        src_y_ptr,
                                        stride_src[C_Y] << pcs->tf_ctrls.sub_sampling_shift,
                                        &sse)
                    << pcs->tf_ctrls.sub_sampling_shift;
            } else {
                uint16_t* pred_y_ptr = pred_16bit[C_Y] + bsize * block_row * stride_pred[C_Y] + bsize * block_col;
                uint16_t* src_y_ptr  = src_16bit[C_Y] + bsize * block_row * stride_src[C_Y] + bsize * block_col;
                const AomVarianceFnPtr* fn_ptr = pcs->tf_ctrls.sub_sampling_shift ? &svt_aom_mefn_ptr[BLOCK_32X16]
                                                                                  : &svt_aom_mefn_ptr[BLOCK_32X32];

                unsigned int sse;

                distortion = fn_ptr->vf_hbd_10(CONVERT_TO_BYTEPTR(pred_y_ptr),
                                               stride_pred[C_Y] << pcs->tf_ctrls.sub_sampling_shift,
                                               CONVERT_TO_BYTEPTR(src_y_ptr),
                                               stride_src[C_Y] << pcs->tf_ctrls.sub_sampling_shift,
                                               &sse)
                    << pcs->tf_ctrls.sub_sampling_shift;
            }
            ctx->tf_32x32_block_error[ctx->idx_32x32] = distortion;
        }
    }
}

static void set_hme_search_params_mctf(MeContext* ctx, uint8_t hme_search_level) {
    switch (hme_search_level) {
    case 0:
        ctx->hme_l0_sa.sa_min.width  = ctx->hme_l0_sa_default_tf.sa_min.width;
        ctx->hme_l0_sa.sa_min.height = ctx->hme_l0_sa_default_tf.sa_min.height;
        ctx->hme_l0_sa.sa_max.width  = ctx->hme_l0_sa_default_tf.sa_max.width;
        ctx->hme_l0_sa.sa_max.height = ctx->hme_l0_sa_default_tf.sa_max.height;
        break;
    case 1:
        ctx->hme_l0_sa.sa_min.width  = ctx->hme_l0_sa_default_tf.sa_min.width << 1;
        ctx->hme_l0_sa.sa_min.height = ctx->hme_l0_sa_default_tf.sa_min.height << 1;
        ctx->hme_l0_sa.sa_max.width  = ctx->hme_l0_sa_default_tf.sa_max.width << 2;
        ctx->hme_l0_sa.sa_max.height = ctx->hme_l0_sa_default_tf.sa_max.height << 2;
        break;

    default:
        assert(0);
        break;
    }
}

// Produce the filtered alt-ref picture
// - core function
static EbErrorType produce_temporally_filtered_pic(PictureParentControlSet** pcs_list,
                                                   EbPictureBufferDesc** list_input_picture_ptr, uint8_t index_center,
                                                   MotionEstimationContext_t* me_context_ptr,
                                                   const int32_t* noise_levels_log1p_fp16, int32_t segment_index,
                                                   bool is_highbd) {
    DECLARE_ALIGNED(16, uint32_t, accumulator[BLK_PELS * COLOR_CHANNELS]);
    DECLARE_ALIGNED(16, uint16_t, counter[BLK_PELS * COLOR_CHANNELS]);
    uint32_t* accum[COLOR_CHANNELS] = {accumulator, accumulator + BLK_PELS, accumulator + (BLK_PELS << 1)};
    uint16_t* count[COLOR_CHANNELS] = {counter, counter + BLK_PELS, counter + (BLK_PELS << 1)};

    EbByte                   predictor                 = {NULL};
    uint16_t*                predictor_16bit           = {NULL};
    PictureParentControlSet* centre_pcs                = pcs_list[index_center];
    SequenceControlSet*      scs                       = centre_pcs->scs;
    EbPictureBufferDesc*     input_picture_ptr_central = list_input_picture_ptr[index_center];
    MeContext*               ctx                       = me_context_ptr->me_ctx;

    // Prep 8bit source if 8bit content or using 8bit for subpel
    if (!is_highbd || ctx->tf_ctrls.use_8bit_subpel) {
        EB_MALLOC_ALIGNED_ARRAY(predictor, BLK_PELS * COLOR_CHANNELS);
    }

    if (is_highbd) {
        EB_MALLOC_ALIGNED_ARRAY(predictor_16bit, BLK_PELS * COLOR_CHANNELS);
    }
    EbByte    pred[COLOR_CHANNELS]       = {predictor, predictor + BLK_PELS, predictor + (BLK_PELS << 1)};
    uint16_t* pred_16bit[COLOR_CHANNELS] = {
        predictor_16bit, predictor_16bit + BLK_PELS, predictor_16bit + (BLK_PELS << 1)};
    int encoder_bit_depth = scs->static_config.encoder_bit_depth;

    // chroma subsampling
    uint32_t ss_x          = scs->subsampling_x;
    uint32_t ss_y          = scs->subsampling_y;
    uint16_t blk_width_ch  = (uint16_t)BW >> ss_x;
    uint16_t blk_height_ch = (uint16_t)BH >> ss_y;

    uint32_t blk_cols = (uint32_t)(input_picture_ptr_central->width + BW - 1) /
        BW; // I think only the part of the picture
    uint32_t blk_rows = (uint32_t)(input_picture_ptr_central->height + BH - 1) /
        BH; // that fits to the 32x32 blocks are actually filtered

    uint32_t stride[COLOR_CHANNELS]      = {input_picture_ptr_central->stride_y,
                                            input_picture_ptr_central->stride_cb,
                                            input_picture_ptr_central->stride_cr};
    uint32_t stride_pred[COLOR_CHANNELS] = {BW, blk_width_ch, blk_width_ch};
    uint32_t x_seg_idx;
    uint32_t y_seg_idx;
    uint32_t picture_width_in_b64  = blk_cols;
    uint32_t picture_height_in_b64 = blk_rows;
    SEGMENT_CONVERT_IDX_TO_XY(segment_index, x_seg_idx, y_seg_idx, centre_pcs->tf_segments_column_count);
    uint32_t x_b64_start_idx = SEGMENT_START_IDX(x_seg_idx, picture_width_in_b64, centre_pcs->tf_segments_column_count);
    uint32_t x_b64_end_idx   = SEGMENT_END_IDX(x_seg_idx, picture_width_in_b64, centre_pcs->tf_segments_column_count);
    uint32_t y_b64_start_idx = SEGMENT_START_IDX(y_seg_idx, picture_height_in_b64, centre_pcs->tf_segments_row_count);
    uint32_t y_b64_end_idx   = SEGMENT_END_IDX(y_seg_idx, picture_height_in_b64, centre_pcs->tf_segments_row_count);

    // first position of the frame buffer according to the index center
    EbByte src_center_ptr_start[COLOR_CHANNELS] = {
        input_picture_ptr_central->buffer_y + input_picture_ptr_central->org_y * input_picture_ptr_central->stride_y +
            input_picture_ptr_central->org_x,
        input_picture_ptr_central->buffer_cb +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_cb +
            (input_picture_ptr_central->org_x >> ss_x),
        input_picture_ptr_central->buffer_cr +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_cr +
            (input_picture_ptr_central->org_x >> ss_x),
    };

    uint16_t* altref_buffer_highbd_start[COLOR_CHANNELS] = {
        centre_pcs->altref_buffer_highbd[C_Y] + input_picture_ptr_central->org_y * input_picture_ptr_central->stride_y +
            input_picture_ptr_central->org_x,
        centre_pcs->altref_buffer_highbd[C_U] +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_bit_inc_cb +
            (input_picture_ptr_central->org_x >> ss_x),
        centre_pcs->altref_buffer_highbd[C_V] +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_bit_inc_cr +
            (input_picture_ptr_central->org_x >> ss_x),
    };
    int decay_control[COLOR_CHANNELS];

    if (scs->vq_ctrls.sharpness_ctrls.tf && centre_pcs->is_noise_level && scs->calculate_variance &&
        centre_pcs->pic_avg_variance < VQ_PIC_AVG_VARIANCE_TH) {
        decay_control[C_Y] = 1;
        decay_control[C_U] = 1;
        decay_control[C_V] = 1;
    } else {
        decay_control[C_Y] = 3;
        decay_control[C_U] = 6;
        decay_control[C_V] = 6;

        if (centre_pcs->slice_type != I_SLICE) {
            int ratio = noise_levels_log1p_fp16[0]
                ? (centre_pcs->filt_to_unfilt_diff * 100) / noise_levels_log1p_fp16[0]
                : 0;

            if (ratio > 150) {
                decay_control[C_Y] += 1;
            }
        }
    }

    // Adjust filtering based on q.
    // Larger q -> stronger filtering -> larger weight.
    // Smaller q -> weaker filtering -> smaller weight.

    // Fixed-QP offsets are use here since final picture QP(s) are not generated @ this early stage
    const int bit_depth            = scs->static_config.encoder_bit_depth;
    int       active_best_quality  = 0;
    int       active_worst_quality = quantizer_to_qindex[(uint8_t)scs->static_config.qp];
    int       q;
    FP_ASSERT(TF_Q_DECAY_THRESHOLD == 20);
    int offset_idx;
    if (!centre_pcs->is_ref) {
        offset_idx = -1;
    } else if (centre_pcs->idr_flag) {
        offset_idx = 0;
    } else {
        offset_idx = MIN(centre_pcs->temporal_layer_index + 1, FIXED_QP_OFFSET_COUNT - 1);
    }

    // Fixed-QP offsets are use here since final picture QP(s) are not generated @ this early stage
    int32_t q_val_fp8 = svt_av1_convert_qindex_to_q_fp8(active_worst_quality, bit_depth);

    const int32_t q_val_target_fp8 = (offset_idx == -1)
        ? q_val_fp8
        : MAX(q_val_fp8 - (q_val_fp8 * percents[centre_pcs->hierarchical_levels <= 4][offset_idx] / 100), 0);

    const int32_t delta_qindex_f = svt_av1_compute_qdelta_fp(q_val_fp8, q_val_target_fp8, bit_depth);
    active_best_quality          = (int32_t)(active_worst_quality + delta_qindex_f);
    q                            = active_best_quality;

    FP_ASSERT(q < (1 << 20));
    // Max q_factor is 255, therefore the upper bound of q_decay is 8.
    // We do not need a clip here.
    //q_decay = 0.5 * pow((double)q / 64, 2);
    FP_ASSERT(q < (1 << 15));
    uint32_t q_decay_fp8 = 256;
    if (q >= TF_QINDEX_CUTOFF) {
        q_decay_fp8 = (q * q) >> 5;
    } else {
        q_decay_fp8 = MAX(q << 2, 1);
    }
    const int32_t const_0dot7_fp16 = 45875; //0.7
    /*Calculation of log and dceay_factor possible to move to estimate_noise() and calculate one time for GOP*/
    //decay_control * (0.7 + log1p(noise_levels[C_Y]))
    int32_t n_decay_fp10 = (decay_control[C_Y] * (const_0dot7_fp16 + noise_levels_log1p_fp16[C_Y])) / ((int32_t)1 << 6);
    //2 * n_decay * n_decay * q_decay * (s_decay always is 1);
    /*
         * TF STRENGTH CALCULATION
         */
    // Get the frame update type for the current frame
    const uint32_t frame_update_type = svt_aom_get_frame_update_type(centre_pcs->scs, centre_pcs);

    if (scs->static_config.enable_tf > 1) {
        uint8_t adaptive_tf_shift_factor = calculate_tf_shift_factor(ctx);
        assert(adaptive_tf_shift_factor <= 14);
        const uint8_t kf_tf_shift_factor = CLIP3(0, 14, adaptive_tf_shift_factor + 1);
        assert(kf_tf_shift_factor <= 14);

        if (frame_update_type == SVT_AV1_KF_UPDATE && kf_tf_shift_factor == 14) {
            ctx->tf_decay_factor_fp16[C_Y] = 0;
            ctx->tf_decay_factor_fp16[C_U] = 0;
            ctx->tf_decay_factor_fp16[C_V] = 0;
        } else if (frame_update_type == SVT_AV1_KF_UPDATE) {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control[C_U],
                                           decay_control[C_V],
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           kf_tf_shift_factor,
                                           ctx->tf_chroma);
        } else {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control[C_U],
                                           decay_control[C_V],
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           adaptive_tf_shift_factor,
                                           ctx->tf_chroma);
        }
    } else {
        // tf_shift_factor is manually adjusted by the user via --tf-strength
        // 10 + (4 - 0) = 14 (8x weaker)
        // 10 + (4 - 1) = 13 (4x weaker)
        // 10 + (4 - 2) = 12 (2x weaker)
        // 10 + (4 - 3) = 11 (default)
        // 10 + (4 - 4) = 10 (2x stronger)
        const uint8_t tf_shift_factor = 10 + (4 - scs->static_config.tf_strength);
        assert(tf_shift_factor <= 14);

        // kf_tf_shift_factor is 1 + tf strength when using Tune 0 (VQ)
        uint8_t kf_tf_shift_factor = tf_shift_factor;
        if (scs->vq_ctrls.sharpness_ctrls.tf) {
            kf_tf_shift_factor = MIN(14, tf_shift_factor + 1);
        }
        assert(kf_tf_shift_factor <= 14);

        // when kf_tf_shift_factor is 14, we disable tf on keyframes
        if (frame_update_type == SVT_AV1_KF_UPDATE && kf_tf_shift_factor == 14) {
            ctx->tf_decay_factor_fp16[C_Y] = 0;
            ctx->tf_decay_factor_fp16[C_U] = 0;
            ctx->tf_decay_factor_fp16[C_V] = 0;
        } else if (frame_update_type == SVT_AV1_KF_UPDATE) {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control[C_U],
                                           decay_control[C_V],
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           kf_tf_shift_factor,
                                           ctx->tf_chroma);
        } else {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control[C_U],
                                           decay_control[C_V],
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           tf_shift_factor,
                                           ctx->tf_chroma);
        }
    }
    for (uint32_t blk_row = y_b64_start_idx; blk_row < y_b64_end_idx; blk_row++) {
        for (uint32_t blk_col = x_b64_start_idx; blk_col < x_b64_end_idx; blk_col++) {
            int blk_y_src_offset  = (blk_col * BW) + (blk_row * BH) * stride[C_Y];
            int blk_ch_src_offset = (blk_col * blk_width_ch) + (blk_row * blk_height_ch) * stride[C_U];

            // reset accumulator and count
            memset(accumulator, 0, BLK_PELS * COLOR_CHANNELS * sizeof(accumulator[0]));
            memset(counter, 0, BLK_PELS * COLOR_CHANNELS * sizeof(counter[0]));

            EbByte    src_center_ptr[COLOR_CHANNELS]           = {NULL};
            uint16_t* altref_buffer_highbd_ptr[COLOR_CHANNELS] = {NULL};
            // Prep 8bit source if 8bit content or using 8bit for subpel
            if (!is_highbd || ctx->tf_ctrls.use_8bit_subpel) {
                src_center_ptr[C_Y] = src_center_ptr_start[C_Y] + blk_y_src_offset;
                if (ctx->tf_chroma) {
                    src_center_ptr[C_U] = src_center_ptr_start[C_U] + blk_ch_src_offset;
                    src_center_ptr[C_V] = src_center_ptr_start[C_V] + blk_ch_src_offset;
                }
            }

            if (is_highbd) {
                altref_buffer_highbd_ptr[C_Y] = altref_buffer_highbd_start[C_Y] + blk_y_src_offset;
                if (ctx->tf_chroma) {
                    altref_buffer_highbd_ptr[C_U] = altref_buffer_highbd_start[C_U] + blk_ch_src_offset;
                    altref_buffer_highbd_ptr[C_V] = altref_buffer_highbd_start[C_V] + blk_ch_src_offset;
                }
            }

            if (!is_highbd) {
                apply_filtering_central(
                    ctx, input_picture_ptr_central, src_center_ptr, accum, count, BW, BH, ss_x, ss_y);
            }
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
            else {
                apply_filtering_central_highbd(
                    ctx, input_picture_ptr_central, altref_buffer_highbd_ptr, accum, count, BW, BH, ss_x, ss_y);
            }
#endif

            // 1st segment: past pics - from closest to farthest
            // 2nd segment: current pic
            // 3rd segment: future pics - from closest to farthest

            int start_frame_index[3] = {0, centre_pcs->past_altref_nframes, centre_pcs->past_altref_nframes + 1};

            int end_frame_index[3] = {centre_pcs->past_altref_nframes - 1,
                                      centre_pcs->past_altref_nframes,
                                      centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes};

            for (int segment_idx = 0; segment_idx < 3; segment_idx++) {
                for (int frame_index = start_frame_index[segment_idx]; frame_index <= end_frame_index[segment_idx];
                     frame_index     = frame_index + me_context_ptr->me_ctx->tf_ctrls.ref_frame_factor) {
                    // Use ahd-error to central/avg to identify/skip outlier ref-frame(s)
                    if (frame_index != index_center) {
                        uint32_t low_ahd_err = centre_pcs->aligned_width * centre_pcs->aligned_height;
                        uint8_t  th          = (centre_pcs->slice_type == I_SLICE) ? 20 : 40;
                        if (pcs_list[frame_index]->tf_ahd_error_to_central >
                                low_ahd_err && // error to central high enough
                            ((int)(((int)pcs_list[frame_index]->tf_ahd_error_to_central -
                                    (int)centre_pcs->tf_avg_ahd_error) *
                                   100)) >
                                (th *
                                 (int)centre_pcs
                                     ->tf_avg_ahd_error)) { // ahd_error_to_central higher than tf_avg_ahd_error by x%
                            continue;
                        }

                        uint32_t bright_change_region_cnt = 0;
                        for (uint32_t region_in_picture_width_index = 0;
                             region_in_picture_width_index < scs->picture_analysis_number_of_regions_per_width;
                             region_in_picture_width_index++) { // loop over horizontal regions
                            for (uint32_t region_in_picture_height_index = 0;
                                 region_in_picture_height_index < scs->picture_analysis_number_of_regions_per_height;
                                 region_in_picture_height_index++) { // loop over vertical regions

                                if (ABS((int)pcs_list[frame_index]
                                            ->average_intensity_per_region[region_in_picture_width_index]
                                                                          [region_in_picture_height_index] -
                                        (int)centre_pcs->average_intensity_per_region[region_in_picture_width_index]
                                                                                     [region_in_picture_height_index]) >
                                        2 &&
                                    pcs_list[frame_index]->avg_luma != centre_pcs->tf_avg_luma) {
                                    bright_change_region_cnt++;
                                }
                            }
                        }
                        if (bright_change_region_cnt >= ((14 * scs->picture_analysis_number_of_regions_per_width *
                                                          scs->picture_analysis_number_of_regions_per_height) /
                                                         16)) {
                            continue;
                        }
                    }
                    // ------------
                    // Step 1: motion estimation + compensation
                    // ------------
                    me_context_ptr->me_ctx->tf_frame_index  = frame_index;
                    me_context_ptr->me_ctx->tf_index_center = index_center;
                    if (frame_index != index_center) {
                        // Initialize ME context
                        create_me_context_and_picture_control(me_context_ptr,
                                                              pcs_list[frame_index],
                                                              pcs_list[index_center],
                                                              input_picture_ptr_central,
                                                              blk_row,
                                                              blk_col,
                                                              ss_x,
                                                              ss_y);
                        ctx->num_of_list_to_search       = 1;
                        ctx->num_of_ref_pic_to_search[0] = 1;
                        ctx->num_of_ref_pic_to_search[1] = 0;
                        ctx->temporal_layer_index        = centre_pcs->temporal_layer_index;
                        ctx->is_ref                      = centre_pcs->is_ref;

                        EbPaReferenceObject* ref_object        = (EbPaReferenceObject*)ctx->alt_ref_reference_ptr;
                        ctx->me_ds_ref_array[0][0].picture_ptr = ref_object->input_padded_pic;
                        ctx->me_ds_ref_array[0][0].sixteenth_picture_ptr =
                            ref_object->sixteenth_downsampled_picture_ptr;
                        ctx->me_ds_ref_array[0][0].quarter_picture_ptr = ref_object->quarter_downsampled_picture_ptr;
                        ctx->me_ds_ref_array[0][0].picture_number      = ref_object->picture_number;
                        ctx->tf_me_exit_th                             = centre_pcs->tf_ctrls.me_exit_th;
                        ;
                        ctx->tf_use_pred_64x64_only_th = centre_pcs->tf_ctrls.use_pred_64x64_only_th;
                        ctx->tf_subpel_early_exit_th   = centre_pcs->tf_ctrls.subpel_early_exit_th;
                        // Perform ME - context_ptr will store the outputs (MVs, buffers, etc)
                        // Block-based MC using open-loop HME + refinement
                        // set default hme search params
                        set_hme_search_params_mctf(ctx, 0);
                        svt_aom_motion_estimation_b64(centre_pcs,
                                                      (uint32_t)blk_row * blk_cols + blk_col,
                                                      (uint32_t)blk_col * BW, // x block
                                                      (uint32_t)blk_row * BH, // y block
                                                      ctx,
                                                      input_picture_ptr_central); // source picture

                        if (ctx->tf_use_pred_64x64_only_th &&
                            (ctx->tf_use_pred_64x64_only_th == (uint8_t)~0 || tf_use_64x64_pred(ctx))) {
                            tf_64x64_sub_pel_search(centre_pcs,
                                                    ctx,
                                                    pcs_list[frame_index],
                                                    list_input_picture_ptr[frame_index],
                                                    pred,
                                                    pred_16bit,
                                                    stride_pred,
                                                    src_center_ptr,
                                                    altref_buffer_highbd_ptr,
                                                    stride,
                                                    (uint32_t)blk_col * BW,
                                                    (uint32_t)blk_row * BH,
                                                    ss_x,
                                                    (ctx->tf_ctrls.use_8bit_subpel) ? EB_EIGHT_BIT : encoder_bit_depth);

                            // Perform MC using the information acquired using the ME step
                            tf_64x64_inter_prediction(centre_pcs,
                                                      ctx,
                                                      pcs_list[frame_index],
                                                      list_input_picture_ptr[frame_index],
                                                      pred,
                                                      pred_16bit,
                                                      (uint32_t)blk_col * BW,
                                                      (uint32_t)blk_row * BH,
                                                      ss_x,
                                                      encoder_bit_depth);
                            convert_64x64_info_to_32x32_info(centre_pcs,
                                                             ctx,
                                                             pred,
                                                             pred_16bit,
                                                             stride_pred,
                                                             src_center_ptr,
                                                             altref_buffer_highbd_ptr,
                                                             stride,
                                                             is_highbd);
                        } else {
                            // 64x64 Sub-Pel search
                            tf_64x64_sub_pel_search(centre_pcs,
                                                    ctx,
                                                    pcs_list[frame_index],
                                                    list_input_picture_ptr[frame_index],
                                                    pred,
                                                    pred_16bit,
                                                    stride_pred,
                                                    src_center_ptr,
                                                    altref_buffer_highbd_ptr,
                                                    stride,
                                                    (uint32_t)blk_col * BW,
                                                    (uint32_t)blk_row * BH,
                                                    ss_x,
                                                    (ctx->tf_ctrls.use_8bit_subpel) ? EB_EIGHT_BIT : encoder_bit_depth);

                            // 32x32 Sub-Pel search
                            for (int block_row = 0; block_row < 2; block_row++) {
                                for (int block_col = 0; block_col < 2; block_col++) {
                                    ctx->idx_32x32 = block_col + (block_row << 1);

                                    tf_32x32_sub_pel_search(
                                        centre_pcs,
                                        ctx,
                                        pcs_list[frame_index],
                                        list_input_picture_ptr[frame_index],
                                        pred,
                                        pred_16bit,
                                        stride_pred,
                                        src_center_ptr,
                                        altref_buffer_highbd_ptr,
                                        stride,
                                        (uint32_t)blk_col * BW,
                                        (uint32_t)blk_row * BH,
                                        ss_x,
                                        (ctx->tf_ctrls.use_8bit_subpel) ? EB_EIGHT_BIT : encoder_bit_depth);
                                }
                            }

                            uint64_t sum_32x32_block_error = ctx->tf_32x32_block_error[0] +
                                ctx->tf_32x32_block_error[1] + ctx->tf_32x32_block_error[2] +
                                ctx->tf_32x32_block_error[3];
                            if ((ctx->tf_64x64_block_error * 14 < sum_32x32_block_error * 16) &&
                                ctx->tf_64x64_block_error < (1 << 18)) {
                                tf_64x64_inter_prediction(centre_pcs,
                                                          ctx,
                                                          pcs_list[frame_index],
                                                          list_input_picture_ptr[frame_index],
                                                          pred,
                                                          pred_16bit,
                                                          (uint32_t)blk_col * BW,
                                                          (uint32_t)blk_row * BH,
                                                          ss_x,
                                                          encoder_bit_depth);
                                convert_64x64_info_to_32x32_info(centre_pcs,
                                                                 ctx,
                                                                 pred,
                                                                 pred_16bit,
                                                                 stride_pred,
                                                                 src_center_ptr,
                                                                 altref_buffer_highbd_ptr,
                                                                 stride,
                                                                 is_highbd);
                            } else {
                                // 16x16 Sub-Pel search, and 32x32 partitioning
                                for (int block_row = 0; block_row < 2; block_row++) {
                                    for (int block_col = 0; block_col < 2; block_col++) {
                                        ctx->idx_32x32 = block_col + (block_row << 1);
                                        if (ctx->tf_32x32_block_error[ctx->idx_32x32] <
                                            centre_pcs->tf_ctrls.pred_error_32x32_th) {
                                            ctx->tf_32x32_block_split_flag[ctx->idx_32x32] = 0;
                                            memset(&ctx->tf_16x16_block_split_flag[ctx->idx_32x32][0],
                                                   0,
                                                   sizeof(ctx->tf_16x16_block_split_flag[ctx->idx_32x32][0]) * 4);
                                        } else {
                                            tf_16x16_sub_pel_search(
                                                centre_pcs,
                                                ctx,
                                                pcs_list[frame_index],
                                                list_input_picture_ptr[frame_index],
                                                pred,
                                                pred_16bit,
                                                stride_pred,
                                                src_center_ptr,
                                                altref_buffer_highbd_ptr,
                                                stride,
                                                (uint32_t)blk_col * BW,
                                                (uint32_t)blk_row * BH,
                                                ss_x,
                                                (ctx->tf_ctrls.use_8bit_subpel) ? EB_EIGHT_BIT : encoder_bit_depth);

                                            if (ctx->tf_ctrls.enable_8x8_pred) {
                                                tf_8x8_sub_pel_search(
                                                    centre_pcs,
                                                    ctx,
                                                    pcs_list[frame_index],
                                                    list_input_picture_ptr[frame_index],
                                                    pred,
                                                    pred_16bit,
                                                    stride_pred,
                                                    src_center_ptr,
                                                    altref_buffer_highbd_ptr,
                                                    stride,
                                                    (uint32_t)blk_col * BW,
                                                    (uint32_t)blk_row * BH,
                                                    ss_x,
                                                    (ctx->tf_ctrls.use_8bit_subpel) ? EB_EIGHT_BIT : encoder_bit_depth);
                                            }

                                            // Derive tf_32x32_block_split_flag
                                            derive_tf_32x32_block_split_flag(ctx);
                                        }
                                        // Perform MC using the information acquired using the ME step
                                        tf_32x32_inter_prediction(centre_pcs,
                                                                  ctx,
                                                                  pcs_list[frame_index],
                                                                  list_input_picture_ptr[frame_index],
                                                                  pred,
                                                                  pred_16bit,
                                                                  (uint32_t)blk_col * BW,
                                                                  (uint32_t)blk_row * BH,
                                                                  ss_x,
                                                                  encoder_bit_depth);
                                    }
                                }
                            }
                        }

                        for (int block_row = 0; block_row < 2; block_row++) {
                            for (int block_col = 0; block_col < 2; block_col++) {
                                ctx->tf_block_col = block_col;
                                ctx->tf_block_row = block_row;

                                apply_filtering_block_plane_wise(ctx,
                                                                 block_row,
                                                                 block_col,
                                                                 src_center_ptr,
                                                                 altref_buffer_highbd_ptr,
                                                                 pred,
                                                                 pred_16bit,
                                                                 accum,
                                                                 count,
                                                                 stride,
                                                                 stride_pred,
                                                                 BW >> 1,
                                                                 BH >> 1,
                                                                 ss_x,
                                                                 ss_y,
                                                                 encoder_bit_depth);
                            }
                        }
                    }
                }
            }

            // Normalize filter output to produce temporally filtered frame
            get_final_filtered_pixels(ctx,
                                      src_center_ptr_start,
                                      altref_buffer_highbd_start,
                                      accum,
                                      count,
                                      stride,
                                      blk_y_src_offset,
                                      blk_ch_src_offset,
                                      blk_width_ch,
                                      blk_height_ch,
                                      is_highbd);
        }
    }
    // Prep 8bit source if 8bit content or using 8bit for subpel
    if (!is_highbd || ctx->tf_ctrls.use_8bit_subpel) {
        EB_FREE_ALIGNED_ARRAY(predictor);
    }

    if (is_highbd) {
        EB_FREE_ALIGNED_ARRAY(predictor_16bit);
    }
    return EB_ErrorNone;
}

// Produce the filtered alt-ref picture for low delay mode
// - core function
static EbErrorType produce_temporally_filtered_pic_ld(PictureParentControlSet** pcs_list,
                                                      EbPictureBufferDesc**     list_input_picture_ptr,
                                                      uint8_t index_center, MotionEstimationContext_t* me_context_ptr,
                                                      const int32_t* noise_levels_log1p_fp16, int32_t segment_index,
                                                      bool is_highbd) {
    DECLARE_ALIGNED(16, uint32_t, accumulator[BLK_PELS * COLOR_CHANNELS]);
    DECLARE_ALIGNED(16, uint16_t, counter[BLK_PELS * COLOR_CHANNELS]);
    uint32_t* accum[COLOR_CHANNELS] = {accumulator, accumulator + BLK_PELS, accumulator + (BLK_PELS << 1)};
    uint16_t* count[COLOR_CHANNELS] = {counter, counter + BLK_PELS, counter + (BLK_PELS << 1)};

    EbByte                   predictor                 = {NULL};
    uint16_t*                predictor_16bit           = {NULL};
    PictureParentControlSet* centre_pcs                = pcs_list[index_center];
    SequenceControlSet*      scs                       = centre_pcs->scs;
    EbPictureBufferDesc*     input_picture_ptr_central = list_input_picture_ptr[index_center];
    MeContext*               ctx                       = me_context_ptr->me_ctx;

    // Prep 8bit source if 8bit content or using 8bit for subpel
    if (!is_highbd || ctx->tf_ctrls.use_8bit_subpel) {
        EB_MALLOC_ALIGNED_ARRAY(predictor, BLK_PELS * COLOR_CHANNELS);
    }

    if (is_highbd) {
        EB_MALLOC_ALIGNED_ARRAY(predictor_16bit, BLK_PELS * COLOR_CHANNELS);
    }
    EbByte    pred[COLOR_CHANNELS]       = {predictor, predictor + BLK_PELS, predictor + (BLK_PELS << 1)};
    uint16_t* pred_16bit[COLOR_CHANNELS] = {
        predictor_16bit, predictor_16bit + BLK_PELS, predictor_16bit + (BLK_PELS << 1)};
    int encoder_bit_depth = scs->static_config.encoder_bit_depth;

    // chroma subsampling
    uint32_t ss_x          = scs->subsampling_x;
    uint32_t ss_y          = scs->subsampling_y;
    uint16_t blk_width_ch  = (uint16_t)BW >> ss_x;
    uint16_t blk_height_ch = (uint16_t)BH >> ss_y;

    uint32_t blk_cols = (uint32_t)(input_picture_ptr_central->width + BW - 1) /
        BW; // I think only the part of the picture
    uint32_t blk_rows = (uint32_t)(input_picture_ptr_central->height + BH - 1) /
        BH; // that fits to the 32x32 blocks are actually filtered

    uint32_t stride[COLOR_CHANNELS]      = {input_picture_ptr_central->stride_y,
                                            input_picture_ptr_central->stride_cb,
                                            input_picture_ptr_central->stride_cr};
    uint32_t stride_pred[COLOR_CHANNELS] = {BW, blk_width_ch, blk_width_ch};
    uint32_t x_seg_idx;
    uint32_t y_seg_idx;
    uint32_t picture_width_in_b64  = blk_cols;
    uint32_t picture_height_in_b64 = blk_rows;
    SEGMENT_CONVERT_IDX_TO_XY(segment_index, x_seg_idx, y_seg_idx, centre_pcs->tf_segments_column_count);
    uint32_t x_b64_start_idx = SEGMENT_START_IDX(x_seg_idx, picture_width_in_b64, centre_pcs->tf_segments_column_count);
    uint32_t x_b64_end_idx   = SEGMENT_END_IDX(x_seg_idx, picture_width_in_b64, centre_pcs->tf_segments_column_count);
    uint32_t y_b64_start_idx = SEGMENT_START_IDX(y_seg_idx, picture_height_in_b64, centre_pcs->tf_segments_row_count);
    uint32_t y_b64_end_idx   = SEGMENT_END_IDX(y_seg_idx, picture_height_in_b64, centre_pcs->tf_segments_row_count);

    // first position of the frame buffer according to the index center
    EbByte src_center_ptr_start[COLOR_CHANNELS] = {
        input_picture_ptr_central->buffer_y + input_picture_ptr_central->org_y * input_picture_ptr_central->stride_y +
            input_picture_ptr_central->org_x,
        input_picture_ptr_central->buffer_cb +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_cb +
            (input_picture_ptr_central->org_x >> ss_x),
        input_picture_ptr_central->buffer_cr +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_cr +
            (input_picture_ptr_central->org_x >> ss_x),
    };

    uint16_t* altref_buffer_highbd_start[COLOR_CHANNELS] = {
        centre_pcs->altref_buffer_highbd[C_Y] + input_picture_ptr_central->org_y * input_picture_ptr_central->stride_y +
            input_picture_ptr_central->org_x,
        centre_pcs->altref_buffer_highbd[C_U] +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_bit_inc_cb +
            (input_picture_ptr_central->org_x >> ss_x),
        centre_pcs->altref_buffer_highbd[C_V] +
            (input_picture_ptr_central->org_y >> ss_y) * input_picture_ptr_central->stride_bit_inc_cr +
            (input_picture_ptr_central->org_x >> ss_x),
    };
    int decay_control;

    if (scs->vq_ctrls.sharpness_ctrls.tf && centre_pcs->is_noise_level && scs->calculate_variance &&
        centre_pcs->pic_avg_variance < VQ_PIC_AVG_VARIANCE_TH) {
        decay_control = 1;
    } else {
        // Hyper-parameter for filter weight adjustment.
        decay_control = 3;
        // Decrease the filter strength for low QPs
        if (scs->static_config.qp <= ALT_REF_QP_THRESH) {
            decay_control--;
        }
    }

    FP_ASSERT(TF_Q_DECAY_THRESHOLD == 20);
    const uint32_t q_decay_fp8 = 256;

    const int32_t const_0dot7_fp16 = 45875; //0.7
    /*Calculation of log and dceay_factor possible to move to estimate_noise() and calculate one time for GOP*/
    //decay_control * (0.7 + log1p(noise_levels[C_Y]))
    int32_t n_decay_fp10 = (decay_control * (const_0dot7_fp16 + noise_levels_log1p_fp16[C_Y])) / ((int32_t)1 << 6);
    //2 * n_decay * n_decay * q_decay * (s_decay always is 1);
    /*
     * TF STRENGTH CALCULATION (2)
     */
    // Get the frame update type for the current frame
    const uint32_t frame_update_type = svt_aom_get_frame_update_type(centre_pcs->scs, centre_pcs);

    if (scs->static_config.enable_tf > 1) {
        uint8_t adaptive_tf_shift_factor = calculate_tf_shift_factor(ctx);
        assert(adaptive_tf_shift_factor <= 14);
        const uint8_t kf_tf_shift_factor = CLIP3(0, 14, adaptive_tf_shift_factor + 1);
        assert(kf_tf_shift_factor <= 14);

        if (frame_update_type == SVT_AV1_KF_UPDATE && kf_tf_shift_factor == 14) {
            ctx->tf_decay_factor_fp16[C_Y] = 0;
            ctx->tf_decay_factor_fp16[C_U] = 0;
            ctx->tf_decay_factor_fp16[C_V] = 0;
        } else if (frame_update_type == SVT_AV1_KF_UPDATE) {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control,
                                           decay_control,
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           kf_tf_shift_factor,
                                           ctx->tf_chroma);
        } else {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control,
                                           decay_control,
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           adaptive_tf_shift_factor,
                                           ctx->tf_chroma);
        }
    } else {
        // tf_shift_factor is manually adjusted by the user via --tf-strength
        // 10 + (4 - 0) = 14 (8x weaker)
        // 10 + (4 - 1) = 13 (4x weaker)
        // 10 + (4 - 2) = 12 (2x weaker)
        // 10 + (4 - 3) = 11 (default)
        // 10 + (4 - 4) = 10 (2x stronger)
        const uint8_t tf_shift_factor = 10 + (4 - scs->static_config.tf_strength);
        assert(tf_shift_factor <= 14);

        // kf_tf_shift_factor is 1 + tf strength when using Tune 0 (VQ)
        uint8_t kf_tf_shift_factor = tf_shift_factor;
        if (scs->vq_ctrls.sharpness_ctrls.tf) {
            kf_tf_shift_factor = MIN(14, tf_shift_factor + 1);
        }
        assert(kf_tf_shift_factor <= 14);

        // when kf_tf_shift_factor is 14, we disable tf on keyframes
        if (frame_update_type == SVT_AV1_KF_UPDATE && kf_tf_shift_factor == 14) {
            ctx->tf_decay_factor_fp16[C_Y] = 0;
            ctx->tf_decay_factor_fp16[C_U] = 0;
            ctx->tf_decay_factor_fp16[C_V] = 0;
        } else if (frame_update_type == SVT_AV1_KF_UPDATE) {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control,
                                           decay_control,
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           kf_tf_shift_factor,
                                           ctx->tf_chroma);
        } else {
            svt_av1_calculate_decay_factor(ctx->tf_decay_factor_fp16,
                                           &n_decay_fp10,
                                           q_decay_fp8,
                                           decay_control,
                                           decay_control,
                                           const_0dot7_fp16,
                                           noise_levels_log1p_fp16,
                                           tf_shift_factor,
                                           ctx->tf_chroma);
        }
    }
    for (uint32_t blk_row = y_b64_start_idx; blk_row < y_b64_end_idx; blk_row++) {
        for (uint32_t blk_col = x_b64_start_idx; blk_col < x_b64_end_idx; blk_col++) {
            int blk_y_src_offset  = (blk_col * BW) + (blk_row * BH) * stride[C_Y];
            int blk_ch_src_offset = (blk_col * blk_width_ch) + (blk_row * blk_height_ch) * stride[C_U];

            // reset accumulator and count
            memset(accumulator, 0, BLK_PELS * COLOR_CHANNELS * sizeof(accumulator[0]));
            memset(counter, 0, BLK_PELS * COLOR_CHANNELS * sizeof(counter[0]));
            EbByte    src_center_ptr[COLOR_CHANNELS]           = {NULL};
            uint16_t* altref_buffer_highbd_ptr[COLOR_CHANNELS] = {NULL};
            // Prep 8bit source if 8bit content or using 8bit for subpel
            if (!is_highbd || ctx->tf_ctrls.use_8bit_subpel) {
                src_center_ptr[C_Y] = src_center_ptr_start[C_Y] + blk_y_src_offset;
                if (ctx->tf_chroma) {
                    src_center_ptr[C_U] = src_center_ptr_start[C_U] + blk_ch_src_offset;
                    src_center_ptr[C_V] = src_center_ptr_start[C_V] + blk_ch_src_offset;
                }
            }

            if (is_highbd) {
                altref_buffer_highbd_ptr[C_Y] = altref_buffer_highbd_start[C_Y] + blk_y_src_offset;
                if (ctx->tf_chroma) {
                    altref_buffer_highbd_ptr[C_U] = altref_buffer_highbd_start[C_U] + blk_ch_src_offset;
                    altref_buffer_highbd_ptr[C_V] = altref_buffer_highbd_start[C_V] + blk_ch_src_offset;
                }
            }

            if (!is_highbd) {
                apply_filtering_central(
                    ctx, input_picture_ptr_central, src_center_ptr, accum, count, BW, BH, ss_x, ss_y);
            }
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
            else {
                apply_filtering_central_highbd(
                    ctx, input_picture_ptr_central, altref_buffer_highbd_ptr, accum, count, BW, BH, ss_x, ss_y);
            }
#endif

            // for every frame to filter
            for (int frame_index = 0;
                 frame_index < (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes + 1);
                 frame_index++) {
                // ------------
                // Step 1: motion estimation + compensation
                // ------------
                me_context_ptr->me_ctx->tf_frame_index  = frame_index;
                me_context_ptr->me_ctx->tf_index_center = index_center;
                // if frame to process is the center frame
                if (frame_index != index_center) {
                    // Initialize ME context
                    create_me_context_and_picture_control(me_context_ptr,
                                                          pcs_list[frame_index],
                                                          pcs_list[index_center],
                                                          input_picture_ptr_central,
                                                          blk_row,
                                                          blk_col,
                                                          ss_x,
                                                          ss_y);
                    ctx->num_of_list_to_search       = 1;
                    ctx->num_of_ref_pic_to_search[0] = 1;
                    ctx->num_of_ref_pic_to_search[1] = 0;
                    ctx->temporal_layer_index        = centre_pcs->temporal_layer_index;
                    ctx->is_ref                      = centre_pcs->is_ref;

                    EbPaReferenceObject* ref_object                  = (EbPaReferenceObject*)ctx->alt_ref_reference_ptr;
                    ctx->me_ds_ref_array[0][0].picture_ptr           = ref_object->input_padded_pic;
                    ctx->me_ds_ref_array[0][0].sixteenth_picture_ptr = ref_object->sixteenth_downsampled_picture_ptr;
                    ctx->me_ds_ref_array[0][0].quarter_picture_ptr   = ref_object->quarter_downsampled_picture_ptr;
                    ctx->me_ds_ref_array[0][0].picture_number        = ref_object->picture_number;
                    ctx->tf_me_exit_th                               = centre_pcs->tf_ctrls.me_exit_th;
                    ctx->tf_use_pred_64x64_only_th                   = centre_pcs->tf_ctrls.use_pred_64x64_only_th;
                    ctx->tf_subpel_early_exit_th                     = centre_pcs->tf_ctrls.subpel_early_exit_th;
                    ctx->search_results[0][0].hme_sc_x               = 0;
                    ctx->search_results[0][0].hme_sc_y               = 0;

                    ctx->tf_64x64_mv_x = 0;
                    ctx->tf_64x64_mv_y = 0;

                    tf_64x64_inter_prediction(centre_pcs,
                                              ctx,
                                              pcs_list[frame_index],
                                              list_input_picture_ptr[frame_index],
                                              pred,
                                              pred_16bit,
                                              (uint32_t)blk_col * BW,
                                              (uint32_t)blk_row * BH,
                                              ss_x,
                                              encoder_bit_depth);

                    ctx->tf_32x32_mv_x[0] = ctx->tf_64x64_mv_x;
                    ctx->tf_32x32_mv_y[0] = ctx->tf_64x64_mv_y;

                    ctx->tf_32x32_mv_x[1] = ctx->tf_64x64_mv_x;
                    ctx->tf_32x32_mv_y[1] = ctx->tf_64x64_mv_y;

                    ctx->tf_32x32_mv_x[2] = ctx->tf_64x64_mv_x;
                    ctx->tf_32x32_mv_y[2] = ctx->tf_64x64_mv_y;

                    ctx->tf_32x32_mv_x[3] = ctx->tf_64x64_mv_x;
                    ctx->tf_32x32_mv_y[3] = ctx->tf_64x64_mv_y;

                    ctx->tf_32x32_block_split_flag[0] = 0;
                    ctx->tf_32x32_block_split_flag[1] = 0;
                    ctx->tf_32x32_block_split_flag[2] = 0;
                    ctx->tf_32x32_block_split_flag[3] = 0;

                    // Update the 32x32 block-error
                    for (int block_row = 0; block_row < 2; block_row++) {
                        for (int block_col = 0; block_col < 2; block_col++) {
                            uint32_t bsize = 32;
                            uint64_t distortion;
                            ctx->idx_32x32 = block_col + (block_row << 1);

                            if (!is_highbd) {
                                uint8_t* pred_y_ptr = pred[C_Y] + bsize * block_row * stride_pred[C_Y] +
                                    bsize * block_col;
                                uint8_t* src_y_ptr = src_center_ptr[C_Y] + bsize * block_row * stride[C_Y] +
                                    bsize * block_col;

                                const AomVarianceFnPtr* fn_ptr = centre_pcs->tf_ctrls.sub_sampling_shift
                                    ? &svt_aom_mefn_ptr[BLOCK_32X16]
                                    : &svt_aom_mefn_ptr[BLOCK_32X32];
                                unsigned int            sse;
                                distortion = fn_ptr->vf(pred_y_ptr,
                                                        stride_pred[C_Y] << centre_pcs->tf_ctrls.sub_sampling_shift,
                                                        src_y_ptr,
                                                        stride[C_Y] << centre_pcs->tf_ctrls.sub_sampling_shift,
                                                        &sse)
                                    << centre_pcs->tf_ctrls.sub_sampling_shift;
                            } else {
                                uint16_t* pred_y_ptr = pred_16bit[C_Y] + bsize * block_row * stride_pred[C_Y] +
                                    bsize * block_col;
                                uint16_t* src_y_ptr = altref_buffer_highbd_ptr[C_Y] + bsize * block_row * stride[C_Y] +
                                    bsize * block_col;
                                const AomVarianceFnPtr* fn_ptr = centre_pcs->tf_ctrls.sub_sampling_shift
                                    ? &svt_aom_mefn_ptr[BLOCK_32X16]
                                    : &svt_aom_mefn_ptr[BLOCK_32X32];

                                unsigned int sse;

                                distortion = fn_ptr->vf_hbd_10(
                                                 CONVERT_TO_BYTEPTR(pred_y_ptr),
                                                 stride_pred[C_Y] << centre_pcs->tf_ctrls.sub_sampling_shift,
                                                 CONVERT_TO_BYTEPTR(src_y_ptr),
                                                 stride[C_Y] << centre_pcs->tf_ctrls.sub_sampling_shift,
                                                 &sse)
                                    << centre_pcs->tf_ctrls.sub_sampling_shift;
                            }
                            ctx->tf_32x32_block_error[ctx->idx_32x32] = distortion;
                        }
                    }

                    for (int block_row = 0; block_row < 2; block_row++) {
                        for (int block_col = 0; block_col < 2; block_col++) {
                            ctx->tf_block_col = block_col;
                            ctx->tf_block_row = block_row;

                            apply_filtering_block_plane_wise(ctx,
                                                             block_row,
                                                             block_col,
                                                             src_center_ptr,
                                                             altref_buffer_highbd_ptr,
                                                             pred,
                                                             pred_16bit,
                                                             accum,
                                                             count,
                                                             stride,
                                                             stride_pred,
                                                             BW >> 1,
                                                             BH >> 1,
                                                             ss_x,
                                                             ss_y,
                                                             encoder_bit_depth);
                        }
                    }
                }
            }

            // Normalize filter output to produce temporally filtered frame
            get_final_filtered_pixels(ctx,
                                      src_center_ptr_start,
                                      altref_buffer_highbd_start,
                                      accum,
                                      count,
                                      stride,
                                      blk_y_src_offset,
                                      blk_ch_src_offset,
                                      blk_width_ch,
                                      blk_height_ch,
                                      is_highbd);
        }
    }
    // Prep 8bit source if 8bit content or using 8bit for subpel
    if (!is_highbd || ctx->tf_ctrls.use_8bit_subpel) {
        EB_FREE_ALIGNED_ARRAY(predictor);
    }

    if (is_highbd) {
        EB_FREE_ALIGNED_ARRAY(predictor_16bit);
    }
    return EB_ErrorNone;
}

// This is an adaptation of the mehtod in the following paper:
// Shen-Chuan Tai, Shih-Ming Yang, "A fast method for image noise
// estimation using Laplacian operator and adaptive edge detection,"
// Proc. 3rd International Symposium on Communications, Control and
// Signal Processing, 2008, St Julians, Malta.
// Return noise estimate, or -1.0 if there was a failure
// function from libaom
// Standard bit depht input (=8 bits) to estimate the noise, I don't think there needs to be two methods for this
// Operates on the Y component only
int32_t svt_estimate_noise_fp16_c(const uint8_t* src, uint16_t width, uint16_t height, uint16_t stride_y) {
    int64_t sum = 0;
    int64_t num = 0;

    for (int i = 1; i < height - 1; ++i) {
        for (int j = 1; j < width - 1; ++j) {
            const int k = i * stride_y + j;
            // Sobel gradients
            const int g_x = (src[k - stride_y - 1] - src[k - stride_y + 1]) +
                (src[k + stride_y - 1] - src[k + stride_y + 1]) + 2 * (src[k - 1] - src[k + 1]);
            const int g_y = (src[k - stride_y - 1] - src[k + stride_y - 1]) +
                (src[k - stride_y + 1] - src[k + stride_y + 1]) + 2 * (src[k - stride_y] - src[k + stride_y]);
            const int ga = abs(g_x) + abs(g_y);
            if (ga < EDGE_THRESHOLD) { // Do not consider edge pixels to estimate the noise
                // Find Laplacian
                const int v = 4 * src[k] - 2 * (src[k - 1] + src[k + 1] + src[k - stride_y] + src[k + stride_y]) +
                    (src[k - stride_y - 1] + src[k - stride_y + 1] + src[k + stride_y - 1] + src[k + stride_y + 1]);
                sum += abs(v);
                ++num;
            }
        }
    }
    // If very few smooth pels, return -1 since the estimate is unreliable
    if (num < SMOOTH_THRESHOLD) {
        return -65536 /*-1:fp16*/;
    }

    FP_ASSERT((((int64_t)sum * SQRT_PI_BY_2_FP16) / (6 * num)) < ((int64_t)1 << 31));
    return (int32_t)((sum * SQRT_PI_BY_2_FP16) / (6 * num));
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
// Noise estimation for highbd
int32_t svt_estimate_noise_highbd_fp16_c(const uint16_t* src, int width, int height, int stride, int bd) {
    int64_t sum = 0;
    int64_t num = 0;

    for (int i = 1; i < height - 1; ++i) {
        for (int j = 1; j < width - 1; ++j) {
            const int k = i * stride + j;
            // Sobel gradients
            const int g_x = (src[k - stride - 1] - src[k - stride + 1]) + (src[k + stride - 1] - src[k + stride + 1]) +
                2 * (src[k - 1] - src[k + 1]);
            const int g_y = (src[k - stride - 1] - src[k + stride - 1]) + (src[k - stride + 1] - src[k + stride + 1]) +
                2 * (src[k - stride] - src[k + stride]);
            const int ga = ROUND_POWER_OF_TWO(abs(g_x) + abs(g_y),
                                              bd - 8); // divide by 2^2 and round up
            if (ga < EDGE_THRESHOLD) { // Do not consider edge pixels to estimate the noise
                // Find Laplacian
                const int v = 4 * src[k] - 2 * (src[k - 1] + src[k + 1] + src[k - stride] + src[k + stride]) +
                    (src[k - stride - 1] + src[k - stride + 1] + src[k + stride - 1] + src[k + stride + 1]);
                sum += ROUND_POWER_OF_TWO(abs(v), bd - 8);
                ++num;
            }
        }
    }
    // If very few smooth pels, return -1 since the estimate is unreliable
    if (num < SMOOTH_THRESHOLD) {
        return -65536 /*-1:fp16*/;
    }

    FP_ASSERT((((int64_t)sum * SQRT_PI_BY_2_FP16) / (6 * num)) < ((int64_t)1 << 31));
    return (int32_t)((sum * SQRT_PI_BY_2_FP16) / (6 * num));
}
#endif

void pad_and_decimate_filtered_pic(PictureParentControlSet* centre_pcs) {
    // reference structures (padded pictures + downsampled versions)
    SequenceControlSet*  scs        = centre_pcs->scs;
    EbPaReferenceObject* src_object = (EbPaReferenceObject*)centre_pcs->pa_ref_pic_wrapper->object_ptr;
    EbPictureBufferDesc* input_pic  = centre_pcs->enhanced_pic;

    // Refine the non-8 padding
    if (((input_pic->width - scs->pad_right) % 8 != 0) || ((input_pic->height - scs->pad_bottom) % 8 != 0)) {
        svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions(scs, input_pic);
    }

    //Generate padding first, then copy
    svt_aom_generate_padding(&(input_pic->buffer_y[C_Y]),
                             input_pic->stride_y,
                             input_pic->width,
                             input_pic->height,
                             input_pic->org_x,
                             input_pic->org_y);
    // Padding chroma after altref
    if (centre_pcs->tf_ctrls.chroma_lvl) {
        svt_aom_generate_padding(input_pic->buffer_cb,
                                 input_pic->stride_cb,
                                 input_pic->width >> scs->subsampling_x,
                                 input_pic->height >> scs->subsampling_y,
                                 input_pic->org_x >> scs->subsampling_x,
                                 input_pic->org_y >> scs->subsampling_y);
        svt_aom_generate_padding(input_pic->buffer_cr,
                                 input_pic->stride_cr,
                                 input_pic->width >> scs->subsampling_x,
                                 input_pic->height >> scs->subsampling_y,
                                 input_pic->org_x >> scs->subsampling_x,
                                 input_pic->org_y >> scs->subsampling_y);
    }

    // 1/4 & 1/16 input picture downsampling
    svt_aom_downsample_filtering_input_picture(centre_pcs,
                                               input_pic,
                                               src_object->quarter_downsampled_picture_ptr,
                                               src_object->sixteenth_downsampled_picture_ptr);
}

// save original enchanced_picture_ptr buffer in a separate buffer (to be replaced by the temporally filtered pic)
static EbErrorType save_src_pic_buffers(PictureParentControlSet* centre_pcs, uint32_t ss_y, bool is_highbd) {
    // save buffer from full size frame enhanced_unscaled_pic
    EbPictureBufferDesc* src_pic_ptr = centre_pcs->enhanced_unscaled_pic;
    assert(src_pic_ptr != NULL);
    // allocate memory for the copy of the original enhanced buffer
    EB_MALLOC_ARRAY(centre_pcs->save_source_picture_ptr[C_Y], src_pic_ptr->luma_size);
    EB_MALLOC_ARRAY(centre_pcs->save_source_picture_ptr[C_U], src_pic_ptr->chroma_size);
    EB_MALLOC_ARRAY(centre_pcs->save_source_picture_ptr[C_V], src_pic_ptr->chroma_size);

    // if highbd, allocate memory for the copy of the original enhanced buffer - bit inc
    if (is_highbd) {
        EB_MALLOC_ARRAY(centre_pcs->save_source_picture_bit_inc_ptr[C_Y], src_pic_ptr->luma_size);
        EB_MALLOC_ARRAY(centre_pcs->save_source_picture_bit_inc_ptr[C_U], src_pic_ptr->chroma_size);
        EB_MALLOC_ARRAY(centre_pcs->save_source_picture_bit_inc_ptr[C_V], src_pic_ptr->chroma_size);
    }
    centre_pcs->save_source_picture_width  = src_pic_ptr->width;
    centre_pcs->save_source_picture_height = src_pic_ptr->height;

    // copy buffers
    // Y
    uint32_t height_y  = (uint32_t)(src_pic_ptr->height + src_pic_ptr->org_y + src_pic_ptr->origin_bot_y);
    uint32_t height_uv = (uint32_t)((src_pic_ptr->height + src_pic_ptr->org_y + src_pic_ptr->origin_bot_y) >> ss_y);

    assert(height_y * src_pic_ptr->stride_y == src_pic_ptr->luma_size);
    assert(height_uv * src_pic_ptr->stride_cb == src_pic_ptr->chroma_size);
    assert(height_uv * src_pic_ptr->stride_cr == src_pic_ptr->chroma_size);

    svt_av1_copy_wxh_8bit(src_pic_ptr->buffer_y,
                          src_pic_ptr->stride_y,
                          centre_pcs->save_source_picture_ptr[C_Y],
                          src_pic_ptr->stride_y,
                          height_y,
                          src_pic_ptr->stride_y);

    svt_av1_copy_wxh_8bit(src_pic_ptr->buffer_cb,
                          src_pic_ptr->stride_cb,
                          centre_pcs->save_source_picture_ptr[C_U],
                          src_pic_ptr->stride_cb,
                          height_uv,
                          src_pic_ptr->stride_cb);

    svt_av1_copy_wxh_8bit(src_pic_ptr->buffer_cr,
                          src_pic_ptr->stride_cr,
                          centre_pcs->save_source_picture_ptr[C_V],
                          src_pic_ptr->stride_cr,
                          height_uv,
                          src_pic_ptr->stride_cr);

    if (is_highbd) {
        // if highbd, copy bit inc buffers
        // Y
        svt_c_unpack_compressed_10bit(centre_pcs->enhanced_pic->buffer_bit_inc_y,
                                      centre_pcs->enhanced_pic->stride_bit_inc_y / 4,
                                      centre_pcs->save_source_picture_bit_inc_ptr[C_Y],
                                      centre_pcs->enhanced_pic->stride_bit_inc_y,
                                      height_y);
        // U
        svt_c_unpack_compressed_10bit(centre_pcs->enhanced_pic->buffer_bit_inc_cb,
                                      centre_pcs->enhanced_pic->stride_bit_inc_cb / 4,
                                      centre_pcs->save_source_picture_bit_inc_ptr[C_U],
                                      centre_pcs->enhanced_pic->stride_bit_inc_cb,
                                      height_uv);
        // V
        svt_c_unpack_compressed_10bit(centre_pcs->enhanced_pic->buffer_bit_inc_cr,
                                      centre_pcs->enhanced_pic->stride_bit_inc_cr / 4,
                                      centre_pcs->save_source_picture_bit_inc_ptr[C_V],
                                      centre_pcs->enhanced_pic->stride_bit_inc_cr,
                                      height_uv);
    }

    return EB_ErrorNone;
}

static EbErrorType save_y_src_pic_buffers(PictureParentControlSet* centre_pcs, bool is_highbd) {
    // save buffer from full size frame enhanced_unscaled_pic
    EbPictureBufferDesc* src_pic_ptr = centre_pcs->enhanced_unscaled_pic;
    assert(src_pic_ptr != NULL);
    // allocate memory for the copy of the original enhanced buffer
    EB_MALLOC_ARRAY(centre_pcs->save_source_picture_ptr[C_Y], src_pic_ptr->luma_size);

    // if highbd, allocate memory for the copy of the original enhanced buffer - bit inc
    if (is_highbd) {
        EB_MALLOC_ARRAY(centre_pcs->save_source_picture_bit_inc_ptr[C_Y], src_pic_ptr->luma_size);
    }
    centre_pcs->save_source_picture_width  = src_pic_ptr->width;
    centre_pcs->save_source_picture_height = src_pic_ptr->height;

    // copy buffers
    // Y
    uint32_t height_y = (uint32_t)(src_pic_ptr->height + src_pic_ptr->org_y + src_pic_ptr->origin_bot_y);

    assert(height_y * src_pic_ptr->stride_y == src_pic_ptr->luma_size);

    svt_av1_copy_wxh_8bit(src_pic_ptr->buffer_y,
                          src_pic_ptr->stride_y,
                          centre_pcs->save_source_picture_ptr[C_Y],
                          src_pic_ptr->stride_y,
                          height_y,
                          src_pic_ptr->stride_y);

    if (is_highbd) {
        // if highbd, copy bit inc buffers
        // Y
        svt_c_unpack_compressed_10bit(centre_pcs->enhanced_pic->buffer_bit_inc_y,
                                      centre_pcs->enhanced_pic->stride_bit_inc_y / 4,
                                      centre_pcs->save_source_picture_bit_inc_ptr[C_Y],
                                      centre_pcs->enhanced_pic->stride_bit_inc_y,
                                      height_y);
    }

    return EB_ErrorNone;
}

static uint32_t filt_unfilt_dist(PictureParentControlSet* ppcs, EbByte filt, EbByte unfil, uint16_t stride_y,
                                 bool is_highbd) {
    uint32_t pic_width_in_b64  = (ppcs->aligned_width + ppcs->scs->b64_size - 1) / ppcs->scs->b64_size;
    uint32_t pic_height_in_b64 = (ppcs->aligned_height + ppcs->scs->b64_size - 1) / ppcs->scs->b64_size;

    EbSpatialFullDistType spatial_full_dist_type_fun = is_highbd ? svt_full_distortion_kernel16_bits
                                                                 : svt_spatial_full_distortion_kernel;

    uint32_t dist = 0;
    for (uint32_t y_b64_idx = 0; y_b64_idx < pic_height_in_b64; ++y_b64_idx) {
        for (uint32_t x_b64_idx = 0; x_b64_idx < pic_width_in_b64; ++x_b64_idx) {
            uint32_t b64_origin_x = x_b64_idx * 64;
            uint32_t b64_origin_y = y_b64_idx * 64;

            uint32_t buffer_index = b64_origin_y * stride_y + b64_origin_x;

            dist += (uint32_t)(spatial_full_dist_type_fun(
                filt, buffer_index, stride_y, unfil, buffer_index, stride_y, ppcs->scs->b64_size, ppcs->scs->b64_size));
        }
    }
    return (dist / (pic_width_in_b64 * pic_height_in_b64));
}

EbErrorType svt_av1_init_temporal_filtering(PictureParentControlSet** pcs_list, PictureParentControlSet* centre_pcs,
                                            MotionEstimationContext_t* me_context_ptr, int32_t segment_index) {
    uint8_t              index_center;
    EbPictureBufferDesc* central_picture_ptr;
    me_context_ptr->me_ctx->tf_ctrls = centre_pcs->tf_ctrls;

    bool high_chroma_noise_lvl = (centre_pcs->noise_levels_log1p_fp16[0] < centre_pcs->noise_levels_log1p_fp16[1] ||
                                  centre_pcs->noise_levels_log1p_fp16[0] < centre_pcs->noise_levels_log1p_fp16[2])
        ? true
        : false;
    me_context_ptr->me_ctx->tf_chroma = centre_pcs->tf_ctrls.chroma_lvl == 1 ? 1
        : centre_pcs->tf_ctrls.chroma_lvl == 2 && high_chroma_noise_lvl      ? 1
                                                                             : 0;

    me_context_ptr->me_ctx->tf_tot_horz_blks = me_context_ptr->me_ctx->tf_tot_vert_blks = 0;
    // index of the central source frame
    index_center = centre_pcs->past_altref_nframes;

    // if this assertion does not fail (as I think it should not, then remove centre_pcs from the input parameters of init_temporal_filtering())
    assert(pcs_list[index_center] == centre_pcs);

    // source central frame picture buffer
    central_picture_ptr = centre_pcs->enhanced_pic;

    uint32_t encoder_bit_depth = centre_pcs->scs->static_config.encoder_bit_depth;
    bool     is_highbd         = (encoder_bit_depth == 8) ? (uint8_t)false : (uint8_t)true;

    // chroma subsampling
    uint32_t ss_x                    = centre_pcs->scs->subsampling_x;
    uint32_t ss_y                    = centre_pcs->scs->subsampling_y;
    int32_t* noise_levels_log1p_fp16 = &(centre_pcs->noise_levels_log1p_fp16[0]);

    //only one performs any picture based prep
    svt_block_on_mutex(centre_pcs->temp_filt_mutex);
    if (centre_pcs->temp_filt_prep_done == 0) {
        centre_pcs->temp_filt_prep_done = 1;
        // Pad chroma reference samples - once only per picture
        for (int i = 0; i < (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes + 1); i++) {
            EbPictureBufferDesc* pic_ptr_ref = pcs_list[i]->enhanced_pic;
            //10bit: for all the reference pictures do the packing once at the beggining.
            if (is_highbd && i != centre_pcs->past_altref_nframes) {
                EB_MALLOC_ARRAY(pcs_list[i]->altref_buffer_highbd[C_Y], central_picture_ptr->luma_size);
                if (centre_pcs->tf_ctrls.chroma_lvl) {
                    EB_MALLOC_ARRAY(pcs_list[i]->altref_buffer_highbd[C_U], central_picture_ptr->chroma_size);
                    EB_MALLOC_ARRAY(pcs_list[i]->altref_buffer_highbd[C_V], central_picture_ptr->chroma_size);
                }
                // pack byte buffers to 16 bit buffer
                svt_aom_pack_highbd_pic(pic_ptr_ref, pcs_list[i]->altref_buffer_highbd, ss_x, ss_y, true);
            }
        }

        centre_pcs->do_tf = true; // set temporal filtering flag ON for current picture

        // save original source picture (to be replaced by the temporally filtered pic)
        // if PSNR or SSIM computation needed or if superres recode is enabled
        SUPERRES_MODE             superres_mode           = centre_pcs->scs->static_config.superres_mode;
        SUPERRES_AUTO_SEARCH_TYPE search_type             = centre_pcs->scs->static_config.superres_auto_search_type;
        uint32_t                  frame_update_type       = svt_aom_get_frame_update_type(centre_pcs->scs, centre_pcs);
        bool                      superres_recode_enabled = (superres_mode == SUPERRES_AUTO) &&
            ((search_type == SUPERRES_AUTO_DUAL) || (search_type == SUPERRES_AUTO_ALL)) // auto-dual or auto-all
            && ((frame_update_type == SVT_AV1_KF_UPDATE) ||
                (frame_update_type == SVT_AV1_ARF_UPDATE)); // recode only applies to key and arf
        if ((centre_pcs->compute_psnr || centre_pcs->compute_ssim) || superres_recode_enabled) {
            save_src_pic_buffers(centre_pcs, ss_y, is_highbd);
        } else if (centre_pcs->slice_type == I_SLICE) {
            save_y_src_pic_buffers(centre_pcs, is_highbd);
        }
    }
    svt_release_mutex(centre_pcs->temp_filt_mutex);
    me_context_ptr->me_ctx->tf_mv_dist_th = CLIP3(
        64, 450, (int)((int)MIN(centre_pcs->aligned_height, centre_pcs->aligned_width) - 150));
    // index of the central source frame
    // index_center = centre_pcs->past_altref_nframes;
    // populate source frames picture buffer list
    EbPictureBufferDesc* list_input_picture_ptr[ALTREF_MAX_NFRAMES] = {NULL};
    for (int i = 0; i < (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes + 1); i++) {
        list_input_picture_ptr[i] = pcs_list[i]->enhanced_unscaled_pic;
    }

    if (centre_pcs->scs->static_config.pred_structure == LOW_DELAY) {
        produce_temporally_filtered_pic_ld(pcs_list,
                                           list_input_picture_ptr,
                                           index_center,
                                           me_context_ptr,
                                           noise_levels_log1p_fp16,
                                           segment_index,
                                           is_highbd);
    } else {
        produce_temporally_filtered_pic(pcs_list,
                                        list_input_picture_ptr,
                                        index_center,
                                        me_context_ptr,
                                        noise_levels_log1p_fp16,
                                        segment_index,
                                        is_highbd);
    }

    svt_block_on_mutex(centre_pcs->temp_filt_mutex);
    centre_pcs->temp_filt_seg_acc++;

    centre_pcs->tf_tot_horz_blks += me_context_ptr->me_ctx->tf_tot_horz_blks;
    centre_pcs->tf_tot_vert_blks += me_context_ptr->me_ctx->tf_tot_vert_blks;

    if (centre_pcs->temp_filt_seg_acc == centre_pcs->tf_segments_total_count) {
#if DEBUG_TF
        if (!is_highbd) {
            save_YUV_to_file("filtered_picture.yuv",
                             central_picture_ptr->buffer_y,
                             central_picture_ptr->buffer_cb,
                             central_picture_ptr->buffer_cr,
                             central_picture_ptr->width,
                             central_picture_ptr->height,
                             central_picture_ptr->stride_y,
                             central_picture_ptr->stride_cb,
                             central_picture_ptr->stride_cr,
                             central_picture_ptr->org_y,
                             central_picture_ptr->org_x,
                             ss_x,
                             ss_y);
        } else {
            save_YUV_to_file_highbd("filtered_picture.yuv",
                                    centre_pcs->altref_buffer_highbd[C_Y],
                                    centre_pcs->altref_buffer_highbd[C_U],
                                    centre_pcs->altref_buffer_highbd[C_V],
                                    central_picture_ptr->width,
                                    central_picture_ptr->height,
                                    central_picture_ptr->stride_y,
                                    central_picture_ptr->stride_cb,
                                    central_picture_ptr->stride_cb,
                                    central_picture_ptr->org_y,
                                    central_picture_ptr->org_x,
                                    ss_x,
                                    ss_y);
        }
#endif
        if (is_highbd) {
            svt_aom_unpack_highbd_pic(centre_pcs->altref_buffer_highbd, central_picture_ptr, ss_x, ss_y, true);
            EB_FREE_ARRAY(centre_pcs->altref_buffer_highbd[C_Y]);
            if (centre_pcs->tf_ctrls.chroma_lvl) {
                EB_FREE_ARRAY(centre_pcs->altref_buffer_highbd[C_U]);
                EB_FREE_ARRAY(centre_pcs->altref_buffer_highbd[C_V]);
            }
            for (int i = 0; i < (centre_pcs->past_altref_nframes + centre_pcs->future_altref_nframes + 1); i++) {
                if (i != centre_pcs->past_altref_nframes) {
                    EB_FREE_ARRAY(pcs_list[i]->altref_buffer_highbd[C_Y]);
                    if (centre_pcs->tf_ctrls.chroma_lvl) {
                        EB_FREE_ARRAY(pcs_list[i]->altref_buffer_highbd[C_U]);
                        EB_FREE_ARRAY(pcs_list[i]->altref_buffer_highbd[C_V]);
                    }
                }
            }
        }

        // padding + decimation: even if highbd src, this is only performed on the 8 bit buffer (excluding the LSBs)
        pad_and_decimate_filtered_pic(centre_pcs);
        if (centre_pcs->slice_type == I_SLICE) {
            EbPictureBufferDesc* input_pic = centre_pcs->enhanced_pic;
            EbByte               filt = input_pic->buffer_y + input_pic->org_y * input_pic->stride_y + input_pic->org_x;
            EbByte unfil = centre_pcs->save_source_picture_ptr[C_Y] + input_pic->org_y * input_pic->stride_y +
                input_pic->org_x;

            centre_pcs->filt_to_unfilt_diff = filt_unfilt_dist(
                centre_pcs, filt, unfil, centre_pcs->enhanced_pic->stride_y, false);
        }

        // signal that temp filt is done
        svt_post_semaphore(centre_pcs->temp_filt_done_semaphore);
    }

    svt_release_mutex(centre_pcs->temp_filt_mutex);

    return EB_ErrorNone;
}
