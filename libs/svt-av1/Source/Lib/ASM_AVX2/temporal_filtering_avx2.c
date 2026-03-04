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
#include <immintrin.h> /* AVX2 */

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

static uint32_t calculate_squared_errors_sum_no_div_avx2(const uint8_t* s, int s_stride, const uint8_t* p, int p_stride,
                                                         unsigned int w, unsigned int h) {
    assert(w % 16 == 0 && "block width must be multiple of 16");
    unsigned int i, j;

    __m256i sum = _mm256_setzero_si256();
    __m256i s_16, p_16, dif;

    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 16) {
            s_16 = _mm256_cvtepu8_epi16(_mm_loadu_si128((const __m128i*)(s + i * s_stride + j)));
            p_16 = _mm256_cvtepu8_epi16(_mm_loadu_si128((const __m128i*)(p + i * p_stride + j)));

            dif = _mm256_sub_epi16(s_16, p_16);
            sum = _mm256_add_epi32(sum, _mm256_madd_epi16(dif, dif));
        }
    }
    __m128i sum_128 = _mm_add_epi32(_mm256_castsi256_si128(sum), _mm256_extracti128_si256(sum, 0x1));
    sum_128         = _mm_hadd_epi32(sum_128, sum_128);
    sum_128         = _mm_hadd_epi32(sum_128, sum_128);

    return _mm_cvtsi128_si32(sum_128);
}

/*This function return 2 separate squared errors for two block 8xh, return value is stored in output array*/
static void calculate_squared_errors_sum_2x8xh_no_div_avx2(const uint8_t* s, int s_stride, const uint8_t* p,
                                                           int p_stride, unsigned int h, uint32_t* output) {
    unsigned int i;

    __m256i sum = _mm256_setzero_si256();
    __m256i s_16, p_16, dif;

    for (i = 0; i < h; i++) {
        s_16 = _mm256_cvtepu8_epi16(_mm_loadu_si128((const __m128i*)(s + i * s_stride)));
        p_16 = _mm256_cvtepu8_epi16(_mm_loadu_si128((const __m128i*)(p + i * p_stride)));

        dif = _mm256_sub_epi16(s_16, p_16);
        sum = _mm256_add_epi32(sum, _mm256_madd_epi16(dif, dif));
    }
    sum = _mm256_hadd_epi32(sum, sum);
    sum = _mm256_hadd_epi32(sum, sum);

    output[0] = _mm_cvtsi128_si32(_mm256_castsi256_si128(sum));
    output[1] = _mm_cvtsi128_si32(_mm256_extracti128_si256(sum, 0x1));
}

static uint32_t calculate_squared_errors_sum_no_div_highbd_avx2(const uint16_t* s, int s_stride, const uint16_t* p,
                                                                int p_stride, unsigned int w, unsigned int h,
                                                                int shift_factor) {
    assert(w % 16 == 0 && "block width must be multiple of 16");
    unsigned int i, j;

    __m256i sum = _mm256_setzero_si256();
    __m256i s_16, p_16, dif;

    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 16) {
            s_16 = _mm256_loadu_si256((const __m256i*)(s + i * s_stride + j));
            p_16 = _mm256_loadu_si256((const __m256i*)(p + i * p_stride + j));

            dif = _mm256_sub_epi16(s_16, p_16);
            sum = _mm256_add_epi32(sum, _mm256_madd_epi16(dif, dif));
        }
    }
    __m128i sum_128 = _mm_add_epi32(_mm256_castsi256_si128(sum), _mm256_extracti128_si256(sum, 0x1));
    sum_128         = _mm_hadd_epi32(sum_128, sum_128);
    sum_128         = _mm_hadd_epi32(sum_128, sum_128);

    return _mm_cvtsi128_si32(sum_128) >> shift_factor;
}

