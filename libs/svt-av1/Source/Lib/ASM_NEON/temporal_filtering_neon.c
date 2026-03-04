/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <assert.h>
#include <arm_neon.h>

#include "definitions.h"
#include "mem_neon.h"
#include "temporal_filtering.h"
#include "utility.h"

/* value [i:0-15] (sqrt((float)i)*65536.0 */
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

/* Calc sqrt linear max error 10% */
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

#define SSE_STRIDE (BW + 2)

static uint32_t calculate_squared_errors_sum_no_div_16x16_neon(const uint8_t* s, int s_stride, const uint8_t* p,
                                                               int p_stride) {
    int32x4_t sum_hi = vdupq_n_s32(0);
    int32x4_t sum_lo = vdupq_n_s32(0);

    const int32x2_t stride = vzip1_s32(vdup_n_s32(s_stride), vdup_n_s32(p_stride));

    for (unsigned int i = 0; i < 16; i++) {
        const int32x2_t offset = vmul_s32(stride, vdup_n_s32(i));

        const uint8x16_t s_8 = vld1q_u8(s + vget_lane_s32(offset, 0));
        const uint8x16_t p_8 = vld1q_u8(p + vget_lane_s32(offset, 1));

        const int16x8_t s_8_hi = vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(s_8)));
        const int16x8_t p_8_hi = vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(p_8)));
        const int16x8_t s_8_lo = vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(s_8)));
        const int16x8_t p_8_lo = vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(p_8)));

        const int16x8_t dif_hi = vsubq_s16(s_8_hi, p_8_hi);
        const int16x8_t dif_lo = vsubq_s16(s_8_lo, p_8_lo);

        const int32x4_t pl_hi = vmull_s16(vget_low_s16(dif_hi), vget_low_s16(dif_hi));
        const int32x4_t ph_hi = vmull_high_s16(dif_hi, dif_hi);
        const int32x4_t pl_lo = vmull_s16(vget_low_s16(dif_lo), vget_low_s16(dif_lo));
        const int32x4_t ph_lo = vmull_high_s16(dif_lo, dif_lo);

        sum_hi = vaddq_s32(sum_hi, vpaddq_s32(pl_hi, ph_hi));
        sum_lo = vaddq_s32(sum_lo, vpaddq_s32(pl_lo, ph_lo));
    }

    int32x4_t sum = vpaddq_s32(sum_hi, sum_lo);

    sum = vpaddq_s32(sum, sum);
    sum = vpaddq_s32(sum, sum);

    return vgetq_lane_s32(sum, 0);
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

static void calculate_squared_errors_sum_2x8x8_no_div_neon(const uint8_t* s, int s_stride, const uint8_t* p,
                                                           int p_stride, uint32_t* output) {
    int32x4_t sum_lo = vdupq_n_s32(0);
    int32x4_t sum_hi = vdupq_n_s32(0);

    const int32x2_t stride = vzip1_s32(vdup_n_s32(s_stride), vdup_n_s32(p_stride));

    for (unsigned int i = 0; i < 8; i++) {
        const int32x2_t offset = vmul_s32(stride, vdup_n_s32(i));

        const uint8x16_t s_8 = vld1q_u8(s + vget_lane_s32(offset, 0));
        const uint8x16_t p_8 = vld1q_u8(p + vget_lane_s32(offset, 1));

        const int16x8_t s_8_lo = vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(s_8)));
        const int16x8_t p_8_lo = vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(p_8)));

        const int16x8_t s_8_hi = vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(s_8)));
        const int16x8_t p_8_hi = vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(p_8)));

        const int16x8_t dif_lo = vsubq_s16(s_8_lo, p_8_lo);
        const int16x8_t dif_hi = vsubq_s16(s_8_hi, p_8_hi);

        const int32x4_t pl_lo = vmull_s16(vget_low_s16(dif_lo), vget_low_s16(dif_lo));
        const int32x4_t ph_lo = vmull_high_s16(dif_lo, dif_lo);
        sum_lo                = vaddq_s32(sum_lo, vpaddq_s32(pl_lo, ph_lo));

        const int32x4_t pl_hi = vmull_s16(vget_low_s16(dif_hi), vget_low_s16(dif_hi));
        const int32x4_t ph_hi = vmull_high_s16(dif_hi, dif_hi);
        sum_hi                = vaddq_s32(sum_hi, vpaddq_s32(pl_hi, ph_hi));
    }
    sum_lo = vpaddq_s32(sum_lo, sum_lo);
    sum_lo = vpaddq_s32(sum_lo, sum_lo);

    sum_hi = vpaddq_s32(sum_hi, sum_hi);
    sum_hi = vpaddq_s32(sum_hi, sum_hi);

    output[0] = vgetq_lane_s32(sum_lo, 0);
    output[1] = vgetq_lane_s32(sum_hi, 0);
}

