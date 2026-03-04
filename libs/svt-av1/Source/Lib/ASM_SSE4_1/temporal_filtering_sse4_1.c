/*
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <assert.h>
#include <smmintrin.h> /* SSE4.1 */
#include <emmintrin.h>

#include "definitions.h"
#include "temporal_filtering_constants.h"
#include "utility.h"

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

// T[X] =  exp(-(X)/16)  for x in [0..7], step 1/16 values in Fixed Points shift 16
static const int32_t expf_tab_fp16[] = {
    65536, 61565, 57835, 54331, 51039, 47947, 45042, 42313, 39749, 37341, 35078, 32953, 30957, 29081, 27319,
    25664, 24109, 22648, 21276, 19987, 18776, 17638, 16570, 15566, 14623, 13737, 12904, 12122, 11388, 10698,
    10050, 9441,  8869,  8331,  7827,  7352,  6907,  6488,  6095,  5726,  5379,  5053,  4747,  4459,  4189,
    3935,  3697,  3473,  3262,  3065,  2879,  2704,  2541,  2387,  2242,  2106,  1979,  1859,  1746,  1640,
    1541,  1447,  1360,  1277,  1200,  1127,  1059,  995,   934,   878,   824,   774,   728,   683,   642,
    603,   566,   532,   500,   470,   441,   414,   389,   366,   343,   323,   303,   285,   267,   251,
    236,   222,   208,   195,   184,   172,   162,   152,   143,   134,   126,   118,   111,   104,   98,
    92,    86,    81,    76,    72,    67,    63,    59,    56,    52,    49,    46,    43,    41,    38,
    36,    34,    31,    30,    28,    26,    24,    23,    21};

#define SSE_STRIDE (BW + 2)

static uint32_t calculate_squared_errors_sum_no_div_sse4_1(const uint8_t* s, int s_stride, const uint8_t* p,
                                                           int p_stride, unsigned int w, unsigned int h) {
    assert(w % 16 == 0 && "block width must be multiple of 16");
    unsigned int i, j;

    __m128i sum = _mm_setzero_si128();
    __m128i s_8, p_8, dif;

    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 8) {
            s_8 = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)(s + i * s_stride + j)));
            p_8 = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)(p + i * p_stride + j)));

            dif = _mm_sub_epi16(s_8, p_8);
            sum = _mm_add_epi32(sum, _mm_madd_epi16(dif, dif));
        }
    }

    sum = _mm_hadd_epi32(sum, sum);
    sum = _mm_hadd_epi32(sum, sum);

    return _mm_cvtsi128_si32(sum);
}

/*This function return 2 separate squared errors for two block 8xh, return value is stored in output array*/
static void calculate_squared_errors_sum_2x8xh_no_div_sse4_1(const uint8_t* s, int s_stride, const uint8_t* p,
                                                             int p_stride, unsigned int h, uint32_t* output) {
    unsigned int i;

    __m128i sum[2] = {_mm_setzero_si128()};
    __m128i s_8[2], p_8[2], dif[2];

    for (i = 0; i < h; i++) {
        s_8[0] = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)(s + i * s_stride)));
        s_8[1] = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)(s + i * s_stride + 8)));
        p_8[0] = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)(p + i * p_stride)));
        p_8[1] = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)(p + i * p_stride + 8)));

        dif[0] = _mm_sub_epi16(s_8[0], p_8[0]);
        dif[1] = _mm_sub_epi16(s_8[1], p_8[1]);
        sum[0] = _mm_add_epi32(sum[0], _mm_madd_epi16(dif[0], dif[0]));
        sum[1] = _mm_add_epi32(sum[1], _mm_madd_epi16(dif[1], dif[1]));
    }
    sum[0] = _mm_hadd_epi32(sum[0], sum[0]);
    sum[0] = _mm_hadd_epi32(sum[0], sum[0]);
    sum[1] = _mm_hadd_epi32(sum[1], sum[1]);
    sum[1] = _mm_hadd_epi32(sum[1], sum[1]);

    output[0] = _mm_cvtsi128_si32(sum[0]);
    output[1] = _mm_cvtsi128_si32(sum[1]);
}