/*This function return 2 separate squared errors for two block 8xh, return value is stored in output array*/
static void calculate_squared_errors_sum_2x8xh_no_div_highbd_avx2(const uint16_t* s, int s_stride, const uint16_t* p,
                                                                  int p_stride, unsigned int h, int shift_factor,
                                                                  uint32_t* output) {
    unsigned int i;

    __m256i sum = _mm256_setzero_si256();
    __m256i s_16, p_16, dif;

    for (i = 0; i < h; i++) {
        s_16 = _mm256_loadu_si256((const __m256i*)(s + i * s_stride));
        p_16 = _mm256_loadu_si256((const __m256i*)(p + i * p_stride));

        dif = _mm256_sub_epi16(s_16, p_16);
        sum = _mm256_add_epi32(sum, _mm256_madd_epi16(dif, dif));
    }
    sum = _mm256_hadd_epi32(sum, sum);
    sum = _mm256_hadd_epi32(sum, sum);

    output[0] = _mm_cvtsi128_si32(_mm256_castsi256_si128(sum)) >> shift_factor;
    output[1] = _mm_cvtsi128_si32(_mm256_extracti128_si256(sum, 0x1)) >> shift_factor;
}

static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_avx2(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor) {
    unsigned int i, j, k, subblock_idx;

    int32_t  idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
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
    __m256i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16 = AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm256_set1_epi32((int32_t)(adjusted_weight));
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
            __m256i accumulator_array = _mm256_loadu_si256((__m256i*)(y_accum + k));
            __m128i frame2_array      = _mm_loadl_epi64((__m128i*)(y_pre + k));
            frame2_array              = _mm_cvtepu8_epi16(frame2_array);
            __m256i frame2_array_u32  = _mm256_cvtepi16_epi32(frame2_array);
            frame2_array_u32          = _mm256_mullo_epi32(frame2_array_u32,
                                                  adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array = _mm256_add_epi32(accumulator_array, frame2_array_u32);
            _mm256_storeu_si256((__m256i*)(y_accum + k), accumulator_array);
        }
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_avx2(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_avx2(me_ctx,
                                                                         y_pre,
                                                                         y_pre_stride,
                                                                         (unsigned int)block_width,
                                                                         (unsigned int)block_height,
                                                                         y_accum,
                                                                         y_count,
                                                                         me_ctx->tf_decay_factor_fp16[C_Y]);

    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_avx2(me_ctx,
                                                                             u_pre,
                                                                             uv_pre_stride,
                                                                             (unsigned int)block_width >> ss_x,
                                                                             (unsigned int)block_height >> ss_y,
                                                                             u_accum,
                                                                             u_count,
                                                                             me_ctx->tf_decay_factor_fp16[C_U]);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_avx2(me_ctx,
                                                                             v_pre,
                                                                             uv_pre_stride,
                                                                             (unsigned int)block_width >> ss_x,
                                                                             (unsigned int)block_height >> ss_y,
                                                                             v_accum,
                                                                             v_count,
                                                                             me_ctx->tf_decay_factor_fp16[C_V]);
    }
}