static void svt_av1_apply_temporal_filter_planewise_medium_partial_neon(
    MeContext* me_ctx, const uint8_t* y_src, int y_src_stride, const uint8_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count, uint32_t tf_decay_factor,
    uint32_t luma_window_error_quad_fp8[4], int is_chroma) {
    unsigned int i, j, k, subblock_idx;

    int32_t  idx_32x32               = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
    uint32_t distance_threshold_fp16 = AOMMAX((me_ctx->tf_mv_dist_th << 16) / 10, 1 << 16);
    // Calculation for every quarter
    uint32_t  d_factor_fp8[4];
    uint32_t  block_error_fp8[4];
    uint32_t  chroma_window_error_quad_fp8[4];
    uint32_t* window_error_quad_fp8 = is_chroma ? chroma_window_error_quad_fp8 : luma_window_error_quad_fp8;

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        const int32x4_t col = vmovl_s16(vld1_s16((int16_t*)&me_ctx->tf_16x16_mv_x[idx_32x32 * 4]));
        const int32x4_t row = vmovl_s16(vld1_s16((int16_t*)&me_ctx->tf_16x16_mv_y[idx_32x32 * 4]));

        const uint32x4_t hyp     = vreinterpretq_u32_s32(vaddq_s32(vmulq_s32(col, col), vmulq_s32(row, row)));
        const uint32x4_t hyp_256 = vshlq_n_u32(hyp, 8);

        uint32x4_t distance_fp4 = vcombine_u32(vzip1_u32(vdup_n_u32(sqrt_fast(vgetq_lane_u32(hyp_256, 0))),
                                                         vdup_n_u32(sqrt_fast(vgetq_lane_u32(hyp_256, 1)))),
                                               vzip1_u32(vdup_n_u32(sqrt_fast(vgetq_lane_u32(hyp_256, 2))),
                                                         vdup_n_u32(sqrt_fast(vgetq_lane_u32(hyp_256, 3)))));

        uint32x4_t d_factor_fp8_v = vcvtq_u32_f32(
            vdivq_f32(vcvtq_f32_u32(vshlq_n_u32(distance_fp4, 12)),
                      vcvtq_f32_u32(vshrq_n_u32(vdupq_n_u32(distance_threshold_fp16), 8))));
        d_factor_fp8_v = vmaxq_u32(d_factor_fp8_v, vdupq_n_u32(1 << 8));
        vst1q_u32(d_factor_fp8, d_factor_fp8_v);

        // ignore odd elements, since those are the higher 32 bits of every 64 bit entry
        uint32x4x2_t aux = vld2q_u32((uint32_t*)&me_ctx->tf_16x16_block_error[idx_32x32 * 4 + 0]);
        vst1q_u32(block_error_fp8, aux.val[0]);

    } else {
        tf_decay_factor <<= 1;
        int32_t col = me_ctx->tf_32x32_mv_x[idx_32x32];
        int32_t row = me_ctx->tf_32x32_mv_y[idx_32x32];

        uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
        d_factor_fp8[0] = d_factor_fp8[1] = d_factor_fp8[2] = d_factor_fp8[3] = AOMMAX(
            (distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
        FP_ASSERT(me_ctx->tf_32x32_block_error[idx_32x32] < ((uint64_t)1 << 30));
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 2);
    }

    if (block_width == 32) {
        window_error_quad_fp8[0] = calculate_squared_errors_sum_no_div_16x16_neon(
            y_src, y_src_stride, y_pre, y_pre_stride);
        window_error_quad_fp8[1] = calculate_squared_errors_sum_no_div_16x16_neon(
            y_src + 16, y_src_stride, y_pre + 16, y_pre_stride);
        window_error_quad_fp8[2] = calculate_squared_errors_sum_no_div_16x16_neon(
            y_src + y_src_stride * 16, y_src_stride, y_pre + y_pre_stride * 16, y_pre_stride);
        window_error_quad_fp8[3] = calculate_squared_errors_sum_no_div_16x16_neon(
            y_src + y_src_stride * 16 + 16, y_src_stride, y_pre + y_pre_stride * 16 + 16, y_pre_stride);
    } else {
        calculate_squared_errors_sum_2x8x8_no_div_neon(y_src, y_src_stride, y_pre, y_pre_stride, window_error_quad_fp8);

        calculate_squared_errors_sum_2x8x8_no_div_neon(
            y_src + y_src_stride * 8, y_src_stride, y_pre + y_pre_stride * 8, y_pre_stride, &window_error_quad_fp8[2]);

        uint32x4_t window_error_quad_fp8_v = vld1q_u32(window_error_quad_fp8);
        window_error_quad_fp8_v            = vshlq_n_u32(window_error_quad_fp8_v, 2);

        vst1q_u32(window_error_quad_fp8, window_error_quad_fp8_v);
    }

    if (is_chroma) {
        for (i = 0; i < 4; ++i) {
            FP_ASSERT(((int64_t)window_error_quad_fp8[i] * 5 + luma_window_error_quad_fp8[i]) < ((int64_t)1 << 31));
        }

        uint32x4_t window_error_quad_fp8_v      = vld1q_u32(window_error_quad_fp8);
        uint32x4_t luma_window_error_quad_fp8_v = vld1q_u32(luma_window_error_quad_fp8);

        window_error_quad_fp8_v = vmlaq_u32(luma_window_error_quad_fp8_v, window_error_quad_fp8_v, vdupq_n_u32(5));
        window_error_quad_fp8_v = vcvtq_u32_f32(vdivq_f32(vcvtq_f32_u32(window_error_quad_fp8_v), vdupq_n_f32(6.0f)));

        vst1q_u32(window_error_quad_fp8, window_error_quad_fp8_v);
    }

    int16x8_t adjusted_weight_int16[4];
    int32x4_t adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                       block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        uint64_t avg_err_fp10    = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        uint32_t scaled_diff16   = (uint32_t)AOMMIN((avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        uint32_t adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        adjusted_weight_int16[subblock_idx] = vdupq_n_s16((int16_t)(adjusted_weight));
        adjusted_weight_int32[subblock_idx] = vdupq_n_s32((int32_t)(adjusted_weight));
    }

    for (i = 0; i < block_height; i++) {
        const int subblock_idx_h = (i >= block_height / 2) * 2;
        for (j = 0; j < block_width; j += 8) {
            k = i * y_pre_stride + j;

            uint16x8_t count_array = vld1q_u16(y_count + k);

            count_array = vaddq_u16(
                count_array, vreinterpretq_u16_s16(adjusted_weight_int16[subblock_idx_h + (j >= block_width / 2)]));

            vst1q_u16(y_count + k, count_array);
            uint32x4_t accumulator_array1 = vld1q_u32(y_accum + k);
            uint32x4_t accumulator_array2 = vld1q_u32(y_accum + k + 4);

            uint16x8_t frame2_array       = vmovl_u8(vld1_u8(y_pre + k));
            uint32x4_t frame2_array_u32_1 = vmovl_u16(vget_low_u16(frame2_array));
            uint32x4_t frame2_array_u32_2 = vmovl_u16(vget_high_u16(frame2_array));

            uint32x4_t adj_weight = vreinterpretq_u32_s32(
                adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            frame2_array_u32_1 = vmulq_u32(frame2_array_u32_1, adj_weight);
            frame2_array_u32_2 = vmulq_u32(frame2_array_u32_2, adj_weight);

            accumulator_array1 = vaddq_u32(accumulator_array1, frame2_array_u32_1);
            accumulator_array2 = vaddq_u32(accumulator_array2, frame2_array_u32_2);

            vst1q_u32(y_accum + k, accumulator_array1);
            vst1q_u32(y_accum + k + 4, accumulator_array2);
        }
    }
}

void svt_av1_apply_temporal_filter_planewise_medium_neon(
    MeContext* me_ctx, const uint8_t* y_src, int y_src_stride, const uint8_t* y_pre, int y_pre_stride,
    const uint8_t* u_src, const uint8_t* v_src, int uv_src_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_partial_neon(me_ctx,
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
        svt_av1_apply_temporal_filter_planewise_medium_partial_neon(me_ctx,
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

        svt_av1_apply_temporal_filter_planewise_medium_partial_neon(me_ctx,
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

// Divide two int32x4 vectors
static uint32x4_t div_u32(const uint32x4_t* a, const uint32x4_t* b) {
    uint32x4_t result = vdupq_n_u32(0);
    result            = vsetq_lane_u32(vdups_laneq_u32(*a, 0) / vdups_laneq_u32(*b, 0), result, 0);
    result            = vsetq_lane_u32(vdups_laneq_u32(*a, 1) / vdups_laneq_u32(*b, 1), result, 1);
    result            = vsetq_lane_u32(vdups_laneq_u32(*a, 2) / vdups_laneq_u32(*b, 2), result, 2);
    result            = vsetq_lane_u32(vdups_laneq_u32(*a, 3) / vdups_laneq_u32(*b, 3), result, 3);
    return result;
}

static void process_block_hbd_neon(int h, int w, uint16_t* buff_hbd_start, uint32_t* accum, uint16_t* count,
                                   uint32_t stride) {
    int i, j;
    int pos = 0;
    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 8) {
            //buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            //buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            uint32x4_t accum_a = vld1q_u32(accum);
            uint32x4_t accum_b = vld1q_u32(&accum[4]);
            accum += 8;

            uint32x4_t count_a = vmovl_u16(vld1_u16(count));
            uint32x4_t count_b = vmovl_u16(vld1_u16(&count[4]));
            count += 8;

            //accum[k] + (count[k] >> 1)
            uint32x4_t tmp_a = vaddq_u32(accum_a, vshrq_n_u32(count_a, 1));
            uint32x4_t tmp_b = vaddq_u32(accum_b, vshrq_n_u32(count_b, 1));

            //accum[k] + (count[k] >> 1))/ count[k]
            tmp_a             = div_u32(&tmp_a, &count_a);
            tmp_b             = div_u32(&tmp_b, &count_b);
            uint16x8_t tmp_ab = vqmovn_high_u32(vqmovn_u32(tmp_a), tmp_b);

            vst1q_u16(buff_hbd_start + pos, tmp_ab);

            pos += 8;
        }
        pos += stride;
    }
}

static void process_block_lbd_neon(int h, int w, uint8_t* buff_lbd_start, uint32_t* accum, uint16_t* count,
                                   uint32_t stride) {
    int i, j;
    int pos = 0;
    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 8) {
            //buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            //buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            uint32x4_t accum_a = vld1q_u32(accum);
            uint32x4_t accum_b = vld1q_u32(&accum[4]);
            accum += 8;

            uint32x4_t count_a = vmovl_u16(vld1_u16(count));
            uint32x4_t count_b = vmovl_u16(vld1_u16(&count[4]));
            count += 8;

            //accum[k] + (count[k] >> 1)
            uint32x4_t tmp_a = vaddq_u32(accum_a, vshrq_n_u32(count_a, 1));
            uint32x4_t tmp_b = vaddq_u32(accum_b, vshrq_n_u32(count_b, 1));

            //accum[k] + (count[k] >> 1))/ count[k]
            tmp_a              = div_u32(&tmp_a, &count_a);
            tmp_b              = div_u32(&tmp_b, &count_b);
            uint16x8_t tmp_ab1 = vqmovn_high_u32(vqmovn_u32(tmp_a), tmp_b);

            vst1_u8(buff_lbd_start + pos, vqmovn_u16(tmp_ab1));

            pos += 8;
        }
        pos += stride;
    }
}

void svt_aom_get_final_filtered_pixels_neon(MeContext* me_ctx, EbByte* src_center_ptr_start,
                                            uint16_t** altref_buffer_highbd_start, uint32_t** accum, uint16_t** count,
                                            const uint32_t* stride, int blk_y_src_offset, int blk_ch_src_offset,
                                            uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd) {
    assert(blk_width_ch % 16 == 0);
    assert(BW % 16 == 0);

    if (!is_highbd) {
        //Process luma
        process_block_lbd_neon(
            BH, BW, &src_center_ptr_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_lbd_neon(blk_height_ch,
                                   blk_width_ch,
                                   &src_center_ptr_start[C_U][blk_ch_src_offset],
                                   accum[C_U],
                                   count[C_U],
                                   stride[C_U] - blk_width_ch);
            process_block_lbd_neon(blk_height_ch,
                                   blk_width_ch,
                                   &src_center_ptr_start[C_V][blk_ch_src_offset],
                                   accum[C_V],
                                   count[C_V],
                                   stride[C_V] - blk_width_ch);
        }
    } else {
        // Process luma
        process_block_hbd_neon(
            BH, BW, &altref_buffer_highbd_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_hbd_neon(blk_height_ch,
                                   blk_width_ch,
                                   &altref_buffer_highbd_start[C_U][blk_ch_src_offset],
                                   accum[C_U],
                                   count[C_U],
                                   stride[C_U] - blk_width_ch);
            process_block_hbd_neon(blk_height_ch,
                                   blk_width_ch,
                                   &altref_buffer_highbd_start[C_V][blk_ch_src_offset],
                                   accum[C_V],
                                   count[C_V],
                                   stride[C_V] - blk_width_ch);
        }
    }
}

int32_t svt_estimate_noise_fp16_neon(const uint8_t* src, uint16_t width, uint16_t height, uint16_t stride_y) {
    uint16x8_t thresh = vdupq_n_u16(EDGE_THRESHOLD);
    uint32x4_t acc    = vdupq_n_u32(0);
    // Count is in theory positive as it counts the number of times we're under
    // the threshold, but it will be counted negatively in order to make best use
    // of the vclt instruction, which sets every bit of a lane to 1 when the
    // condition is true.
    int32x4_t      count       = vdupq_n_s32(0);
    int            final_count = 0;
    int64_t        final_acc   = 0;
    const uint8_t* src_start   = src + stride_y + 1;
    int            h           = 1;

    do {
        int            w       = 1;
        const uint8_t* src_ptr = src_start;

        while (w <= (width - 1) - 16) {
            uint8x16_t mat[3][3];
            mat[0][0] = vld1q_u8(src_ptr - stride_y - 1);
            mat[0][1] = vld1q_u8(src_ptr - stride_y);
            mat[0][2] = vld1q_u8(src_ptr - stride_y + 1);
            mat[1][0] = vld1q_u8(src_ptr - 1);
            mat[1][1] = vld1q_u8(src_ptr);
            mat[1][2] = vld1q_u8(src_ptr + 1);
            mat[2][0] = vld1q_u8(src_ptr + stride_y - 1);
            mat[2][1] = vld1q_u8(src_ptr + stride_y);
            mat[2][2] = vld1q_u8(src_ptr + stride_y + 1);

            // Compute Sobel gradients.
            uint16x8_t gxa_lo = vaddl_u8(vget_low_u8(mat[0][0]), vget_low_u8(mat[2][0]));
            uint16x8_t gxa_hi = vaddl_u8(vget_high_u8(mat[0][0]), vget_high_u8(mat[2][0]));
            uint16x8_t gxb_lo = vaddl_u8(vget_low_u8(mat[0][2]), vget_low_u8(mat[2][2]));
            uint16x8_t gxb_hi = vaddl_u8(vget_high_u8(mat[0][2]), vget_high_u8(mat[2][2]));
            gxa_lo            = vaddq_u16(gxa_lo, vaddl_u8(vget_low_u8(mat[1][0]), vget_low_u8(mat[1][0])));
            gxa_hi            = vaddq_u16(gxa_hi, vaddl_u8(vget_high_u8(mat[1][0]), vget_high_u8(mat[1][0])));
            gxb_lo            = vaddq_u16(gxb_lo, vaddl_u8(vget_low_u8(mat[1][2]), vget_low_u8(mat[1][2])));
            gxb_hi            = vaddq_u16(gxb_hi, vaddl_u8(vget_high_u8(mat[1][2]), vget_high_u8(mat[1][2])));

            uint16x8_t gya_lo = vaddl_u8(vget_low_u8(mat[0][0]), vget_low_u8(mat[0][2]));
            uint16x8_t gya_hi = vaddl_u8(vget_high_u8(mat[0][0]), vget_high_u8(mat[0][2]));
            uint16x8_t gyb_lo = vaddl_u8(vget_low_u8(mat[2][0]), vget_low_u8(mat[2][2]));
            uint16x8_t gyb_hi = vaddl_u8(vget_high_u8(mat[2][0]), vget_high_u8(mat[2][2]));
            gya_lo            = vaddq_u16(gya_lo, vaddl_u8(vget_low_u8(mat[0][1]), vget_low_u8(mat[0][1])));
            gya_hi            = vaddq_u16(gya_hi, vaddl_u8(vget_high_u8(mat[0][1]), vget_high_u8(mat[0][1])));
            gyb_lo            = vaddq_u16(gyb_lo, vaddl_u8(vget_low_u8(mat[2][1]), vget_low_u8(mat[2][1])));
            gyb_hi            = vaddq_u16(gyb_hi, vaddl_u8(vget_high_u8(mat[2][1]), vget_high_u8(mat[2][1])));

            uint16x8_t ga_lo = vabaq_u16(vabdq_u16(gxa_lo, gxb_lo), gya_lo, gyb_lo);
            uint16x8_t ga_hi = vabaq_u16(vabdq_u16(gxa_hi, gxb_hi), gya_hi, gyb_hi);

            // Check which vector elements are under the threshold. The Laplacian is
            // then unconditionally computed and we accumulate zeros if we're not
            // under the threshold. This is much faster than using an if statement.
            uint16x8_t thresh_u16_lo = vcltq_u16(ga_lo, thresh);
            uint16x8_t thresh_u16_hi = vcltq_u16(ga_hi, thresh);

            uint16x8_t center_lo = vshll_n_u8(vget_low_u8(mat[1][1]), 2);
            uint16x8_t center_hi = vshll_n_u8(vget_high_u8(mat[1][1]), 2);

            uint16x8_t adj0_lo = vaddl_u8(vget_low_u8(mat[0][1]), vget_low_u8(mat[2][1]));
            uint16x8_t adj0_hi = vaddl_u8(vget_high_u8(mat[0][1]), vget_high_u8(mat[2][1]));
            uint16x8_t adj1_lo = vaddl_u8(vget_low_u8(mat[1][0]), vget_low_u8(mat[1][2]));
            uint16x8_t adj1_hi = vaddl_u8(vget_high_u8(mat[1][0]), vget_high_u8(mat[1][2]));
            uint16x8_t adj_lo  = vaddq_u16(adj0_lo, adj1_lo);
            adj_lo             = vaddq_u16(adj_lo, adj_lo);
            uint16x8_t adj_hi  = vaddq_u16(adj0_hi, adj1_hi);
            adj_hi             = vaddq_u16(adj_hi, adj_hi);

            uint16x8_t diag0_lo = vaddl_u8(vget_low_u8(mat[0][0]), vget_low_u8(mat[0][2]));
            uint16x8_t diag0_hi = vaddl_u8(vget_high_u8(mat[0][0]), vget_high_u8(mat[0][2]));
            uint16x8_t diag1_lo = vaddl_u8(vget_low_u8(mat[2][0]), vget_low_u8(mat[2][2]));
            uint16x8_t diag1_hi = vaddl_u8(vget_high_u8(mat[2][0]), vget_high_u8(mat[2][2]));
            uint16x8_t diag_lo  = vaddq_u16(diag0_lo, diag1_lo);
            uint16x8_t diag_hi  = vaddq_u16(diag0_hi, diag1_hi);

            uint16x8_t v_lo = vaddq_u16(center_lo, diag_lo);
            v_lo            = vabdq_u16(v_lo, adj_lo);
            uint16x8_t v_hi = vaddq_u16(center_hi, diag_hi);
            v_hi            = vabdq_u16(v_hi, adj_hi);

            acc = vpadalq_u16(acc, vandq_u16(v_lo, thresh_u16_lo));
            acc = vpadalq_u16(acc, vandq_u16(v_hi, thresh_u16_hi));

            // Add -1 for each lane where the gradient is under the threshold.
            count = vpadalq_s16(count, vreinterpretq_s16_u16(thresh_u16_lo));
            count = vpadalq_s16(count, vreinterpretq_s16_u16(thresh_u16_hi));

            w += 16;
            src_ptr += 16;
        }

        if (w <= (width - 1) - 8) {
            uint8x8_t mat[3][3];
            mat[0][0] = vld1_u8(src_ptr - stride_y - 1);
            mat[0][1] = vld1_u8(src_ptr - stride_y);
            mat[0][2] = vld1_u8(src_ptr - stride_y + 1);
            mat[1][0] = vld1_u8(src_ptr - 1);
            mat[1][1] = vld1_u8(src_ptr);
            mat[1][2] = vld1_u8(src_ptr + 1);
            mat[2][0] = vld1_u8(src_ptr + stride_y - 1);
            mat[2][1] = vld1_u8(src_ptr + stride_y);
            mat[2][2] = vld1_u8(src_ptr + stride_y + 1);

            // Compute Sobel gradients.
            uint16x8_t gxa = vaddl_u8(mat[0][0], mat[2][0]);
            uint16x8_t gxb = vaddl_u8(mat[0][2], mat[2][2]);
            gxa            = vaddq_u16(gxa, vaddl_u8(mat[1][0], mat[1][0]));
            gxb            = vaddq_u16(gxb, vaddl_u8(mat[1][2], mat[1][2]));

            uint16x8_t gya = vaddl_u8(mat[0][0], mat[0][2]);
            uint16x8_t gyb = vaddl_u8(mat[2][0], mat[2][2]);
            gya            = vaddq_u16(gya, vaddl_u8(mat[0][1], mat[0][1]));
            gyb            = vaddq_u16(gyb, vaddl_u8(mat[2][1], mat[2][1]));

            uint16x8_t ga = vabaq_u16(vabdq_u16(gxa, gxb), gya, gyb);

            // Check which vector elements are under the threshold. The Laplacian is
            // then unconditionally computed and we accumulate zeros if we're not
            // under the threshold. This is much faster than using an if statement.
            uint16x8_t thresh_u16 = vcltq_u16(ga, thresh);

            uint16x8_t center = vshll_n_u8(mat[1][1], 2);

            uint16x8_t adj0 = vaddl_u8(mat[0][1], mat[2][1]);
            uint16x8_t adj1 = vaddl_u8(mat[1][0], mat[1][2]);
            uint16x8_t adj  = vaddq_u16(adj0, adj1);
            adj             = vaddq_u16(adj, adj);

            uint16x8_t diag0 = vaddl_u8(mat[0][0], mat[0][2]);
            uint16x8_t diag1 = vaddl_u8(mat[2][0], mat[2][2]);
            uint16x8_t diag  = vaddq_u16(diag0, diag1);

            uint16x8_t v = vaddq_u16(center, diag);
            v            = vabdq_u16(v, adj);

            acc = vpadalq_u16(acc, vandq_u16(v, thresh_u16));
            // Add -1 for each lane where the gradient is under the threshold.
            count = vpadalq_s16(count, vreinterpretq_s16_u16(thresh_u16));

            w += 8;
            src_ptr += 8;
        }

        if (w <= (width - 1) - 4) {
            uint16x8_t mask = vcombine_u16(vdup_n_u16(65535), vdup_n_u16(0));
            uint8x8_t  mat[3][3];
            mat[0][0] = load_u8_4x1(src_ptr - stride_y - 1);
            mat[0][1] = load_u8_4x1(src_ptr - stride_y);
            mat[0][2] = load_u8_4x1(src_ptr - stride_y + 1);
            mat[1][0] = load_u8_4x1(src_ptr - 1);
            mat[1][1] = load_u8_4x1(src_ptr);
            mat[1][2] = load_u8_4x1(src_ptr + 1);
            mat[2][0] = load_u8_4x1(src_ptr + stride_y - 1);
            mat[2][1] = load_u8_4x1(src_ptr + stride_y);
            mat[2][2] = load_u8_4x1(src_ptr + stride_y + 1);

            // Compute Sobel gradients.
            uint16x8_t gxa = vaddl_u8(mat[0][0], mat[2][0]);
            uint16x8_t gxb = vaddl_u8(mat[0][2], mat[2][2]);
            gxa            = vaddq_u16(gxa, vaddl_u8(mat[1][0], mat[1][0]));
            gxb            = vaddq_u16(gxb, vaddl_u8(mat[1][2], mat[1][2]));

            uint16x8_t gya = vaddl_u8(mat[0][0], mat[0][2]);
            uint16x8_t gyb = vaddl_u8(mat[2][0], mat[2][2]);
            gya            = vaddq_u16(gya, vaddl_u8(mat[0][1], mat[0][1]));
            gyb            = vaddq_u16(gyb, vaddl_u8(mat[2][1], mat[2][1]));

            uint16x8_t ga = vabaq_u16(vabdq_u16(gxa, gxb), gya, gyb);

            // Check which vector elements are under the threshold. The Laplacian is
            // then unconditionally computed and we accumulate zeros if we're not
            // under the threshold. This is much faster than using an if statement.
            uint16x8_t thresh_u16 = vandq_u16(vcltq_u16(ga, thresh), mask);

            uint16x8_t center = vshll_n_u8(mat[1][1], 2);

            uint16x8_t adj0 = vaddl_u8(mat[0][1], mat[2][1]);
            uint16x8_t adj1 = vaddl_u8(mat[1][0], mat[1][2]);
            uint16x8_t adj  = vaddq_u16(adj0, adj1);
            adj             = vaddq_u16(adj, adj);

            uint16x8_t diag0 = vaddl_u8(mat[0][0], mat[0][2]);
            uint16x8_t diag1 = vaddl_u8(mat[2][0], mat[2][2]);
            uint16x8_t diag  = vaddq_u16(diag0, diag1);

            uint16x8_t v = vaddq_u16(center, diag);
            v            = vabdq_u16(v, adj);

            acc = vpadalq_u16(acc, vandq_u16(v, thresh_u16));
            // Add -1 for each lane where the gradient is under the threshold.
            count = vpadalq_s16(count, vreinterpretq_s16_u16(thresh_u16));

            w += 4;
            src_ptr += 4;
        }

        while (w < width - 1) {
            int mat[3][3];
            mat[0][0] = *(src_ptr - stride_y - 1);
            mat[0][1] = *(src_ptr - stride_y);
            mat[0][2] = *(src_ptr - stride_y + 1);
            mat[1][0] = *(src_ptr - 1);
            mat[1][1] = *(src_ptr);
            mat[1][2] = *(src_ptr + 1);
            mat[2][0] = *(src_ptr + stride_y - 1);
            mat[2][1] = *(src_ptr + stride_y);
            mat[2][2] = *(src_ptr + stride_y + 1);

            // Compute Sobel gradients.
            const int gx = (mat[0][0] - mat[0][2]) + (mat[2][0] - mat[2][2]) + 2 * (mat[1][0] - mat[1][2]);
            const int gy = (mat[0][0] - mat[2][0]) + (mat[0][2] - mat[2][2]) + 2 * (mat[0][1] - mat[2][1]);
            const int ga = abs(gx) + abs(gy);

            // Accumulate Laplacian.
            const int is_under = ga < EDGE_THRESHOLD;
            const int v        = 4 * mat[1][1] - 2 * (mat[0][1] + mat[2][1] + mat[1][0] + mat[1][2]) +
                (mat[0][0] + mat[0][2] + mat[2][0] + mat[2][2]);
            final_acc += abs(v) * is_under;
            final_count += is_under;

            src_ptr++;
            w++;
        }
        src_start += stride_y;
    } while (++h < height - 1);

    // We counted negatively, so subtract to get the final value.
    final_count -= vaddvq_s32(count);
    final_acc += vaddlvq_u32(acc);
    return (final_count < SMOOTH_THRESHOLD) ? -1.0 : (double)(final_acc * SQRT_PI_BY_2_FP16) / (6 * final_count);
}

static void apply_filtering_central_loop_lbd(uint16_t w, uint16_t h, uint8_t* src, uint16_t src_stride, uint32_t* accum,
                                             uint16_t* count) {
    assert(w % 8 == 0);

    uint32x4_t modifier       = vdupq_n_u32(TF_PLANEWISE_FILTER_WEIGHT_SCALE);
    uint16x8_t modifier_epi16 = vdupq_n_u16(TF_PLANEWISE_FILTER_WEIGHT_SCALE);

    for (uint16_t k = 0, i = 0; i < h; i++) {
        for (uint16_t j = 0; j < w; j += 8) {
            const uint16x8_t src_16 = vmovl_u8(vld1_u8(src + i * src_stride + j));

            vst1q_u32(accum + k + 0, vmulq_u32(modifier, vmovl_u16(vget_low_u16(src_16))));
            vst1q_u32(accum + k + 4, vmulq_u32(modifier, vmovl_u16(vget_high_u16(src_16))));
            vst1q_u16(count + k, modifier_epi16);

            k += 8;
        }
    }
}

static uint32_t calculate_squared_errors_sum_no_div_highbd_neon(const uint16_t* s, int s_stride, const uint16_t* p,
                                                                int p_stride, unsigned int w, unsigned int h,
                                                                int const shift_factor) {
    assert(w % 16 == 0 && "block width must be multiple of 16");
    unsigned int i, j;

    int32x4_t sum = vdupq_n_s32(0);

    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j += 8) {
            const uint16x8_t s_16 = vld1q_u16(s + i * s_stride + j);
            const uint16x8_t p_16 = vld1q_u16(p + i * p_stride + j);

            const int16x8_t dif = vreinterpretq_s16_u16(vsubq_u16(s_16, p_16));
            sum = vaddq_s32(sum, vpaddq_s32(vmull_s16(vget_low_s16(dif), vget_low_s16(dif)), vmull_high_s16(dif, dif)));
        }
    }

    sum = vpaddq_s32(sum, sum);
    sum = vpaddq_s32(sum, sum);

    return vgetq_lane_s32(sum, 0) >> shift_factor;
}