static uint32_t calculate_squared_errors_sum_no_div_highbd_sse4_1(const uint16_t* s, int s_stride, const uint16_t* p,
                                                                  int p_stride, unsigned int w, unsigned int h,
                                                                  int shift_factor) {
    assert(w % 16 == 0 && "block width must be multiple of 16");
    unsigned int i, j;

    __m128i sum = _mm_setzero_si128();
    __m128i s_16, p_16, dif;

    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 8) {
            s_16 = _mm_loadu_si128((const __m128i*)(s + i * s_stride + j));
            p_16 = _mm_loadu_si128((const __m128i*)(p + i * p_stride + j));

            dif = _mm_sub_epi16(s_16, p_16);
            sum = _mm_add_epi32(sum, _mm_madd_epi16(dif, dif));
        }
    }

    sum = _mm_hadd_epi32(sum, sum);
    sum = _mm_hadd_epi32(sum, sum);

    return _mm_cvtsi128_si32(sum) >> shift_factor;
}

/*This function return 2 separate squared errors for two block 8xh, return value is stored in output array*/
static void calculate_squared_errors_sum_2x8xh_no_div_highbd_sse4_1(const uint16_t* s, int s_stride, const uint16_t* p,
                                                                    int p_stride, unsigned int h, int shift_factor,
                                                                    uint32_t* output) {
    unsigned int i;

    __m128i sum[2] = {_mm_setzero_si128()};
    __m128i s_8[2], p_8[2], dif[2];

    for (i = 0; i < h; i++) {
        s_8[0] = _mm_loadu_si128((const __m128i*)(s + i * s_stride));
        s_8[1] = _mm_loadu_si128((const __m128i*)(s + i * s_stride + 8));
        p_8[0] = _mm_loadu_si128((const __m128i*)(p + i * p_stride));
        p_8[1] = _mm_loadu_si128((const __m128i*)(p + i * p_stride + 8));

        dif[0] = _mm_sub_epi16(s_8[0], p_8[0]);
        dif[1] = _mm_sub_epi16(s_8[1], p_8[1]);

        sum[0] = _mm_add_epi32(sum[0], _mm_madd_epi16(dif[0], dif[0]));
        sum[1] = _mm_add_epi32(sum[1], _mm_madd_epi16(dif[1], dif[1]));
    }
    sum[0] = _mm_hadd_epi32(sum[0], sum[0]);
    sum[0] = _mm_hadd_epi32(sum[0], sum[0]);
    sum[1] = _mm_hadd_epi32(sum[1], sum[1]);
    sum[1] = _mm_hadd_epi32(sum[1], sum[1]);

    output[0] = _mm_cvtsi128_si32(sum[0]) >> shift_factor;
    output[1] = _mm_cvtsi128_si32(sum[1]) >> shift_factor;
}