static void svt_av1_apply_temporal_filter_planewise_medium_partial_avx2(
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
        window_error_quad_fp8[0] = calculate_squared_errors_sum_no_div_avx2(
            y_src, y_src_stride, y_pre, y_pre_stride, 16, 16);
        window_error_quad_fp8[1] = calculate_squared_errors_sum_no_div_avx2(
            y_src + 16, y_src_stride, y_pre + 16, y_pre_stride, 16, 16);
        window_error_quad_fp8[2] = calculate_squared_errors_sum_no_div_avx2(
            y_src + y_src_stride * 16, y_src_stride, y_pre + y_pre_stride * 16, y_pre_stride, 16, 16);
        window_error_quad_fp8[3] = calculate_squared_errors_sum_no_div_avx2(
            y_src + y_src_stride * 16 + 16, y_src_stride, y_pre + y_pre_stride * 16 + 16, y_pre_stride, 16, 16);
    } else { //block_width == 16
        //(((sum<<4) /bw_half)<<4)/bh_half;
        //(((sum<<4) /8)<<4)/8;
        //sum<<2;
        calculate_squared_errors_sum_2x8xh_no_div_avx2(
            y_src, y_src_stride, y_pre, y_pre_stride, 8, window_error_quad_fp8);

        calculate_squared_errors_sum_2x8xh_no_div_avx2(y_src + y_src_stride * 8,
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
    __m256i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10  = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        uint32_t scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm256_set1_epi32((int32_t)(adjusted_weight));
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
            __m256i accumulator_array = _mm256_loadu_si256((__m256i*)(y_accum + k));
            __m128i frame2_array      = _mm_loadl_epi64((__m128i*)(y_pre + k));
            frame2_array              = _mm_cvtepu8_epi16(frame2_array);
            __m256i frame2_array_u32  = _mm256_cvtepi16_epi32(frame2_array);
            frame2_array_u32          = _mm256_mullo_epi32(frame2_array_u32,
                                                  adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array = _mm256_add_epi32(accumulator_array, frame2_array_u32);
            _mm256_storeu_si256((__m256i*)(y_accum + k), accumulator_array);
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_avx2(
    MeContext* me_ctx, const uint8_t* y_src, int y_src_stride, const uint8_t* y_pre, int y_pre_stride,
    const uint8_t* u_src, const uint8_t* v_src, int uv_src_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_partial_avx2(me_ctx,
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
        svt_av1_apply_temporal_filter_planewise_medium_partial_avx2(me_ctx,
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

        svt_av1_apply_temporal_filter_planewise_medium_partial_avx2(me_ctx,
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

static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_avx2(
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
    __m256i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16 = AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        int adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm256_set1_epi32((int32_t)(adjusted_weight));
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
            __m256i accumulator_array = _mm256_loadu_si256((__m256i*)(y_accum + k));
            __m128i frame2_array      = _mm_loadu_si128((__m128i*)(y_pre + k));
            __m256i frame2_array_u32  = _mm256_cvtepi16_epi32(frame2_array);
            frame2_array_u32          = _mm256_mullo_epi32(frame2_array_u32,
                                                  adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array = _mm256_add_epi32(accumulator_array, frame2_array_u32);
            _mm256_storeu_si256((__m256i*)(y_accum + k), accumulator_array);
        }
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_avx2(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_avx2(me_ctx,
                                                                             y_pre,
                                                                             y_pre_stride,
                                                                             (unsigned int)block_width,
                                                                             (unsigned int)block_height,
                                                                             y_accum,
                                                                             y_count,
                                                                             me_ctx->tf_decay_factor_fp16[C_Y],
                                                                             encoder_bit_depth);
    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_avx2(me_ctx,
                                                                                 u_pre,
                                                                                 uv_pre_stride,
                                                                                 (unsigned int)block_width >> ss_x,
                                                                                 (unsigned int)block_height >> ss_y,
                                                                                 u_accum,
                                                                                 u_count,
                                                                                 me_ctx->tf_decay_factor_fp16[C_U],
                                                                                 encoder_bit_depth);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_partial_avx2(me_ctx,
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

static void svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_avx2(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count, uint32_t tf_decay_factor,
    uint32_t luma_window_error_quad_fp8[4], int is_chroma, uint32_t encoder_bit_depth) {
    unsigned int i, j, k, subblock_idx;

    int32_t  idx_32x32               = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
    int      shift_factor            = ((encoder_bit_depth - 8) * 2);
    uint32_t distance_threshold_fp16 = AOMMAX((me_ctx->tf_mv_dist_th << 16) / 10, 1 << 16); //TODO Change to FP8
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
        window_error_quad_fp8[0] = calculate_squared_errors_sum_no_div_highbd_avx2(
            y_src, y_src_stride, y_pre, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[1] = calculate_squared_errors_sum_no_div_highbd_avx2(
            y_src + 16, y_src_stride, y_pre + 16, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[2] = calculate_squared_errors_sum_no_div_highbd_avx2(
            y_src + y_src_stride * 16, y_src_stride, y_pre + y_pre_stride * 16, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[3] = calculate_squared_errors_sum_no_div_highbd_avx2(y_src + y_src_stride * 16 + 16,
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
        calculate_squared_errors_sum_2x8xh_no_div_highbd_avx2(
            y_src, y_src_stride, y_pre, y_pre_stride, 8, shift_factor, window_error_quad_fp8);

        calculate_squared_errors_sum_2x8xh_no_div_highbd_avx2(y_src + y_src_stride * 8,
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
    __m256i adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10  = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        uint32_t scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        int adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        adjusted_weight_int16[subblock_idx] = _mm_set1_epi16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = _mm256_set1_epi32((int32_t)(adjusted_weight));
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
            __m256i accumulator_array = _mm256_loadu_si256((__m256i*)(y_accum + k));
            __m128i frame2_array      = _mm_loadu_si128((__m128i*)(y_pre + k));
            __m256i frame2_array_u32  = _mm256_cvtepi16_epi32(frame2_array);
            frame2_array_u32          = _mm256_mullo_epi32(frame2_array_u32,
                                                  adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array = _mm256_add_epi32(accumulator_array, frame2_array_u32);
            _mm256_storeu_si256((__m256i*)(y_accum + k), accumulator_array);
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_hbd_avx2(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    const uint16_t* u_src, const uint16_t* v_src, int uv_src_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_avx2(me_ctx,
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
        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_avx2(me_ctx,
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

        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_avx2(me_ctx,
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

static INLINE __m256i __m256i_div_epi32(const __m256i* a, const __m256i* b) {
    __m256 d_f = _mm256_div_ps(_mm256_cvtepi32_ps(*a), _mm256_cvtepi32_ps(*b));
    return _mm256_cvtps_epi32(_mm256_floor_ps(d_f));
}

static void process_block_lbd_avx2(int h, int w, uint8_t* buff_lbd_start, uint32_t* accum, uint16_t* count,
                                   uint32_t stride) {
    int i, j, k;
    int pos = 0;
    for (i = 0, k = 0; i < h; i++) {
        for (j = 0; j < w; j += 16, k += 16) {
            //buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            //buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            __m256i accum_a = _mm256_loadu_si256((__m256i*)(accum + k));
            __m256i accum_b = _mm256_loadu_si256((__m256i*)(accum + k + 8));
            __m256i count_a = _mm256_cvtepi16_epi32(_mm_loadu_si128((__m128i*)(count + k)));
            __m256i count_b = _mm256_cvtepi16_epi32(_mm_loadu_si128((__m128i*)(count + k + 8)));

            //accum[k] + (count[k] >> 1)
            __m256i tmp_a = _mm256_add_epi32(accum_a, _mm256_srli_epi32(count_a, 1));
            __m256i tmp_b = _mm256_add_epi32(accum_b, _mm256_srli_epi32(count_b, 1));

            //accum[k] + (count[k] >> 1))/ count[k]
            tmp_a          = __m256i_div_epi32(&tmp_a, &count_a);
            tmp_b          = __m256i_div_epi32(&tmp_b, &count_b);
            __m256i tmp_ab = _mm256_packs_epi32(tmp_a, tmp_b);

            tmp_ab = _mm256_permutevar8x32_epi32(tmp_ab, _mm256_setr_epi32(0, 1, 4, 5, 2, 3, 6, 7));

            __m128i tmp_ab_128 = _mm_packus_epi16(_mm256_castsi256_si128(tmp_ab),
                                                  _mm256_extracti128_si256(tmp_ab, 0x1));

            _mm_storeu_si128((__m128i*)(buff_lbd_start + pos), tmp_ab_128);

            pos += 16;
        }
        pos += stride;
    }
}

static INLINE __m256i __m256i_div_epi32_pd(const __m256i* a, const __m256i* b) {
    __m256d d_f1 = _mm256_div_pd(_mm256_cvtepi32_pd(_mm256_castsi256_si128(*a)),
                                 _mm256_cvtepi32_pd(_mm256_castsi256_si128(*b)));
    __m256d d_f2 = _mm256_div_pd(_mm256_cvtepi32_pd(_mm256_extracti128_si256(*a, 0x1)),
                                 _mm256_cvtepi32_pd(_mm256_extracti128_si256(*b, 0x1)));
    __m128i i_f1 = _mm256_cvtpd_epi32(_mm256_floor_pd(d_f1));
    __m128i i_f2 = _mm256_cvtpd_epi32(_mm256_floor_pd(d_f2));

    return _mm256_insertf128_si256(_mm256_castsi128_si256(i_f1), i_f2, 0x1);
}

static void process_block_hbd_avx2(int h, int w, uint16_t* buff_hbd_start, uint32_t* accum, uint16_t* count,
                                   uint32_t stride) {
    int i, j, k;
    int pos = 0;
    for (i = 0, k = 0; i < h; i++) {
        for (j = 0; j < w; j += 16, k += 16) {
            //buff_hbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            //buff_hbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            __m256i accum_a = _mm256_loadu_si256((__m256i*)(accum + k));
            __m256i accum_b = _mm256_loadu_si256((__m256i*)(accum + k + 8));
            __m256i count_a = _mm256_cvtepi16_epi32(_mm_loadu_si128((__m128i*)(count + k)));
            __m256i count_b = _mm256_cvtepi16_epi32(_mm_loadu_si128((__m128i*)(count + k + 8)));

            //accum[k] + (count[k] >> 1)
            __m256i tmp_a = _mm256_add_epi32(accum_a, _mm256_srli_epi32(count_a, 1));
            __m256i tmp_b = _mm256_add_epi32(accum_b, _mm256_srli_epi32(count_b, 1));

            //accum[k] + (count[k] >> 1))/ count[k]
            tmp_a          = __m256i_div_epi32_pd(&tmp_a, &count_a);
            tmp_b          = __m256i_div_epi32_pd(&tmp_b, &count_b);
            __m256i tmp_ab = _mm256_packs_epi32(tmp_a, tmp_b);

            tmp_ab = _mm256_permutevar8x32_epi32(tmp_ab, _mm256_setr_epi32(0, 1, 4, 5, 2, 3, 6, 7));

            _mm256_storeu_si256((__m256i*)(buff_hbd_start + pos), tmp_ab);

            pos += 16;
        }
        pos += stride;
    }
}

void svt_aom_get_final_filtered_pixels_avx2(MeContext* me_ctx, EbByte* src_center_ptr_start,
                                            uint16_t** altref_buffer_highbd_start, uint32_t** accum, uint16_t** count,
                                            const uint32_t* stride, int blk_y_src_offset, int blk_ch_src_offset,
                                            uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd) {
    assert(blk_width_ch % 16 == 0);
    assert(BW % 16 == 0);

    if (!is_highbd) {
        //Process luma
        process_block_lbd_avx2(
            BH, BW, &src_center_ptr_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_lbd_avx2(blk_height_ch,
                                   blk_width_ch,
                                   &src_center_ptr_start[C_U][blk_ch_src_offset],
                                   accum[C_U],
                                   count[C_U],
                                   stride[C_U] - blk_width_ch);
            process_block_lbd_avx2(blk_height_ch,
                                   blk_width_ch,
                                   &src_center_ptr_start[C_V][blk_ch_src_offset],
                                   accum[C_V],
                                   count[C_V],
                                   stride[C_V] - blk_width_ch);
        }
    } else {
        // Process luma
        process_block_hbd_avx2(
            BH, BW, &altref_buffer_highbd_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_hbd_avx2(blk_height_ch,
                                   blk_width_ch,
                                   &altref_buffer_highbd_start[C_U][blk_ch_src_offset],
                                   accum[C_U],
                                   count[C_U],
                                   stride[C_U] - blk_width_ch);
            process_block_hbd_avx2(blk_height_ch,
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

    __m256i modifier       = _mm256_set1_epi32(TF_PLANEWISE_FILTER_WEIGHT_SCALE);
    __m128i modifier_epi16 = _mm_set1_epi16(TF_PLANEWISE_FILTER_WEIGHT_SCALE);

    for (uint16_t k = 0, i = 0; i < h; i++) {
        for (uint16_t j = 0; j < w; j += 8) {
            __m256i src_ = _mm256_cvtepu16_epi32(
                _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(src + i * src_stride + j))));
            _mm256_storeu_si256((__m256i*)(accum + k), _mm256_mullo_epi32(modifier, src_));
            _mm_storeu_si128((__m128i*)(count + k), modifier_epi16);
            k += 8;
        }
    }
}

static void apply_filtering_central_loop_hbd(uint16_t w, uint16_t h, uint16_t* src, uint16_t src_stride,
                                             uint32_t* accum, uint16_t* count) {
    assert(w % 8 == 0);

    __m256i modifier       = _mm256_set1_epi32(TF_PLANEWISE_FILTER_WEIGHT_SCALE);
    __m128i modifier_epi16 = _mm_set1_epi16(TF_PLANEWISE_FILTER_WEIGHT_SCALE);

    for (uint16_t k = 0, i = 0; i < h; i++) {
        for (uint16_t j = 0; j < w; j += 8) {
            __m256i src_ = _mm256_cvtepu16_epi32(_mm_loadu_si128((__m128i*)(src + i * src_stride + j)));
            _mm256_storeu_si256((__m256i*)(accum + k), _mm256_mullo_epi32(modifier, src_));
            _mm_storeu_si128((__m128i*)(count + k), modifier_epi16);
            k += 8;
        }
    }
}

// Apply filtering to the central picture
void svt_aom_apply_filtering_central_avx2(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
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
void svt_aom_apply_filtering_central_highbd_avx2(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
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

int32_t svt_estimate_noise_fp16_avx2(const uint8_t* src, uint16_t width, uint16_t height, uint16_t stride_y) {
    int64_t sum = 0;
    int64_t num = 0;

    //  A | B | C
    //  D | E | F
    //  G | H | I
    // g_x = (A - I) + (G - C) + 2*(D - F)
    // g_y = (A - I) - (G - C) + 2*(B - H)
    // v   = 4*E - 2*(D+F+B+H) + (A+C+G+I)

    const __m256i zero            = _mm256_setzero_si256();
    const __m256i edge_threshold  = _mm256_set1_epi16(EDGE_THRESHOLD);
    __m256i       num_accumulator = _mm256_setzero_si256();
    __m256i       sum_accumulator = _mm256_setzero_si256();

    for (int i = 1; i < height - 1; ++i) {
        int j = 1;
        for (; j + 16 < width - 1; j += 16) {
            const int k = i * stride_y + j;

            __m256i A = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k - stride_y - 1])));
            __m256i B = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k - stride_y])));
            __m256i C = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k - stride_y + 1])));
            __m256i D = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k - 1])));
            __m256i E = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k])));
            __m256i F = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k + 1])));
            __m256i G = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k + stride_y - 1])));
            __m256i H = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k + stride_y])));
            __m256i I = _mm256_cvtepu8_epi16(_mm_loadu_si128((__m128i*)(&src[k + stride_y + 1])));

            __m256i A_m_I   = _mm256_sub_epi16(A, I);
            __m256i G_m_C   = _mm256_sub_epi16(G, C);
            __m256i D_m_Fx2 = _mm256_slli_epi16(_mm256_sub_epi16(D, F), 1);
            __m256i B_m_Hx2 = _mm256_slli_epi16(_mm256_sub_epi16(B, H), 1);

            __m256i gx_avx = _mm256_abs_epi16(_mm256_add_epi16(_mm256_add_epi16(A_m_I, G_m_C), D_m_Fx2));
            __m256i gy_avx = _mm256_abs_epi16(_mm256_add_epi16(_mm256_sub_epi16(A_m_I, G_m_C), B_m_Hx2));
            __m256i ga_avx = _mm256_add_epi16(gx_avx, gy_avx);

            __m256i D_F_B_Hx2 = _mm256_slli_epi16(_mm256_add_epi16(_mm256_add_epi16(D, F), _mm256_add_epi16(B, H)), 1);
            __m256i A_C_G_I   = _mm256_add_epi16(_mm256_add_epi16(A, C), _mm256_add_epi16(G, I));
            __m256i v_avx2    = _mm256_abs_epi16(
                _mm256_add_epi16(_mm256_sub_epi16(_mm256_slli_epi16(E, 2), D_F_B_Hx2), A_C_G_I));

            //if (ga < EDGE_THRESHOLD)
            __m256i cmp = _mm256_srli_epi16(_mm256_cmpgt_epi16(edge_threshold, ga_avx), 15);
            v_avx2      = _mm256_mullo_epi16(v_avx2, cmp);

            //num_accumulator and sum_accumulator have 32bit values
            num_accumulator = _mm256_add_epi32(num_accumulator, _mm256_unpacklo_epi16(cmp, zero));
            num_accumulator = _mm256_add_epi32(num_accumulator, _mm256_unpackhi_epi16(cmp, zero));
            sum_accumulator = _mm256_add_epi32(sum_accumulator, _mm256_unpacklo_epi16(v_avx2, zero));
            sum_accumulator = _mm256_add_epi32(sum_accumulator, _mm256_unpackhi_epi16(v_avx2, zero));
        }
        for (; j < width - 1; ++j) {
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

    __m256i sum_avx = _mm256_hadd_epi32(sum_accumulator, num_accumulator);
    sum_avx         = _mm256_hadd_epi32(sum_avx, sum_avx);
    __m128i sum_sse = _mm256_castsi256_si128(sum_avx);
    sum_sse         = _mm_add_epi32(sum_sse, _mm256_extractf128_si256(sum_avx, 1));
    sum += _mm_cvtsi128_si32(sum_sse);
    num += _mm_cvtsi128_si32(_mm_srli_si128(sum_sse, 4));

    // If very few smooth pels, return -1 since the estimate is unreliable
    if (num < SMOOTH_THRESHOLD) {
        return -65536 /*-1:fp16*/;
    }

    FP_ASSERT((((int64_t)sum * SQRT_PI_BY_2_FP16) / (6 * num)) < ((int64_t)1 << 31));
    return (int32_t)((sum * SQRT_PI_BY_2_FP16) / (6 * num));
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int32_t svt_estimate_noise_highbd_fp16_avx2(const uint16_t* src, int width, int height, int stride, int bd) {
    int64_t sum = 0;
    int64_t num = 0;

    //  A | B | C
    //  D | E | F
    //  G | H | I
    // g_x = (A - I) + (G - C) + 2*(D - F)
    // g_y = (A - I) - (G - C) + 2*(B - H)
    // v   = 4*E - 2*(D+F+B+H) + (A+C+G+I)

    const __m256i zero            = _mm256_setzero_si256();
    const __m256i edge_threshold  = _mm256_set1_epi16(EDGE_THRESHOLD);
    __m256i       num_accumulator = _mm256_setzero_si256();
    __m256i       sum_accumulator = _mm256_setzero_si256();
    const __m256i rounding        = _mm256_set1_epi16(1 << ((bd - 8) - 1));

    for (int i = 1; i < height - 1; ++i) {
        int j = 1;
        for (; j + 16 < width - 1; j += 16) {
            const int k = i * stride + j;

            __m256i A = _mm256_loadu_si256((__m256i*)(&src[k - stride - 1]));
            __m256i B = _mm256_loadu_si256((__m256i*)(&src[k - stride]));
            __m256i C = _mm256_loadu_si256((__m256i*)(&src[k - stride + 1]));
            __m256i D = _mm256_loadu_si256((__m256i*)(&src[k - 1]));
            __m256i E = _mm256_loadu_si256((__m256i*)(&src[k]));
            __m256i F = _mm256_loadu_si256((__m256i*)(&src[k + 1]));
            __m256i G = _mm256_loadu_si256((__m256i*)(&src[k + stride - 1]));
            __m256i H = _mm256_loadu_si256((__m256i*)(&src[k + stride]));
            __m256i I = _mm256_loadu_si256((__m256i*)(&src[k + stride + 1]));

            __m256i A_m_I   = _mm256_sub_epi16(A, I);
            __m256i G_m_C   = _mm256_sub_epi16(G, C);
            __m256i D_m_Fx2 = _mm256_slli_epi16(_mm256_sub_epi16(D, F), 1);
            __m256i B_m_Hx2 = _mm256_slli_epi16(_mm256_sub_epi16(B, H), 1);

            __m256i gx_avx = _mm256_abs_epi16(_mm256_add_epi16(_mm256_add_epi16(A_m_I, G_m_C), D_m_Fx2));
            __m256i gy_avx = _mm256_abs_epi16(_mm256_add_epi16(_mm256_sub_epi16(A_m_I, G_m_C), B_m_Hx2));
            __m256i ga_avx = _mm256_srai_epi16(_mm256_add_epi16(_mm256_add_epi16(gx_avx, gy_avx), rounding), (bd - 8));

            __m256i D_F_B_Hx2 = _mm256_slli_epi16(_mm256_add_epi16(_mm256_add_epi16(D, F), _mm256_add_epi16(B, H)), 1);
            __m256i A_C_G_I   = _mm256_add_epi16(_mm256_add_epi16(A, C), _mm256_add_epi16(G, I));
            __m256i v_avx2    = _mm256_abs_epi16(
                _mm256_add_epi16(_mm256_sub_epi16(_mm256_slli_epi16(E, 2), D_F_B_Hx2), A_C_G_I));

            //if (ga < EDGE_THRESHOLD)
            __m256i cmp = _mm256_srli_epi16(_mm256_cmpgt_epi16(edge_threshold, ga_avx), 15);
            v_avx2      = _mm256_srai_epi16(_mm256_add_epi16(_mm256_mullo_epi16(v_avx2, cmp), rounding), (bd - 8));

            //num_accumulator and sum_accumulator have 32bit values
            num_accumulator = _mm256_add_epi32(num_accumulator, _mm256_unpacklo_epi16(cmp, zero));
            num_accumulator = _mm256_add_epi32(num_accumulator, _mm256_unpackhi_epi16(cmp, zero));
            sum_accumulator = _mm256_add_epi32(sum_accumulator, _mm256_unpacklo_epi16(v_avx2, zero));
            sum_accumulator = _mm256_add_epi32(sum_accumulator, _mm256_unpackhi_epi16(v_avx2, zero));
        }
        for (; j < width - 1; ++j) {
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

    __m256i sum_avx = _mm256_hadd_epi32(sum_accumulator, num_accumulator);
    sum_avx         = _mm256_hadd_epi32(sum_avx, sum_avx);
    __m128i sum_sse = _mm256_castsi256_si128(sum_avx);
    sum_sse         = _mm_add_epi32(sum_sse, _mm256_extractf128_si256(sum_avx, 1));
    sum += _mm_cvtsi128_si32(sum_sse);
    num += _mm_cvtsi128_si32(_mm_srli_si128(sum_sse, 4));

    // If very few smooth pels, return -1 since the estimate is unreliable
    if (num < SMOOTH_THRESHOLD) {
        return -65536 /*-1:fp16*/;
    }

    FP_ASSERT((((int64_t)sum * SQRT_PI_BY_2_FP16) / (6 * num)) < ((int64_t)1 << 31));
    return (int32_t)((sum * SQRT_PI_BY_2_FP16) / (6 * num));
}
#endif