static void calculate_squared_errors_sum_2x8xh_no_div_highbd_neon(const uint16_t* s, int s_stride, const uint16_t* p,
                                                                  int p_stride, unsigned int h, int shift_factor,
                                                                  uint32_t* output) {
    const int32x4_t zero  = vdupq_n_s32(0);
    int32x4_t       sum_0 = zero;
    int32x4_t       sum_1 = zero;

    for (unsigned int i = 0; i < h; i++) {
        const uint16x8_t s_8_0 = vld1q_u16(s + i * s_stride);
        const uint16x8_t s_8_1 = vld1q_u16(s + i * s_stride + 8);
        const uint16x8_t p_8_0 = vld1q_u16(p + i * p_stride);
        const uint16x8_t p_8_1 = vld1q_u16(p + i * p_stride + 8);

        const int16x8_t dif_0 = vreinterpretq_s16_u16(vsubq_u16(s_8_0, p_8_0));
        const int16x8_t dif_1 = vreinterpretq_s16_u16(vsubq_u16(s_8_1, p_8_1));

        sum_0 = vaddq_s32(
            sum_0, vpaddq_s32(vmull_s16(vget_low_s16(dif_0), vget_low_s16(dif_0)), vmull_high_s16(dif_0, dif_0)));
        sum_1 = vaddq_s32(
            sum_1, vpaddq_s32(vmull_s16(vget_low_s16(dif_1), vget_low_s16(dif_1)), vmull_high_s16(dif_1, dif_1)));
    }
    sum_0 = vpaddq_s32(sum_0, sum_0);
    sum_0 = vpaddq_s32(sum_0, sum_0);
    sum_1 = vpaddq_s32(sum_1, sum_1);
    sum_1 = vpaddq_s32(sum_1, sum_1);

    output[0] = vgetq_lane_s32(sum_0, 0) >> shift_factor;
    output[1] = vgetq_lane_s32(sum_1, 0) >> shift_factor;
}