static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_sse4_1(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor) {
    unsigned int i, j, k, subblock_idx;

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

    __m128i adjusted_weight_int16[4];
    __m128i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16 = AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm_set1_epi32((int32_t)(adjusted_weight));
    }

    for (i = 0; i < block_height; i++) {
        const int subblock_idx_h = (i >= block_height / 2) * 2;
        for (j = 0; j < block_width; j += 8) {
            k = i * y_pre_stride + j;

            //y_count[k] += adjusted_weight;
            __m128i count_array = _mm_loadu_si128((__m128i*)(y_count + k));
            count_array = _mm_add_epi16(count_array, adjusted_weight_int16[subblock_idx_h + (j >= block_width / 2)]);
            _mm_storeu_si128((__m128i*)(y_count + k), count_array);

            //y_accum[k] += adjusted_weight * pixel_value;
            __m128i accumulator_array1 = _mm_loadu_si128((__m128i*)(y_accum + k));
            __m128i accumulator_array2 = _mm_loadu_si128((__m128i*)(y_accum + k + 4));
            __m128i frame2_array       = _mm_loadl_epi64((__m128i*)(y_pre + k));
            frame2_array               = _mm_cvtepu8_epi16(frame2_array);
            __m128i frame2_array_u32_1 = _mm_cvtepi16_epi32(frame2_array);
            __m128i frame2_array_u32_2 = _mm_cvtepi16_epi32(_mm_srli_si128(frame2_array, 8));
            frame2_array_u32_1         = _mm_mullo_epi32(frame2_array_u32_1,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);
            frame2_array_u32_2         = _mm_mullo_epi32(frame2_array_u32_2,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array1 = _mm_add_epi32(accumulator_array1, frame2_array_u32_1);
            accumulator_array2 = _mm_add_epi32(accumulator_array2, frame2_array_u32_2);
            _mm_storeu_si128((__m128i*)(y_accum + k), accumulator_array1);
            _mm_storeu_si128((__m128i*)(y_accum + k + 4), accumulator_array2);
        }
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_sse4_1(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_sse4_1(me_ctx,
                                                                           y_pre,
                                                                           y_pre_stride,
                                                                           (unsigned int)block_width,
                                                                           (unsigned int)block_height,
                                                                           y_accum,
                                                                           y_count,
                                                                           me_ctx->tf_decay_factor_fp16[C_Y]);

    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_sse4_1(me_ctx,
                                                                               u_pre,
                                                                               uv_pre_stride,
                                                                               (unsigned int)block_width >> ss_x,
                                                                               (unsigned int)block_height >> ss_y,
                                                                               u_accum,
                                                                               u_count,
                                                                               me_ctx->tf_decay_factor_fp16[C_U]);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_sse4_1(me_ctx,
                                                                               v_pre,
                                                                               uv_pre_stride,
                                                                               (unsigned int)block_width >> ss_x,
                                                                               (unsigned int)block_height >> ss_y,
                                                                               v_accum,
                                                                               v_count,
                                                                               me_ctx->tf_decay_factor_fp16[C_V]);
    }
}

static void svt_av1_apply_temporal_filter_planewise_medium_partial_sse4_1(
    MeContext* me_ctx, const uint8_t* y_src, int y_src_stride, const uint8_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count, uint32_t tf_decay_factor,
    uint32_t luma_window_error_quad_fp8[4], int is_chroma) {
    unsigned int i, j, k, subblock_idx;

    int32_t  idx_32x32               = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
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
        tf_decay_factor <<= 1;
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

    if (block_width == 32) {
        ////(((sum<<4) /bw_half)<<4)/bh_half;
        ////(((sum<<4) /16)<<4)/16;
        ////sum;
        window_error_quad_fp8[0] = calculate_squared_errors_sum_no_div_sse4_1(
            y_src, y_src_stride, y_pre, y_pre_stride, 16, 16);
        window_error_quad_fp8[1] = calculate_squared_errors_sum_no_div_sse4_1(
            y_src + 16, y_src_stride, y_pre + 16, y_pre_stride, 16, 16);
        window_error_quad_fp8[2] = calculate_squared_errors_sum_no_div_sse4_1(
            y_src + y_src_stride * 16, y_src_stride, y_pre + y_pre_stride * 16, y_pre_stride, 16, 16);
        window_error_quad_fp8[3] = calculate_squared_errors_sum_no_div_sse4_1(
            y_src + y_src_stride * 16 + 16, y_src_stride, y_pre + y_pre_stride * 16 + 16, y_pre_stride, 16, 16);
    } else { //block_width == 16
        //(((sum<<4) /bw_half)<<4)/bh_half;
        //(((sum<<4) /8)<<4)/8;
        //sum<<2;
        calculate_squared_errors_sum_2x8xh_no_div_sse4_1(
            y_src, y_src_stride, y_pre, y_pre_stride, 8, window_error_quad_fp8);

        calculate_squared_errors_sum_2x8xh_no_div_sse4_1(y_src + y_src_stride * 8,
                                                         y_src_stride,
                                                         y_pre + y_pre_stride * 8,
                                                         y_pre_stride,
                                                         8,
                                                         &window_error_quad_fp8[2]);
        window_error_quad_fp8[0] <<= 2;
        window_error_quad_fp8[1] <<= 2;
        window_error_quad_fp8[2] <<= 2;
        window_error_quad_fp8[3] <<= 2;
    }

    if (is_chroma) {
        for (i = 0; i < 4; ++i) {
            FP_ASSERT(((int64_t)window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) < ((int64_t)1 << 31));
            window_error_quad_fp8[i] = (window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) / 6;
        }
    }

    __m128i adjusted_weight_int16[4];
    __m128i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10  = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        uint32_t scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm_set1_epi32((int32_t)(adjusted_weight));
    }

    for (i = 0; i < block_height; i++) {
        const int subblock_idx_h = (i >= block_height / 2) * 2;
        for (j = 0; j < block_width; j += 8) {
            k = i * y_pre_stride + j;

            //y_count[k] += adjusted_weight;
            __m128i count_array = _mm_loadu_si128((__m128i*)(y_count + k));
            count_array = _mm_add_epi16(count_array, adjusted_weight_int16[subblock_idx_h + (j >= block_width / 2)]);
            _mm_storeu_si128((__m128i*)(y_count + k), count_array);

            //y_accum[k] += adjusted_weight * pixel_value;
            __m128i accumulator_array1 = _mm_loadu_si128((__m128i*)(y_accum + k));
            __m128i accumulator_array2 = _mm_loadu_si128((__m128i*)(y_accum + k + 4));
            __m128i frame2_array       = _mm_loadl_epi64((__m128i*)(y_pre + k));
            frame2_array               = _mm_cvtepu8_epi16(frame2_array);
            __m128i frame2_array_u32_1 = _mm_cvtepi16_epi32(frame2_array);
            __m128i frame2_array_u32_2 = _mm_cvtepi16_epi32(_mm_srli_si128(frame2_array, 8));
            frame2_array_u32_1         = _mm_mullo_epi32(frame2_array_u32_1,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);
            frame2_array_u32_2         = _mm_mullo_epi32(frame2_array_u32_2,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array1 = _mm_add_epi32(accumulator_array1, frame2_array_u32_1);
            accumulator_array2 = _mm_add_epi32(accumulator_array2, frame2_array_u32_2);
            _mm_storeu_si128((__m128i*)(y_accum + k), accumulator_array1);
            _mm_storeu_si128((__m128i*)(y_accum + k + 4), accumulator_array2);
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_sse4_1(
    MeContext* me_ctx, const uint8_t* y_src, int y_src_stride, const uint8_t* y_pre, int y_pre_stride,
    const uint8_t* u_src, const uint8_t* v_src, int uv_src_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_partial_sse4_1(me_ctx,
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
        svt_av1_apply_temporal_filter_planewise_medium_partial_sse4_1(me_ctx,
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

        svt_av1_apply_temporal_filter_planewise_medium_partial_sse4_1(me_ctx,
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

static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_sse4_1(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor, uint32_t encoder_bit_depth) {
    unsigned int i, j, k, subblock_idx;

    int32_t idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
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

    __m128i adjusted_weight_int16[4];
    __m128i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16 = AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        int adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm_set1_epi32((int32_t)(adjusted_weight));
    }

    for (i = 0; i < block_height; i++) {
        const int subblock_idx_h = (i >= block_height / 2) * 2;
        for (j = 0; j < block_width; j += 8) {
            k = i * y_pre_stride + j;

            //y_count[k] += adjusted_weight;
            __m128i count_array = _mm_loadu_si128((__m128i*)(y_count + k));
            count_array = _mm_add_epi16(count_array, adjusted_weight_int16[subblock_idx_h + (j >= block_width / 2)]);
            _mm_storeu_si128((__m128i*)(y_count + k), count_array);

            //y_accum[k] += adjusted_weight * pixel_value;
            __m128i accumulator_array1 = _mm_loadu_si128((__m128i*)(y_accum + k));
            __m128i accumulator_array2 = _mm_loadu_si128((__m128i*)(y_accum + k + 4));
            __m128i frame2_array1      = _mm_loadl_epi64((__m128i*)(y_pre + k));
            __m128i frame2_array2      = _mm_loadl_epi64((__m128i*)(y_pre + k + 4));
            __m128i frame2_array_u32_1 = _mm_cvtepi16_epi32(frame2_array1);
            __m128i frame2_array_u32_2 = _mm_cvtepi16_epi32(frame2_array2);
            frame2_array_u32_1         = _mm_mullo_epi32(frame2_array_u32_1,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);
            frame2_array_u32_2         = _mm_mullo_epi32(frame2_array_u32_2,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array1 = _mm_add_epi32(accumulator_array1, frame2_array_u32_1);
            accumulator_array2 = _mm_add_epi32(accumulator_array2, frame2_array_u32_2);
            _mm_storeu_si128((__m128i*)(y_accum + k), accumulator_array1);
            _mm_storeu_si128((__m128i*)(y_accum + k + 4), accumulator_array2);
        }
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_sse4_1(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_sse4_1(me_ctx,
                                                                               y_pre,
                                                                               y_pre_stride,
                                                                               (unsigned int)block_width,
                                                                               (unsigned int)block_height,
                                                                               y_accum,
                                                                               y_count,
                                                                               me_ctx->tf_decay_factor_fp16[C_Y],
                                                                               encoder_bit_depth);
    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_sse4_1(me_ctx,
                                                                                   u_pre,
                                                                                   uv_pre_stride,
                                                                                   (unsigned int)block_width >> ss_x,
                                                                                   (unsigned int)block_height >> ss_y,
                                                                                   u_accum,
                                                                                   u_count,
                                                                                   me_ctx->tf_decay_factor_fp16[C_U],
                                                                                   encoder_bit_depth);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_sse4_1(me_ctx,
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

static void svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_sse4_1(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count, uint32_t tf_decay_factor,
    uint32_t luma_window_error_quad_fp8[4], int is_chroma, uint32_t encoder_bit_depth) {
    unsigned int i, j, k, subblock_idx;

    int32_t  idx_32x32               = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
    int      shift_factor            = ((encoder_bit_depth - 8) * 2);
    uint32_t distance_threshold_fp16 = AOMMAX((me_ctx->tf_mv_dist_th << 16) / 10,
                                              1 << 16); //TODO Change to FP8

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
            FP_ASSERT(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] < ((uint64_t)1 << 35));
            //block_error[i] = (double)me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] / 256;
            block_error_fp8[i] = (uint32_t)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] >> 4);
        }
    } else {
        tf_decay_factor <<= 1;
        int32_t col = me_ctx->tf_32x32_mv_x[idx_32x32];
        int32_t row = me_ctx->tf_32x32_mv_y[idx_32x32];

        uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
        //d_factor[0] = d_factor[1] = d_factor[2] = d_factor[3] = AOMMAX(distance / distance_threshold, 1);
        d_factor_fp8[0] = d_factor_fp8[1] = d_factor_fp8[2] = d_factor_fp8[3] = AOMMAX(
            (distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
        FP_ASSERT(me_ctx->tf_32x32_block_error[idx_32x32] < ((uint64_t)1 << 35));
        //block_error[0] = block_error[1] = block_error[2] = block_error[3] = (double)me_ctx->tf_32x32_block_error[idx_32x32] / 1024;
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 6);
    }

    if (block_width == 32) {
        ////(((sum<<4) /bw_half)<<4)/bh_half;
        ////(((sum<<4) /16)<<4)/16;
        ////sum;
        window_error_quad_fp8[0] = calculate_squared_errors_sum_no_div_highbd_sse4_1(
            y_src, y_src_stride, y_pre, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[1] = calculate_squared_errors_sum_no_div_highbd_sse4_1(
            y_src + 16, y_src_stride, y_pre + 16, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[2] = calculate_squared_errors_sum_no_div_highbd_sse4_1(
            y_src + y_src_stride * 16, y_src_stride, y_pre + y_pre_stride * 16, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[3] = calculate_squared_errors_sum_no_div_highbd_sse4_1(y_src + y_src_stride * 16 + 16,
                                                                                     y_src_stride,
                                                                                     y_pre + y_pre_stride * 16 + 16,
                                                                                     y_pre_stride,
                                                                                     16,
                                                                                     16,
                                                                                     shift_factor);

    } else { //block_width == 16
        //(((sum<<4) /bw_half)<<4)/bh_half;
        //(((sum<<4) /8)<<4)/8;
        //sum<<2;
        calculate_squared_errors_sum_2x8xh_no_div_highbd_sse4_1(
            y_src, y_src_stride, y_pre, y_pre_stride, 8, shift_factor, window_error_quad_fp8);

        calculate_squared_errors_sum_2x8xh_no_div_highbd_sse4_1(y_src + y_src_stride * 8,
                                                                y_src_stride,
                                                                y_pre + y_pre_stride * 8,
                                                                y_pre_stride,
                                                                8,
                                                                shift_factor,
                                                                &window_error_quad_fp8[2]);
        window_error_quad_fp8[0] <<= 2;
        window_error_quad_fp8[1] <<= 2;
        window_error_quad_fp8[2] <<= 2;
        window_error_quad_fp8[3] <<= 2;
    }

    if (is_chroma) {
        for (i = 0; i < 4; ++i) {
            FP_ASSERT(((int64_t)window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) < ((int64_t)1 << 31));
            window_error_quad_fp8[i] = (window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) / 6;
        }
    }

    __m128i adjusted_weight_int16[4];
    __m128i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10  = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        uint32_t scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        int adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm_set1_epi32((int32_t)(adjusted_weight));
    }

    for (i = 0; i < block_height; i++) {
        const int subblock_idx_h = (i >= block_height / 2) * 2;
        for (j = 0; j < block_width; j += 8) {
            k = i * y_pre_stride + j;

            //y_count[k] += adjusted_weight;
            __m128i count_array = _mm_loadu_si128((__m128i*)(y_count + k));
            count_array = _mm_add_epi16(count_array, adjusted_weight_int16[subblock_idx_h + (j >= block_width / 2)]);
            _mm_storeu_si128((__m128i*)(y_count + k), count_array);

            //y_accum[k] += adjusted_weight * pixel_value;
            __m128i accumulator_array1 = _mm_loadu_si128((__m128i*)(y_accum + k));
            __m128i accumulator_array2 = _mm_loadu_si128((__m128i*)(y_accum + k + 4));
            __m128i frame2_array1      = _mm_loadl_epi64((__m128i*)(y_pre + k));
            __m128i frame2_array2      = _mm_loadl_epi64((__m128i*)(y_pre + k + 4));
            __m128i frame2_array_u32_1 = _mm_cvtepi16_epi32(frame2_array1);
            __m128i frame2_array_u32_2 = _mm_cvtepi16_epi32(frame2_array2);
            frame2_array_u32_1         = _mm_mullo_epi32(frame2_array_u32_1,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);
            frame2_array_u32_2         = _mm_mullo_epi32(frame2_array_u32_2,
                                                 adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array1 = _mm_add_epi32(accumulator_array1, frame2_array_u32_1);
            accumulator_array2 = _mm_add_epi32(accumulator_array2, frame2_array_u32_2);
            _mm_storeu_si128((__m128i*)(y_accum + k), accumulator_array1);
            _mm_storeu_si128((__m128i*)(y_accum + k + 4), accumulator_array2);
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_hbd_sse4_1(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    const uint16_t* u_src, const uint16_t* v_src, int uv_src_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_sse4_1(me_ctx,
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
        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_sse4_1(me_ctx,
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

        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_sse4_1(me_ctx,
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

static INLINE __m128i __mm_div_epi32(const __m128i* a, const __m128i* b) {
    __m128 d_f = _mm_div_ps(_mm_cvtepi32_ps(*a), _mm_cvtepi32_ps(*b));
    return _mm_cvtps_epi32(_mm_floor_ps(d_f));
}

static void process_block_lbd_sse4_1(int h, int w, uint8_t* buff_lbd_start, uint32_t* accum, uint16_t* count,
                                     uint32_t stride) {
    int i, j, k;
    int pos = 0;
    for (i = 0, k = 0; i < h; i++) {
        for (j = 0; j < w; j += 8, k += 8) {
            //buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            //buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            __m128i accum_a = _mm_loadu_si128((__m128i*)(accum + k));
            __m128i accum_b = _mm_loadu_si128((__m128i*)(accum + k + 4));
            __m128i count_a = _mm_cvtepi16_epi32(_mm_loadl_epi64((__m128i*)(count + k)));
            __m128i count_b = _mm_cvtepi16_epi32(_mm_loadl_epi64((__m128i*)(count + k + 4)));

            //accum[k] + (count[k] >> 1)
            __m128i tmp_a = _mm_add_epi32(accum_a, _mm_srli_epi32(count_a, 1));
            __m128i tmp_b = _mm_add_epi32(accum_b, _mm_srli_epi32(count_b, 1));

            //accum[k] + (count[k] >> 1))/ count[k]
            tmp_a          = __mm_div_epi32(&tmp_a, &count_a);
            tmp_b          = __mm_div_epi32(&tmp_b, &count_b);
            __m128i tmp_ab = _mm_packus_epi16(_mm_packs_epi32(tmp_a, tmp_b), _mm_setzero_si128());

            _mm_storel_epi64((__m128i*)(buff_lbd_start + pos), tmp_ab);

            pos += 8;
        }
        pos += stride;
    }
}

static INLINE __m128i __mm_div_epi32_pd(const __m128i* a, const __m128i* b) {
    __m128d d_f1 = _mm_div_pd(_mm_cvtepi32_pd(*a), _mm_cvtepi32_pd(*b));
    __m128d d_f2 = _mm_div_pd(_mm_cvtepi32_pd(_mm_srli_si128(*a, 8)), _mm_cvtepi32_pd(_mm_srli_si128(*b, 8)));
    d_f1         = _mm_floor_pd(d_f1);
    d_f2         = _mm_floor_pd(d_f2);
    return _mm_unpacklo_epi64(_mm_cvtpd_epi32(d_f1), _mm_cvtpd_epi32(d_f2));
}

static void process_block_hbd_sse4_1(int h, int w, uint16_t* buff_hbd_start, uint32_t* accum, uint16_t* count,
                                     uint32_t stride) {
    int i, j, k;
    int pos = 0;
    for (i = 0, k = 0; i < h; i++) {
        for (j = 0; j < w; j += 8, k += 8) {
            //buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            //buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            __m128i accum_a = _mm_loadu_si128((__m128i*)(accum + k));
            __m128i accum_b = _mm_loadu_si128((__m128i*)(accum + k + 4));
            __m128i count_a = _mm_cvtepi16_epi32(_mm_loadl_epi64((__m128i*)(count + k)));
            __m128i count_b = _mm_cvtepi16_epi32(_mm_loadl_epi64((__m128i*)(count + k + 4)));

            //accum[k] + (count[k] >> 1)
            __m128i tmp_a = _mm_add_epi32(accum_a, _mm_srli_epi32(count_a, 1));
            __m128i tmp_b = _mm_add_epi32(accum_b, _mm_srli_epi32(count_b, 1));

            //accum[k] + (count[k] >> 1))/ count[k]
            tmp_a          = __mm_div_epi32_pd(&tmp_a, &count_a);
            tmp_b          = __mm_div_epi32_pd(&tmp_b, &count_b);
            __m128i tmp_ab = _mm_packs_epi32(tmp_a, tmp_b), _mm_setzero_si128();

            _mm_storeu_si128((__m128i*)(buff_hbd_start + pos), tmp_ab);

            pos += 8;
        }
        pos += stride;
    }
}

void svt_aom_get_final_filtered_pixels_sse4_1(MeContext* me_ctx, EbByte* src_center_ptr_start,
                                              uint16_t** altref_buffer_highbd_start, uint32_t** accum, uint16_t** count,
                                              const uint32_t* stride, int blk_y_src_offset, int blk_ch_src_offset,
                                              uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd) {
    assert(blk_width_ch % 16 == 0);
    assert(BW % 16 == 0);

    if (!is_highbd) {
        //Process luma
        process_block_lbd_sse4_1(
            BH, BW, &src_center_ptr_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_lbd_sse4_1(blk_height_ch,
                                     blk_width_ch,
                                     &src_center_ptr_start[C_U][blk_ch_src_offset],
                                     accum[C_U],
                                     count[C_U],
                                     stride[C_U] - blk_width_ch);
            process_block_lbd_sse4_1(blk_height_ch,
                                     blk_width_ch,
                                     &src_center_ptr_start[C_V][blk_ch_src_offset],
                                     accum[C_V],
                                     count[C_V],
                                     stride[C_V] - blk_width_ch);
        }
    } else {
        // Process luma
        process_block_hbd_sse4_1(
            BH, BW, &altref_buffer_highbd_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_hbd_sse4_1(blk_height_ch,
                                     blk_width_ch,
                                     &altref_buffer_highbd_start[C_U][blk_ch_src_offset],
                                     accum[C_U],
                                     count[C_U],
                                     stride[C_U] - blk_width_ch);
            process_block_hbd_sse4_1(blk_height_ch,
                                     blk_width_ch,
                                     &altref_buffer_highbd_start[C_V][blk_ch_src_offset],
                                     accum[C_V],
                                     count[C_V],
                                     stride[C_V] - blk_width_ch);
        }
    }
}

static void apply_filtering_central_loop_lbd(uint16_t w, uint16_t h, uint8_t* src, uint16_t src_stride, uint32_t* accum,
                                             uint16_t* count) {
    assert(w % 8 == 0);

    __m128i modifier       = _mm_set1_epi32(TF_PLANEWISE_FILTER_WEIGHT_SCALE);
    __m128i modifier_epi16 = _mm_set1_epi16(TF_PLANEWISE_FILTER_WEIGHT_SCALE);

    for (uint16_t k = 0, i = 0; i < h; i++) {
        for (uint16_t j = 0; j < w; j += 8) {
            __m128i src_16 = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(src + i * src_stride + j)));
            _mm_storeu_si128((__m128i*)(accum + k), _mm_mullo_epi32(modifier, _mm_cvtepu16_epi32(src_16)));
            _mm_storeu_si128((__m128i*)(accum + k + 4),
                             _mm_mullo_epi32(modifier, _mm_cvtepu16_epi32(_mm_srli_si128(src_16, 8))));
            _mm_storeu_si128((__m128i*)(count + k), modifier_epi16);
            k += 8;
        }
    }
}

static void apply_filtering_central_loop_hbd(uint16_t w, uint16_t h, uint16_t* src, uint16_t src_stride,
                                             uint32_t* accum, uint16_t* count) {
    assert(w % 8 == 0);

    __m128i modifier       = _mm_set1_epi32(TF_PLANEWISE_FILTER_WEIGHT_SCALE);
    __m128i modifier_epi16 = _mm_set1_epi16(TF_PLANEWISE_FILTER_WEIGHT_SCALE);

    for (uint16_t k = 0, i = 0; i < h; i++) {
        for (uint16_t j = 0; j < w; j += 8) {
            __m128i src_1 = _mm_cvtepu16_epi32(_mm_loadl_epi64((__m128i*)(src + i * src_stride + j)));
            __m128i src_2 = _mm_cvtepu16_epi32(_mm_loadl_epi64((__m128i*)(src + i * src_stride + j + 4)));
            _mm_storeu_si128((__m128i*)(accum + k), _mm_mullo_epi32(modifier, src_1));
            _mm_storeu_si128((__m128i*)(accum + k + 4), _mm_mullo_epi32(modifier, src_2));
            _mm_storeu_si128((__m128i*)(count + k), modifier_epi16);
            k += 8;
        }
    }
}

// Apply filtering to the central picture
void svt_aom_apply_filtering_central_sse4_1(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
                                            EbByte* src, uint32_t** accum, uint16_t** count, uint16_t blk_width,
                                            uint16_t blk_height, uint32_t ss_x, uint32_t ss_y) {
    uint16_t src_stride_y = input_picture_ptr_central->stride_y;

    // Luma
    apply_filtering_central_loop_lbd(blk_width, blk_height, src[C_Y], src_stride_y, accum[C_Y], count[C_Y]);

    // Chroma
    if (me_ctx->tf_chroma) {
        uint16_t blk_height_ch = blk_height >> ss_y;
        uint16_t blk_width_ch  = blk_width >> ss_x;
        uint16_t src_stride_ch = src_stride_y >> ss_x;
        apply_filtering_central_loop_lbd(blk_width_ch, blk_height_ch, src[C_U], src_stride_ch, accum[C_U], count[C_U]);
        apply_filtering_central_loop_lbd(blk_width_ch, blk_height_ch, src[C_V], src_stride_ch, accum[C_V], count[C_V]);
    }
}

// Apply filtering to the central picture
void svt_aom_apply_filtering_central_highbd_sse4_1(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
                                                   uint16_t** src_16bit, uint32_t** accum, uint16_t** count,
                                                   uint16_t blk_width, uint16_t blk_height, uint32_t ss_x,
                                                   uint32_t ss_y) {
    uint16_t src_stride_y = input_picture_ptr_central->stride_y;

    // Luma
    apply_filtering_central_loop_hbd(blk_width, blk_height, src_16bit[C_Y], src_stride_y, accum[C_Y], count[C_Y]);

    // Chroma
    if (me_ctx->tf_chroma) {
        uint16_t blk_height_ch = blk_height >> ss_y;
        uint16_t blk_width_ch  = blk_width >> ss_x;
        uint16_t src_stride_ch = src_stride_y >> ss_x;
        apply_filtering_central_loop_hbd(
            blk_width_ch, blk_height_ch, src_16bit[C_U], src_stride_ch, accum[C_U], count[C_U]);
        apply_filtering_central_loop_hbd(
            blk_width_ch, blk_height_ch, src_16bit[C_V], src_stride_ch, accum[C_V], count[C_V]);
    }
}