static void svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_neon(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    unsigned int block_width, unsigned int block_height, uint32_t* y_accum, uint16_t* y_count, uint32_t tf_decay_factor,
    uint32_t luma_window_error_quad_fp8[4], int is_chroma, uint32_t encoder_bit_depth) {
    unsigned int i, j, k, subblock_idx;

    const int32_t  idx_32x32               = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;
    const int      shift_factor            = ((encoder_bit_depth - 8) * 2);
    const uint32_t distance_threshold_fp16 = AOMMAX((me_ctx->tf_mv_dist_th << 16) / 10,
                                                    1 << 16); //TODO Change to FP8

    //Calculation for every quarter
    uint32_t  d_factor_fp8[4];
    uint32_t  block_error_fp8[4];
    uint32_t  chroma_window_error_quad_fp8[4];
    uint32_t* window_error_quad_fp8 = is_chroma ? chroma_window_error_quad_fp8 : luma_window_error_quad_fp8;

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        for (i = 0; i < 4; ++i) {
            const int32_t  col          = me_ctx->tf_16x16_mv_x[idx_32x32 * 4 + i];
            const int32_t  row          = me_ctx->tf_16x16_mv_y[idx_32x32 * 4 + i];
            const uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
            d_factor_fp8[i]             = AOMMAX((distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
            FP_ASSERT(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] < ((uint64_t)1 << 35));
            block_error_fp8[i] = (uint32_t)(me_ctx->tf_16x16_block_error[idx_32x32 * 4 + i] >> 4);
        }
    } else {
        tf_decay_factor <<= 1;
        const int32_t col = me_ctx->tf_32x32_mv_x[idx_32x32];
        const int32_t row = me_ctx->tf_32x32_mv_y[idx_32x32];

        const uint32_t distance_fp4 = sqrt_fast(((uint32_t)(col * col + row * row)) << 8);
        d_factor_fp8[0] = d_factor_fp8[1] = d_factor_fp8[2] = d_factor_fp8[3] = AOMMAX(
            (distance_fp4 << 12) / (distance_threshold_fp16 >> 8), 1 << 8);
        FP_ASSERT(me_ctx->tf_32x32_block_error[idx_32x32] < ((uint64_t)1 << 35));
        block_error_fp8[0] = block_error_fp8[1] = block_error_fp8[2] = block_error_fp8[3] =
            (uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 6);
    }

    if (block_width == 32) {
        window_error_quad_fp8[0] = calculate_squared_errors_sum_no_div_highbd_neon(
            y_src, y_src_stride, y_pre, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[1] = calculate_squared_errors_sum_no_div_highbd_neon(
            y_src + 16, y_src_stride, y_pre + 16, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[2] = calculate_squared_errors_sum_no_div_highbd_neon(
            y_src + y_src_stride * 16, y_src_stride, y_pre + y_pre_stride * 16, y_pre_stride, 16, 16, shift_factor);
        window_error_quad_fp8[3] = calculate_squared_errors_sum_no_div_highbd_neon(y_src + y_src_stride * 16 + 16,
                                                                                   y_src_stride,
                                                                                   y_pre + y_pre_stride * 16 + 16,
                                                                                   y_pre_stride,
                                                                                   16,
                                                                                   16,
                                                                                   shift_factor);

    } else {
        calculate_squared_errors_sum_2x8xh_no_div_highbd_neon(
            y_src, y_src_stride, y_pre, y_pre_stride, 8, shift_factor, window_error_quad_fp8);

        calculate_squared_errors_sum_2x8xh_no_div_highbd_neon(y_src + y_src_stride * 8,
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

    uint16x8_t adjusted_weight_int16[4];
    uint32x4_t adjusted_weight_int32[4];

    for (subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        const uint32_t combined_error_fp8 = (window_error_quad_fp8[subblock_idx] * TF_WINDOW_BLOCK_BALANCE_WEIGHT +
                                             block_error_fp8[subblock_idx]) /
            (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1);

        const uint64_t avg_err_fp10  = ((combined_error_fp8 >> 3) * (d_factor_fp8[subblock_idx] >> 3));
        uint32_t       scaled_diff16 = (uint32_t)AOMMIN(
            /*((16*avg_err)<<8)*/ (avg_err_fp10) / AOMMAX((tf_decay_factor >> 10), 1), 7 * 16);
        const int adjusted_weight = (expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 16;

        adjusted_weight_int16[subblock_idx] = vdupq_n_u16(adjusted_weight);
        adjusted_weight_int32[subblock_idx] = vdupq_n_u32(adjusted_weight);
    }

    for (i = 0; i < block_height; i++) {
        const int subblock_idx_h = (i >= block_height / 2) * 2;
        for (j = 0; j < block_width; j += 8) {
            k = i * y_pre_stride + j;

            //y_count[k] += adjusted_weight;
            uint16x8_t count_array = vld1q_u16(y_count + k);
            count_array = vaddq_u16(count_array, adjusted_weight_int16[subblock_idx_h + (j >= block_width / 2)]);
            vst1q_u16(y_count + k, count_array);

            //y_accum[k] += adjusted_weight * pixel_value;
            uint32x4_t       accumulator_array1 = vld1q_u32(y_accum + k);
            uint32x4_t       accumulator_array2 = vld1q_u32(y_accum + k + 4);
            const uint16x8_t frame2_array       = vld1q_u16(y_pre + k);
            uint32x4_t       frame2_array_u32_1 = vmovl_u16(vget_low_u16(frame2_array));
            uint32x4_t       frame2_array_u32_2 = vmovl_u16(vget_high_u16(frame2_array));
            frame2_array_u32_1                  = vmulq_u32(frame2_array_u32_1,
                                           adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);
            frame2_array_u32_2                  = vmulq_u32(frame2_array_u32_2,
                                           adjusted_weight_int32[subblock_idx_h + (j >= block_width / 2)]);

            accumulator_array1 = vaddq_u32(accumulator_array1, frame2_array_u32_1);
            accumulator_array2 = vaddq_u32(accumulator_array2, frame2_array_u32_2);
            vst1q_u32(y_accum + k, accumulator_array1);
            vst1q_u32(y_accum + k + 4, accumulator_array2);
        }
    }
}

static void apply_filtering_central_loop_hbd(uint16_t w, uint16_t h, uint16_t* src, uint16_t src_stride,
                                             uint32_t* accum, uint16_t* count) {
    assert(w % 8 == 0);

    uint32x4_t modifier       = vdupq_n_u32(TF_PLANEWISE_FILTER_WEIGHT_SCALE);
    uint16x8_t modifier_epi16 = vdupq_n_u16(TF_PLANEWISE_FILTER_WEIGHT_SCALE);

    for (uint16_t k = 0, i = 0; i < h; i++) {
        for (uint16_t j = 0; j < w; j += 8) {
            const uint32x4_t src_1 = vmovl_u16(vld1_u16(src + i * src_stride + j + 0));
            const uint32x4_t src_2 = vmovl_u16(vld1_u16(src + i * src_stride + j + 4));

            vst1q_u32(accum + k + 0, vmulq_u32(modifier, src_1));
            vst1q_u32(accum + k + 4, vmulq_u32(modifier, src_2));
            vst1q_u16(count + k, modifier_epi16);

            k += 8;
        }
    }
}

void svt_aom_apply_filtering_central_neon(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
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

void svt_aom_apply_filtering_central_highbd_neon(MeContext* me_ctx, EbPictureBufferDesc* input_picture_ptr_central,
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

void svt_av1_apply_temporal_filter_planewise_medium_hbd_neon(
    MeContext* me_ctx, const uint16_t* y_src, int y_src_stride, const uint16_t* y_pre, int y_pre_stride,
    const uint16_t* u_src, const uint16_t* v_src, int uv_src_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    uint32_t luma_window_error_quad_fp8[4];

    svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_neon(me_ctx,
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
        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_neon(me_ctx,
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

        svt_av1_apply_temporal_filter_planewise_medium_hbd_partial_neon(me_ctx,
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

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int32_t svt_estimate_noise_highbd_fp16_neon(const uint16_t* src, int width, int height, int stride, int bd) {
    //  A | B | C
    //  D | E | F
    //  G | H | I
    // g_x = (A - I) + (G - C) + 2*(D - F)
    // g_y = (A - I) - (G - C) + 2*(B - H)
    // v   = 4*E - 2*(D+F+B+H) + (A+C+G+I)

    uint16x8_t thresh = vdupq_n_u16(EDGE_THRESHOLD);
    uint64x2_t acc    = vdupq_n_u64(0);
    // Count is in theory positive as it counts the number of times we're under
    // the threshold, but it will be counted negatively in order to make best use
    // of the vclt instruction, which sets every bit of a lane to 1 when the
    // condition is true.
    int32x4_t       count       = vdupq_n_s32(0);
    int             final_count = 0;
    uint64_t        final_acc   = 0;
    const uint16_t* src_start   = src + stride + 1;
    int             h           = 1;

    do {
        int             w       = 1;
        const uint16_t* src_ptr = src_start;

        while (w <= (width - 1) - 8) {
            uint16x8_t mat[3][3];
            mat[0][0] = vld1q_u16(src_ptr - stride - 1);
            mat[0][1] = vld1q_u16(src_ptr - stride);
            mat[0][2] = vld1q_u16(src_ptr - stride + 1);
            mat[1][0] = vld1q_u16(src_ptr - 1);
            mat[1][1] = vld1q_u16(src_ptr);
            mat[1][2] = vld1q_u16(src_ptr + 1);
            mat[2][0] = vld1q_u16(src_ptr + stride - 1);
            mat[2][1] = vld1q_u16(src_ptr + stride);
            mat[2][2] = vld1q_u16(src_ptr + stride + 1);

            // Compute Sobel gradients.
            uint16x8_t gxa = vaddq_u16(mat[0][0], mat[2][0]);
            uint16x8_t gxb = vaddq_u16(mat[0][2], mat[2][2]);
            gxa            = vaddq_u16(gxa, vaddq_u16(mat[1][0], mat[1][0]));
            gxb            = vaddq_u16(gxb, vaddq_u16(mat[1][2], mat[1][2]));

            uint16x8_t gya = vaddq_u16(mat[0][0], mat[0][2]);
            uint16x8_t gyb = vaddq_u16(mat[2][0], mat[2][2]);
            gya            = vaddq_u16(gya, vaddq_u16(mat[0][1], mat[0][1]));
            gyb            = vaddq_u16(gyb, vaddq_u16(mat[2][1], mat[2][1]));

            uint16x8_t ga = vabaq_u16(vabdq_u16(gxa, gxb), gya, gyb);
            ga            = vrshlq_u16(ga, vdupq_n_s16(8 - bd));

            // Check which vector elements are under the threshold. The Laplacian is
            // then unconditionally computed and we accumulate zeros if we're not
            // under the threshold. This is much faster than using an if statement.
            uint16x8_t thresh_u16 = vcltq_u16(ga, thresh);

            uint16x8_t center = vshlq_n_u16(mat[1][1], 2);

            uint16x8_t adj0 = vaddq_u16(mat[0][1], mat[2][1]);
            uint16x8_t adj1 = vaddq_u16(mat[1][0], mat[1][2]);
            uint16x8_t adj  = vaddq_u16(adj0, adj1);
            adj             = vaddq_u16(adj, adj);

            uint16x8_t diag0 = vaddq_u16(mat[0][0], mat[0][2]);
            uint16x8_t diag1 = vaddq_u16(mat[2][0], mat[2][2]);
            uint16x8_t diag  = vaddq_u16(diag0, diag1);

            uint16x8_t v     = vabdq_u16(vaddq_u16(center, diag), adj);
            v                = vandq_u16(vrshlq_u16(v, vdupq_n_s16(8 - bd)), thresh_u16);
            uint32x4_t v_u32 = vpaddlq_u16(v);

            acc = vpadalq_u32(acc, v_u32);
            // Add -1 for each lane where the gradient is under the threshold.
            count = vpadalq_s16(count, vreinterpretq_s16_u16(thresh_u16));

            w += 8;
            src_ptr += 8;
        }

        if (w <= (width - 1) - 4) {
            uint16x4_t mat[3][3];
            mat[0][0] = vld1_u16(src_ptr - stride - 1);
            mat[0][1] = vld1_u16(src_ptr - stride);
            mat[0][2] = vld1_u16(src_ptr - stride + 1);
            mat[1][0] = vld1_u16(src_ptr - 1);
            mat[1][1] = vld1_u16(src_ptr);
            mat[1][2] = vld1_u16(src_ptr + 1);
            mat[2][0] = vld1_u16(src_ptr + stride - 1);
            mat[2][1] = vld1_u16(src_ptr + stride);
            mat[2][2] = vld1_u16(src_ptr + stride + 1);

            // Compute Sobel gradients.
            uint16x4_t gxa = vadd_u16(mat[0][0], mat[2][0]);
            uint16x4_t gxb = vadd_u16(mat[0][2], mat[2][2]);
            gxa            = vadd_u16(gxa, vadd_u16(mat[1][0], mat[1][0]));
            gxb            = vadd_u16(gxb, vadd_u16(mat[1][2], mat[1][2]));

            uint16x4_t gya = vadd_u16(mat[0][0], mat[0][2]);
            uint16x4_t gyb = vadd_u16(mat[2][0], mat[2][2]);
            gya            = vadd_u16(gya, vadd_u16(mat[0][1], mat[0][1]));
            gyb            = vadd_u16(gyb, vadd_u16(mat[2][1], mat[2][1]));

            uint16x4_t ga = vaba_u16(vabd_u16(gxa, gxb), gya, gyb);
            ga            = vrshl_u16(ga, vdup_n_s16(8 - bd));

            // Check which vector elements are under the threshold. The Laplacian is
            // then unconditionally computed and we accumulate zeros if we're not
            // under the threshold. This is much faster than using an if statement.
            uint16x4_t thresh_u16 = vclt_u16(ga, vget_low_u16(thresh));

            uint16x4_t center = vshl_n_u16(mat[1][1], 2);

            uint16x4_t adj0 = vadd_u16(mat[0][1], mat[2][1]);
            uint16x4_t adj1 = vadd_u16(mat[1][0], mat[1][2]);
            uint16x4_t adj  = vadd_u16(adj0, adj1);
            adj             = vadd_u16(adj, adj);

            uint16x4_t diag0 = vadd_u16(mat[0][0], mat[0][2]);
            uint16x4_t diag1 = vadd_u16(mat[2][0], mat[2][2]);
            uint16x4_t diag  = vadd_u16(diag0, diag1);

            uint16x4_t v     = vabd_u16(vadd_u16(center, diag), adj);
            v                = vand_u16(v, thresh_u16);
            uint32x4_t v_u32 = vmovl_u16(vrshl_u16(v, vdup_n_s16(8 - bd)));

            acc = vpadalq_u32(acc, v_u32);
            // Add -1 for each lane where the gradient is under the threshold.
            count = vaddw_s16(count, vreinterpret_s16_u16(thresh_u16));

            w += 4;
            src_ptr += 4;
        }

        while (w < width - 1) {
            int mat[3][3];
            mat[0][0] = *(src_ptr - stride - 1);
            mat[0][1] = *(src_ptr - stride);
            mat[0][2] = *(src_ptr - stride + 1);
            mat[1][0] = *(src_ptr - 1);
            mat[1][1] = *(src_ptr);
            mat[1][2] = *(src_ptr + 1);
            mat[2][0] = *(src_ptr + stride - 1);
            mat[2][1] = *(src_ptr + stride);
            mat[2][2] = *(src_ptr + stride + 1);

            // Compute Sobel gradients.
            const int gx = (mat[0][0] - mat[0][2]) + (mat[2][0] - mat[2][2]) + 2 * (mat[1][0] - mat[1][2]);
            const int gy = (mat[0][0] - mat[2][0]) + (mat[0][2] - mat[2][2]) + 2 * (mat[0][1] - mat[2][1]);
            const int ga = ROUND_POWER_OF_TWO(abs(gx) + abs(gy), bd - 8);

            // Accumulate Laplacian.
            const int is_under = ga < EDGE_THRESHOLD;
            const int v        = 4 * mat[1][1] - 2 * (mat[0][1] + mat[2][1] + mat[1][0] + mat[1][2]) +
                (mat[0][0] + mat[0][2] + mat[2][0] + mat[2][2]);
            final_acc += ROUND_POWER_OF_TWO(abs(v), bd - 8) * is_under;
            final_count += is_under;

            src_ptr++;
            w++;
        }
        src_start += stride;
    } while (++h < height - 1);

    // We counted negatively, so subtract to get the final value.
    final_count -= vaddvq_s32(count);
    final_acc += vaddvq_u64(acc);

    // If very few smooth pels, return -1 since the estimate is unreliable
    if (final_count < SMOOTH_THRESHOLD) {
        return -65536 /*-1:fp16*/;
    }

    FP_ASSERT((((int64_t)final_acc * SQRT_PI_BY_2_FP16) / (6 * final_count)) < ((int64_t)1 << 31));
    return (int32_t)((final_acc * SQRT_PI_BY_2_FP16) / (6 * final_count));
}
#endif

static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_neon(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor_fp16) {
    // Decay factors for non-local mean approach.
    // Larger noise -> larger filtering weight.
    int32_t idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;

    uint32_t block_error_fp8[4];

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        uint64x2_t b0 = vld1q_u64(&me_ctx->tf_16x16_block_error[idx_32x32 * 4]);
        uint64x2_t b1 = vld1q_u64(&me_ctx->tf_16x16_block_error[idx_32x32 * 4 + 2]);
        vst1q_u32(block_error_fp8, vcombine_u32(vmovn_u64(b0), vmovn_u64(b1)));
    } else {
        vst1q_u32(block_error_fp8, vdupq_n_u32((uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 2)));
    }

    // Calculation for every quarter.
    for (int subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]) << 2;
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx]) << 2) < ((int64_t)1 << 31));

        uint32_t scaled_diff16   = AOMMIN((avg_err_fp10) / AOMMAX((tf_decay_factor_fp16 >> 10), 1), 7 * 16);
        uint16_t adjusted_weight = (uint16_t)((expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17);

        int x_offset = (subblock_idx % 2) * block_width / 2;
        int y_offset = (subblock_idx / 2) * block_height / 2;

        unsigned int i = 0;
        do {
            unsigned int j = 0;
            do {
                const int k = (i + y_offset) * y_pre_stride + x_offset + j;

                uint16x8_t count_lo = vld1q_u16(y_count + k);
                count_lo            = vaddq_u16(count_lo, vdupq_n_u16(adjusted_weight));
                vst1q_u16(y_count + k, count_lo);

                uint16x8_t pre    = vmovl_u8(vld1_u8(y_pre + k));
                uint32x4_t accum0 = vld1q_u32(y_accum + k + 0);
                uint32x4_t accum1 = vld1q_u32(y_accum + k + 4);

                accum0 = vmlal_n_u16(accum0, vget_low_u16(pre), adjusted_weight);
                accum1 = vmlal_n_u16(accum1, vget_high_u16(pre), adjusted_weight);
                vst1q_u32(y_accum + k + 0, accum0);
                vst1q_u32(y_accum + k + 4, accum1);

                j += 8;
            } while (j < block_width / 2);
        } while (++i < block_height / 2);
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_neon(
    MeContext* me_ctx, const uint8_t* y_pre, int y_pre_stride, const uint8_t* u_pre, const uint8_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count) {
    svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_neon(
        me_ctx, y_pre, y_pre_stride, block_width, block_height, y_accum, y_count, me_ctx->tf_decay_factor_fp16[C_Y]);

    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_neon(me_ctx,
                                                                             u_pre,
                                                                             uv_pre_stride,
                                                                             block_width >> ss_x,
                                                                             block_height >> ss_y,
                                                                             u_accum,
                                                                             u_count,
                                                                             me_ctx->tf_decay_factor_fp16[C_U]);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_neon(me_ctx,
                                                                             v_pre,
                                                                             uv_pre_stride,
                                                                             block_width >> ss_x,
                                                                             block_height >> ss_y,
                                                                             v_accum,
                                                                             v_count,
                                                                             me_ctx->tf_decay_factor_fp16[C_V]);
    }
}

static void svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_hbd_neon(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, unsigned int block_width, unsigned int block_height,
    uint32_t* y_accum, uint16_t* y_count, const uint32_t tf_decay_factor_fp16) {
    // Decay factors for non-local mean approach.
    // Larger noise -> larger filtering weight.
    int32_t idx_32x32 = me_ctx->tf_block_col + me_ctx->tf_block_row * 2;

    uint32_t block_error_fp8[4];

    if (me_ctx->tf_32x32_block_split_flag[idx_32x32]) {
        uint64x2_t b0  = vld1q_u64(&me_ctx->tf_16x16_block_error[idx_32x32 * 4]);
        uint64x2_t b1  = vld1q_u64(&me_ctx->tf_16x16_block_error[idx_32x32 * 4 + 2]);
        uint32x4_t b01 = vcombine_u32(vshrn_n_u64(b0, 2), vshrn_n_u64(b1, 2));
        vst1q_u32(block_error_fp8, b01);
    } else {
        vst1q_u32(block_error_fp8, vdupq_n_u32((uint32_t)(me_ctx->tf_32x32_block_error[idx_32x32] >> 4)));
    }

    // Calculation for every quarter.
    for (int subblock_idx = 0; subblock_idx < 4; subblock_idx++) {
        uint32_t avg_err_fp10 = (block_error_fp8[subblock_idx]);
        FP_ASSERT((((int64_t)block_error_fp8[subblock_idx])) < ((int64_t)1 << 31));

        uint32_t scaled_diff16   = AOMMIN((avg_err_fp10) / AOMMAX((tf_decay_factor_fp16 >> 10), 1), 7 * 16);
        uint16_t adjusted_weight = (uint16_t)((expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE) >> 17);

        int x_offset = (subblock_idx % 2) * block_width / 2;
        int y_offset = (subblock_idx / 2) * block_height / 2;

        unsigned int i = 0;
        do {
            unsigned int j = 0;
            do {
                const int k = (i + y_offset) * y_pre_stride + x_offset + j;

                uint16x8_t count_lo = vld1q_u16(y_count + k);
                count_lo            = vaddq_u16(count_lo, vdupq_n_u16(adjusted_weight));
                vst1q_u16(y_count + k, count_lo);

                uint16x8_t pre    = vld1q_u16(y_pre + k);
                uint32x4_t accum0 = vld1q_u32(y_accum + k + 0);
                uint32x4_t accum1 = vld1q_u32(y_accum + k + 4);

                accum0 = vmlal_n_u16(accum0, vget_low_u16(pre), adjusted_weight);
                accum1 = vmlal_n_u16(accum1, vget_high_u16(pre), adjusted_weight);
                vst1q_u32(y_accum + k + 0, accum0);
                vst1q_u32(y_accum + k + 4, accum1);

                j += 8;
            } while (j < block_width / 2);
        } while (++i < block_height / 2);
    }
}

void svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_neon(
    MeContext* me_ctx, const uint16_t* y_pre, int y_pre_stride, const uint16_t* u_pre, const uint16_t* v_pre,
    int uv_pre_stride, unsigned int block_width, unsigned int block_height, int ss_x, int ss_y, uint32_t* y_accum,
    uint16_t* y_count, uint32_t* u_accum, uint16_t* u_count, uint32_t* v_accum, uint16_t* v_count,
    uint32_t encoder_bit_depth) {
    (void)encoder_bit_depth;

    svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_hbd_neon(
        me_ctx, y_pre, y_pre_stride, block_width, block_height, y_accum, y_count, me_ctx->tf_decay_factor_fp16[C_Y]);

    if (me_ctx->tf_chroma) {
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_hbd_neon(me_ctx,
                                                                                 u_pre,
                                                                                 uv_pre_stride,
                                                                                 block_width >> ss_x,
                                                                                 block_height >> ss_y,
                                                                                 u_accum,
                                                                                 u_count,
                                                                                 me_ctx->tf_decay_factor_fp16[C_U]);

        svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_hbd_neon(me_ctx,
                                                                                 v_pre,
                                                                                 uv_pre_stride,
                                                                                 block_width >> ss_x,
                                                                                 block_height >> ss_y,
                                                                                 v_accum,
                                                                                 v_count,
                                                                                 me_ctx->tf_decay_factor_fp16[C_V]);
    }
}
