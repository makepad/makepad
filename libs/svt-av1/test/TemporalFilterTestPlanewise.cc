/*
 * Copyright(c) 2020 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#include "aom_dsp_rtcd.h"
#include "gtest/gtest.h"
#include "temporal_filtering.h"
#include "util.h"
#include "random.h"
#include "utility.h"
#include "unit_test_utility.h"
#include "utility.h"

#define MAX_STRIDE 256

using svt_av1_test_tool::SVTRandom;

/** setup_test_env is implemented in test/TestEnv.c */
extern "C" void setup_test_env();

typedef void (*TemporalFilterFunc)(
    MeContext *me_ctx, const uint8_t *y_src, int y_src_stride,
    const uint8_t *y_pre, int y_pre_stride, const uint8_t *u_src,
    const uint8_t *v_src, int uv_src_stride, const uint8_t *u_pre,
    const uint8_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum,
    uint16_t *y_count, uint32_t *u_accum, uint16_t *u_count, uint32_t *v_accum,
    uint16_t *v_count);

typedef void (*TemporalFilterZZFunc)(
    MeContext *me_ctx, const uint8_t *y_pre, int y_pre_stride,
    const uint8_t *u_pre, const uint8_t *v_pre, int uv_pre_stride,
    unsigned int block_width, unsigned int block_height, int ss_x, int ss_y,
    uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum, uint16_t *u_count,
    uint32_t *v_accum, uint16_t *v_count);

static void TemporalFilterFillMeContexts(MeContext *cnt1, MeContext *cnt2) {
    // Prepare two MeContext to cover all paths in code.
    cnt1->tf_32x32_block_error[0] = 0;
    cnt1->tf_32x32_block_error[1] = 156756;
    cnt1->tf_32x32_block_error[2] = 2;
    cnt1->tf_32x32_block_error[3] = 0;
    cnt1->tf_block_col = 0;  // or 1
    cnt1->tf_block_row = 0;
    cnt1->tf_32x32_block_split_flag[0] = 1;
    cnt1->tf_32x32_block_split_flag[1] = 0;
    cnt1->tf_32x32_block_split_flag[2] = 1;
    cnt1->tf_32x32_block_split_flag[3] = 0;
    cnt1->tf_16x16_block_error[0] = 29087;
    cnt1->tf_16x16_block_error[1] = 28482;
    cnt1->tf_16x16_block_error[2] = 44508;
    cnt1->tf_16x16_block_error[3] = 42356;
    cnt1->tf_16x16_block_error[4] = 35319;
    cnt1->tf_16x16_block_error[5] = 37670;
    cnt1->tf_16x16_block_error[6] = 38551;
    cnt1->tf_16x16_block_error[7] = 39320;
    cnt1->tf_16x16_block_error[8] = 28360;
    cnt1->tf_16x16_block_error[9] = 32753;
    cnt1->tf_16x16_block_error[10] = 38335;
    cnt1->tf_16x16_block_error[11] = 50679;
    cnt1->tf_16x16_block_error[12] = 44700;
    cnt1->tf_16x16_block_error[13] = 49620;
    cnt1->tf_16x16_block_error[14] = 47070;
    cnt1->tf_16x16_block_error[15] = 41171;
    cnt1->tf_32x32_block_error[0] = 156756;
    cnt1->tf_32x32_block_error[1] = 168763;
    cnt1->tf_32x32_block_error[2] = 143351;
    cnt1->tf_32x32_block_error[3] = 189005;
    cnt1->tf_16x16_mv_x[0] = 160;
    cnt1->tf_16x16_mv_x[1] = 13;
    cnt1->tf_16x16_mv_x[2] = 13;
    cnt1->tf_16x16_mv_x[3] = 13;
    cnt1->tf_16x16_mv_x[4] = 44;
    cnt1->tf_16x16_mv_x[5] = 12;
    cnt1->tf_16x16_mv_x[6] = 1;
    cnt1->tf_16x16_mv_x[7] = 13;
    cnt1->tf_16x16_mv_x[8] = 12;
    cnt1->tf_16x16_mv_x[9] = 52;
    cnt1->tf_16x16_mv_x[10] = 11;
    cnt1->tf_16x16_mv_x[11] = 52;
    cnt1->tf_16x16_mv_x[12] = -20;
    cnt1->tf_16x16_mv_x[13] = 13;
    cnt1->tf_16x16_mv_x[14] = 11;
    cnt1->tf_16x16_mv_x[15] = -4;
    cnt1->tf_16x16_mv_y[0] = -390;
    cnt1->tf_16x16_mv_y[1] = -35;
    cnt1->tf_16x16_mv_y[2] = -38;
    cnt1->tf_16x16_mv_y[3] = -60;
    cnt1->tf_16x16_mv_y[4] = -33;
    cnt1->tf_16x16_mv_y[5] = -19;
    cnt1->tf_16x16_mv_y[6] = -35;
    cnt1->tf_16x16_mv_y[7] = -34;
    cnt1->tf_16x16_mv_y[8] = -39;
    cnt1->tf_16x16_mv_y[9] = -19;
    cnt1->tf_16x16_mv_y[10] = -39;
    cnt1->tf_16x16_mv_y[11] = 21;
    cnt1->tf_16x16_mv_y[12] = -52;
    cnt1->tf_16x16_mv_y[13] = -31;
    cnt1->tf_16x16_mv_y[14] = -12;
    cnt1->tf_16x16_mv_y[15] = -36;
    cnt1->tf_32x32_mv_x[0] = -196;
    cnt1->tf_32x32_mv_x[1] = -188;
    cnt1->tf_32x32_mv_x[2] = -228;
    cnt1->tf_32x32_mv_x[3] = -220;
    cnt1->tf_32x32_mv_y[0] = -436;
    cnt1->tf_32x32_mv_y[1] = -436;
    cnt1->tf_32x32_mv_y[2] = -436;
    cnt1->tf_32x32_mv_y[3] = -420;
    cnt1->tf_mv_dist_th = 1080;
    cnt1->tf_chroma = 1;
    *cnt2 = *cnt1;
    cnt2->tf_block_col = 1;

    cnt1->tf_decay_factor[0] = 20000;
    cnt2->tf_decay_factor[0] = 15000;
    cnt1->tf_decay_factor[1] = 20000;
    cnt2->tf_decay_factor[1] = 15000;
    cnt1->tf_decay_factor[2] = 20000;
    cnt2->tf_decay_factor[2] = 15000;
}

template <typename FuncType, bool zz>
class TemporalFilterTestPlanewiseMediumBase
    : public ::testing::TestWithParam<FuncType> {
  public:
    TemporalFilterTestPlanewiseMediumBase() : rnd_(0, 255) {
    }
    ~TemporalFilterTestPlanewiseMediumBase() {
    }

    void SetUp() {
        setup_test_env();

        for (int color_channel = 0; color_channel < COLOR_CHANNELS;
             color_channel++) {
            if (!zz) {
                src_ptr[color_channel] = reinterpret_cast<uint8_t *>(
                    svt_aom_memalign(8, MAX_STRIDE * MAX_STRIDE));
            }
            pred_ptr[color_channel] = reinterpret_cast<uint8_t *>(
                svt_aom_memalign(8, MAX_STRIDE * MAX_STRIDE));

            accum_ref_ptr[color_channel] =
                reinterpret_cast<uint32_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)));
            count_ref_ptr[color_channel] =
                reinterpret_cast<uint16_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)));
            accum_tst_ptr[color_channel] =
                reinterpret_cast<uint32_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)));
            count_tst_ptr[color_channel] =
                reinterpret_cast<uint16_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)));

            memset(accum_ref_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t));
            memset(count_ref_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t));
            memset(accum_tst_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t));
            memset(count_tst_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t));

            stride[color_channel] = MAX_STRIDE;
            stride_pred[color_channel] = MAX_STRIDE;

            tf_decay_factor[color_channel] = 1;
        }
    }

    void TearDown() {
        for (int color_channel = 0; color_channel < COLOR_CHANNELS;
             color_channel++) {
            if (!zz) {
                svt_aom_free(src_ptr[color_channel]);
            }
            svt_aom_free(pred_ptr[color_channel]);

            svt_aom_free(accum_ref_ptr[color_channel]);
            svt_aom_free(count_ref_ptr[color_channel]);
            svt_aom_free(accum_tst_ptr[color_channel]);
            svt_aom_free(count_tst_ptr[color_channel]);
        }
    }
    void GenRandomData(int width, int height, int strd, int stride2) {
        int mode = rnd_.Rand8() < 128;

        float range = 100;
        if (rnd_.Rand8() < 128) {
            range = 100000.0f;
        }
        tf_decay_factor[0] =
            (float)fclamp((rnd_.random_float() * range), 1.0, 32760);
        tf_decay_factor[1] =
            (float)fclamp((rnd_.random_float() * range), 1.0, 32760);
        tf_decay_factor[2] =
            (float)fclamp((rnd_.random_float() * range), 1.0, 32760);

        for (int ii = 0; ii < height; ii++) {
            for (int jj = 0; jj < width; jj++) {
                if (!zz) {
                    src_ptr[C_Y][ii * strd + jj] = rnd_.Rand8();
                    src_ptr[C_U][ii * strd + jj] = rnd_.Rand8();
                    src_ptr[C_V][ii * strd + jj] = rnd_.Rand8();
                }

                if (mode && !zz) {
                    int val1 =
                        -16 + src_ptr[C_Y][ii * strd + jj] + rnd_.Rand8() % 32;
                    int val2 =
                        -16 + src_ptr[C_U][ii * strd + jj] + rnd_.Rand8() % 32;
                    int val3 =
                        -16 + src_ptr[C_V][ii * strd + jj] + rnd_.Rand8() % 32;
                    pred_ptr[C_Y][ii * stride2 + jj] = MAX(0, val1);
                    pred_ptr[C_U][ii * stride2 + jj] = MAX(0, val2);
                    pred_ptr[C_V][ii * stride2 + jj] = MAX(0, val3);
                } else {
                    pred_ptr[C_Y][ii * stride2 + jj] = rnd_.Rand8();
                    pred_ptr[C_U][ii * stride2 + jj] = rnd_.Rand8();
                    pred_ptr[C_V][ii * stride2 + jj] = rnd_.Rand8();
                }
            }
        }
    }

    virtual void RunTest(int run_times) = 0;

  protected:
    SVTRandom rnd_;
    FuncType tst_func_;
    uint8_t *src_ptr[COLOR_CHANNELS];
    uint8_t *pred_ptr[COLOR_CHANNELS];
    uint32_t *accum_ref_ptr[COLOR_CHANNELS];
    uint16_t *count_ref_ptr[COLOR_CHANNELS];

    uint32_t *accum_tst_ptr[COLOR_CHANNELS];
    uint16_t *count_tst_ptr[COLOR_CHANNELS];

    uint32_t stride[COLOR_CHANNELS];
    uint32_t stride_pred[COLOR_CHANNELS];
    float tf_decay_factor[COLOR_CHANNELS];
};

class TemporalFilterTestPlanewiseMedium
    : public TemporalFilterTestPlanewiseMediumBase<TemporalFilterFunc, false> {
  public:
    TemporalFilterTestPlanewiseMedium() {
        tst_func_ = GetParam();
    }

    void RunTest(int run_times) {
        const int width = 32;
        const int height = 32;
        MeContext context1, context2, *me_ctx;
        TemporalFilterFillMeContexts(&context1, &context2);

        if (run_times <= 100) {
            for (int j = 0; j < run_times; j++) {
                GenRandomData(width, height, MAX_STRIDE, MAX_STRIDE);
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];

                svt_av1_apply_temporal_filter_planewise_medium_c(
                    me_ctx,
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
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V]);

                tst_func_(me_ctx,
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
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V]);

                for (int color_channel = 0; color_channel < COLOR_CHANNELS;
                     color_channel++) {
                    EXPECT_EQ(
                        memcmp(accum_ref_ptr[color_channel],
                               accum_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)),
                        0);

                    EXPECT_EQ(
                        memcmp(count_ref_ptr[color_channel],
                               count_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)),
                        0);
                }
            }
        } else {
            uint64_t ref_timer_seconds, ref_timer_useconds;
            uint64_t middle_timer_seconds, middle_timer_useconds;
            uint64_t test_timer_seconds, test_timer_useconds;
            double ref_time, tst_time;

            svt_av1_get_time(&ref_timer_seconds, &ref_timer_useconds);
            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_temporal_filter_planewise_medium_c(
                    me_ctx,
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
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V]);
            }
            svt_av1_get_time(&middle_timer_seconds, &middle_timer_useconds);

            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                tst_func_(me_ctx,
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
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V]);
            }
            svt_av1_get_time(&test_timer_seconds, &test_timer_useconds);

            ref_time =
                svt_av1_compute_overall_elapsed_time_ms(ref_timer_seconds,
                                                        ref_timer_useconds,
                                                        middle_timer_seconds,
                                                        middle_timer_useconds);

            tst_time =
                svt_av1_compute_overall_elapsed_time_ms(middle_timer_seconds,
                                                        middle_timer_useconds,
                                                        test_timer_seconds,
                                                        test_timer_useconds);

            printf(
                "c_time=%lf \t simd_time=%lf \t "
                "gain=%lf\t width=%d\t height=%d \n",
                ref_time / 1000,
                tst_time / 1000,
                ref_time / tst_time,
                width,
                height);
        }
    }
};

TEST_P(TemporalFilterTestPlanewiseMedium, OperationCheck) {
    RunTest(100);
}

TEST_P(TemporalFilterTestPlanewiseMedium, DISABLED_Speed) {
    RunTest(100000);
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterTestPlanewiseMedium,
    ::testing::Values(svt_av1_apply_temporal_filter_planewise_medium_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterTestPlanewiseMedium,
    ::testing::Values(svt_av1_apply_temporal_filter_planewise_medium_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterTestPlanewiseMedium,
    ::testing::Values(svt_av1_apply_temporal_filter_planewise_medium_neon));

#endif  // ARCH_AARCH64

class TemporalFilterZZTestPlanewiseMedium
    : public TemporalFilterTestPlanewiseMediumBase<TemporalFilterZZFunc, true> {
  public:
    TemporalFilterZZTestPlanewiseMedium() {
        tst_func_ = GetParam();
    }

    void RunTest(int run_times) {
        const int width = 32;
        const int height = 32;
        MeContext context1, context2, *me_ctx;
        TemporalFilterFillMeContexts(&context1, &context2);

        if (run_times <= 100) {
            for (int j = 0; j < run_times; j++) {
                GenRandomData(width, height, MAX_STRIDE, MAX_STRIDE);
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];

                svt_av1_apply_zz_based_temporal_filter_planewise_medium_c(
                    me_ctx,
                    pred_ptr[C_Y],
                    stride_pred[C_Y],
                    pred_ptr[C_U],
                    pred_ptr[C_V],
                    stride_pred[C_U],
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V]);

                tst_func_(me_ctx,
                          pred_ptr[C_Y],
                          stride_pred[C_Y],
                          pred_ptr[C_U],
                          pred_ptr[C_V],
                          stride_pred[C_U],
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V]);

                for (int color_channel = 0; color_channel < COLOR_CHANNELS;
                     color_channel++) {
                    EXPECT_EQ(
                        memcmp(accum_ref_ptr[color_channel],
                               accum_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)),
                        0);

                    EXPECT_EQ(
                        memcmp(count_ref_ptr[color_channel],
                               count_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)),
                        0);
                }
            }
        } else {
            uint64_t ref_timer_seconds, ref_timer_useconds;
            uint64_t middle_timer_seconds, middle_timer_useconds;
            uint64_t test_timer_seconds, test_timer_useconds;
            double ref_time, tst_time;

            svt_av1_get_time(&ref_timer_seconds, &ref_timer_useconds);
            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_zz_based_temporal_filter_planewise_medium_c(
                    me_ctx,
                    pred_ptr[C_Y],
                    stride_pred[C_Y],
                    pred_ptr[C_U],
                    pred_ptr[C_V],
                    stride_pred[C_U],
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V]);
            }
            svt_av1_get_time(&middle_timer_seconds, &middle_timer_useconds);

            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                tst_func_(me_ctx,
                          pred_ptr[C_Y],
                          stride_pred[C_Y],
                          pred_ptr[C_U],
                          pred_ptr[C_V],
                          stride_pred[C_U],
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V]);
            }
            svt_av1_get_time(&test_timer_seconds, &test_timer_useconds);

            ref_time =
                svt_av1_compute_overall_elapsed_time_ms(ref_timer_seconds,
                                                        ref_timer_useconds,
                                                        middle_timer_seconds,
                                                        middle_timer_useconds);

            tst_time =
                svt_av1_compute_overall_elapsed_time_ms(middle_timer_seconds,
                                                        middle_timer_useconds,
                                                        test_timer_seconds,
                                                        test_timer_useconds);

            printf(
                "c_time=%lf \t simd_time=%lf \t "
                "gain=%lf\t width=%d\t height=%d \n",
                ref_time / 1000,
                tst_time / 1000,
                ref_time / tst_time,
                width,
                height);
        }
    }
};

TEST_P(TemporalFilterZZTestPlanewiseMedium, OperationCheck) {
    RunTest(100);
}

TEST_P(TemporalFilterZZTestPlanewiseMedium, DISABLED_Speed) {
    RunTest(100000);
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterZZTestPlanewiseMedium,
    ::testing::Values(
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterZZTestPlanewiseMedium,
    ::testing::Values(
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterZZTestPlanewiseMedium,
    ::testing::Values(
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_neon));

#endif  // ARCH_AARCH64

typedef void (*TemporalFilterFuncHbd)(
    MeContext *me_ctx, const uint16_t *y_src, int y_src_stride,
    const uint16_t *y_pre, int y_pre_stride, const uint16_t *u_src,
    const uint16_t *v_src, int uv_src_stride, const uint16_t *u_pre,
    const uint16_t *v_pre, int uv_pre_stride, unsigned int block_width,
    unsigned int block_height, int ss_x, int ss_y, uint32_t *y_accum,
    uint16_t *y_count, uint32_t *u_accum, uint16_t *u_count, uint32_t *v_accum,
    uint16_t *v_count, uint32_t encoder_bit_depth);

typedef void (*TemporalFilterZZFuncHbd)(
    MeContext *me_ctx, const uint16_t *y_pre, int y_pre_stride,
    const uint16_t *u_pre, const uint16_t *v_pre, int uv_pre_stride,
    unsigned int block_width, unsigned int block_height, int ss_x, int ss_y,
    uint32_t *y_accum, uint16_t *y_count, uint32_t *u_accum, uint16_t *u_count,
    uint32_t *v_accum, uint16_t *v_count, uint32_t encoder_bit_depth);

template <typename FuncType, bool zz>
class TemporalFilterTestPlanewiseMediumHbdBase
    : public ::testing::TestWithParam<FuncType> {
  public:
    TemporalFilterTestPlanewiseMediumHbdBase() : rnd_(0, (1 << 10) - 1) {};
    ~TemporalFilterTestPlanewiseMediumHbdBase() {
    }

    void SetUp() {
        setup_test_env();

        for (int color_channel = 0; color_channel < COLOR_CHANNELS;
             color_channel++) {
            if (!zz) {
                src_ptr[color_channel] = reinterpret_cast<uint16_t *>(
                    svt_aom_memalign(8, MAX_STRIDE * MAX_STRIDE));
            }
            pred_ptr[color_channel] = reinterpret_cast<uint16_t *>(
                svt_aom_memalign(8, MAX_STRIDE * MAX_STRIDE));

            accum_ref_ptr[color_channel] =
                reinterpret_cast<uint32_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)));
            count_ref_ptr[color_channel] =
                reinterpret_cast<uint16_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)));
            accum_tst_ptr[color_channel] =
                reinterpret_cast<uint32_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)));
            count_tst_ptr[color_channel] =
                reinterpret_cast<uint16_t *>(svt_aom_memalign(
                    8, MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)));

            memset(accum_ref_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t));
            memset(count_ref_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t));
            memset(accum_tst_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t));
            memset(count_tst_ptr[color_channel],
                   0,
                   MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t));

            stride[color_channel] = MAX_STRIDE;
            stride_pred[color_channel] = MAX_STRIDE;
            tf_decay_factor[color_channel] = 1;
        }
    }

    void TearDown() {
        for (int color_channel = 0; color_channel < COLOR_CHANNELS;
             color_channel++) {
            if (!zz) {
                svt_aom_free(src_ptr[color_channel]);
            }
            svt_aom_free(pred_ptr[color_channel]);

            svt_aom_free(accum_ref_ptr[color_channel]);
            svt_aom_free(count_ref_ptr[color_channel]);
            svt_aom_free(accum_tst_ptr[color_channel]);
            svt_aom_free(count_tst_ptr[color_channel]);
        }
    }

    void GenRandomData(int width, int height, int strd, int stride2) {
        int mode = rnd_.Rand8() < 128;
        float range = 100;
        if (rnd_.Rand8() < 128) {
            range = 100000.0f;
        }
        tf_decay_factor[0] =
            (float)fclamp((rnd_.random_float() * range), 1.0, 32760);
        tf_decay_factor[1] =
            (float)fclamp((rnd_.random_float() * range), 1.0, 32760);
        tf_decay_factor[2] =
            (float)fclamp((rnd_.random_float() * range), 1.0, 32760);

        for (int ii = 0; ii < height; ii++) {
            for (int jj = 0; jj < width; jj++) {
                if (!zz) {
                    src_ptr[C_Y][ii * strd + jj] = rnd_.random();
                    src_ptr[C_U][ii * strd + jj] = rnd_.random();
                    src_ptr[C_V][ii * strd + jj] = rnd_.random();
                }

                if (mode && !zz) {
                    int val1 = -512 + src_ptr[C_Y][ii * strd + jj] +
                               rnd_.random() % 1024;
                    int val2 = -512 + src_ptr[C_U][ii * strd + jj] +
                               rnd_.random() % 1024;
                    int val3 = -512 + src_ptr[C_V][ii * strd + jj] +
                               rnd_.random() % 1024;
                    pred_ptr[C_Y][ii * stride2 + jj] = MAX(0, val1);
                    pred_ptr[C_U][ii * stride2 + jj] = MAX(0, val2);
                    pred_ptr[C_V][ii * stride2 + jj] = MAX(0, val3);
                } else {
                    pred_ptr[C_Y][ii * stride2 + jj] = rnd_.random();
                    pred_ptr[C_U][ii * stride2 + jj] = rnd_.random();
                    pred_ptr[C_V][ii * stride2 + jj] = rnd_.random();
                }
            }
        }
    }

    virtual void RunTest(int run_times) = 0;

  protected:
    SVTRandom rnd_;
    FuncType tst_func_;
    uint16_t *src_ptr[COLOR_CHANNELS];
    uint16_t *pred_ptr[COLOR_CHANNELS];
    uint32_t *accum_ref_ptr[COLOR_CHANNELS];
    uint16_t *count_ref_ptr[COLOR_CHANNELS];

    uint32_t *accum_tst_ptr[COLOR_CHANNELS];
    uint16_t *count_tst_ptr[COLOR_CHANNELS];

    uint32_t stride[COLOR_CHANNELS];
    uint32_t stride_pred[COLOR_CHANNELS];

    float tf_decay_factor[COLOR_CHANNELS];
    uint32_t encoder_bit_depth;
};

class TemporalFilterTestPlanewiseMediumHbd
    : public TemporalFilterTestPlanewiseMediumHbdBase<TemporalFilterFuncHbd,
                                                      false> {
  public:
    TemporalFilterTestPlanewiseMediumHbd() {
        tst_func_ = GetParam();
    }

    void RunTest(int run_times) {
        const int width = 32;
        const int height = 32;
        MeContext context1, context2, *me_ctx;
        TemporalFilterFillMeContexts(&context1, &context2);

        if (run_times <= 100) {
            for (int j = 0; j < run_times; j++) {
                GenRandomData(width, height, MAX_STRIDE, MAX_STRIDE);
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                encoder_bit_depth = 10;
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
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
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);

                tst_func_(me_ctx,
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
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);

                for (int color_channel = 0; color_channel < COLOR_CHANNELS;
                     color_channel++) {
                    EXPECT_EQ(
                        memcmp(accum_ref_ptr[color_channel],
                               accum_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)),
                        0);

                    EXPECT_EQ(
                        memcmp(count_ref_ptr[color_channel],
                               count_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)),
                        0);
                }
                encoder_bit_depth = 12;
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
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
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);

                tst_func_(me_ctx,
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
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);

                for (int color_channel = 0; color_channel < COLOR_CHANNELS;
                     color_channel++) {
                    EXPECT_EQ(
                        memcmp(accum_ref_ptr[color_channel],
                               accum_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)),
                        0);

                    EXPECT_EQ(
                        memcmp(count_ref_ptr[color_channel],
                               count_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)),
                        0);
                }
            }
        } else {
            uint64_t ref_timer_seconds, ref_timer_useconds;
            uint64_t middle_timer_seconds, middle_timer_useconds;
            uint64_t test_timer_seconds, test_timer_useconds;
            double ref_time, tst_time;

            encoder_bit_depth = 10;
            svt_av1_get_time(&ref_timer_seconds, &ref_timer_useconds);
            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
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
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);
            }
            svt_av1_get_time(&middle_timer_seconds, &middle_timer_useconds);

            for (int j = 0; j < run_times; j++) {
                tst_func_(me_ctx,
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
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);
            }
            svt_av1_get_time(&test_timer_seconds, &test_timer_useconds);

            ref_time =
                svt_av1_compute_overall_elapsed_time_ms(ref_timer_seconds,
                                                        ref_timer_useconds,
                                                        middle_timer_seconds,
                                                        middle_timer_useconds);

            tst_time =
                svt_av1_compute_overall_elapsed_time_ms(middle_timer_seconds,
                                                        middle_timer_useconds,
                                                        test_timer_seconds,
                                                        test_timer_useconds);

            printf(
                "c_time=%lf \t simd_time=%lf \t "
                "gain=%lf\t width=%d\t height=%d \t encoder_bit_depth=%d \n",
                ref_time / 1000,
                tst_time / 1000,
                ref_time / tst_time,
                width,
                height,
                encoder_bit_depth);

            encoder_bit_depth = 12;
            svt_av1_get_time(&ref_timer_seconds, &ref_timer_useconds);
            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
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
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);
            }
            svt_av1_get_time(&middle_timer_seconds, &middle_timer_useconds);

            for (int j = 0; j < run_times; j++) {
                tst_func_(me_ctx,
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
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);
            }
            svt_av1_get_time(&test_timer_seconds, &test_timer_useconds);

            ref_time =
                svt_av1_compute_overall_elapsed_time_ms(ref_timer_seconds,
                                                        ref_timer_useconds,
                                                        middle_timer_seconds,
                                                        middle_timer_useconds);

            tst_time =
                svt_av1_compute_overall_elapsed_time_ms(middle_timer_seconds,
                                                        middle_timer_useconds,
                                                        test_timer_seconds,
                                                        test_timer_useconds);

            printf(
                "c_time=%lf \t simd_time=%lf \t "
                "gain=%lf\t width=%d\t height=%d \t encoder_bit_depth=%d \n",
                ref_time / 1000,
                tst_time / 1000,
                ref_time / tst_time,
                width,
                height,
                encoder_bit_depth);
        }
    }
};

TEST_P(TemporalFilterTestPlanewiseMediumHbd, OperationCheck) {
    RunTest(100);
}

TEST_P(TemporalFilterTestPlanewiseMediumHbd, DISABLED_Speed) {
    RunTest(100000);
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterTestPlanewiseMediumHbd,
    ::testing::Values(
        svt_av1_apply_temporal_filter_planewise_medium_hbd_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterTestPlanewiseMediumHbd,
    ::testing::Values(svt_av1_apply_temporal_filter_planewise_medium_hbd_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterTestPlanewiseMediumHbd,
    ::testing::Values(svt_av1_apply_temporal_filter_planewise_medium_hbd_neon));

#endif  // ARCH_AARCH64

class TemporalFilterZZTestPlanewiseMediumHbd
    : public TemporalFilterTestPlanewiseMediumHbdBase<TemporalFilterZZFuncHbd,
                                                      true> {
  public:
    TemporalFilterZZTestPlanewiseMediumHbd() {
        tst_func_ = GetParam();
    }

    void RunTest(int run_times) {
        const int width = 32;
        const int height = 32;
        MeContext context1, context2, *me_ctx;
        TemporalFilterFillMeContexts(&context1, &context2);

        if (run_times <= 100) {
            for (int j = 0; j < run_times; j++) {
                GenRandomData(width, height, MAX_STRIDE, MAX_STRIDE);
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                encoder_bit_depth = 10;
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
                    pred_ptr[C_Y],
                    stride_pred[C_Y],
                    pred_ptr[C_U],
                    pred_ptr[C_V],
                    stride_pred[C_U],
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);

                tst_func_(me_ctx,
                          pred_ptr[C_Y],
                          stride_pred[C_Y],
                          pred_ptr[C_U],
                          pred_ptr[C_V],
                          stride_pred[C_U],
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);

                for (int color_channel = 0; color_channel < COLOR_CHANNELS;
                     color_channel++) {
                    EXPECT_EQ(
                        memcmp(accum_ref_ptr[color_channel],
                               accum_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)),
                        0);

                    EXPECT_EQ(
                        memcmp(count_ref_ptr[color_channel],
                               count_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)),
                        0);
                }
                encoder_bit_depth = 12;
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
                    pred_ptr[C_Y],
                    stride_pred[C_Y],
                    pred_ptr[C_U],
                    pred_ptr[C_V],
                    stride_pred[C_U],
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);

                tst_func_(me_ctx,
                          pred_ptr[C_Y],
                          stride_pred[C_Y],
                          pred_ptr[C_U],
                          pred_ptr[C_V],
                          stride_pred[C_U],
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);

                for (int color_channel = 0; color_channel < COLOR_CHANNELS;
                     color_channel++) {
                    EXPECT_EQ(
                        memcmp(accum_ref_ptr[color_channel],
                               accum_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint32_t)),
                        0);

                    EXPECT_EQ(
                        memcmp(count_ref_ptr[color_channel],
                               count_tst_ptr[color_channel],
                               MAX_STRIDE * MAX_STRIDE * sizeof(uint16_t)),
                        0);
                }
            }
        } else {
            uint64_t ref_timer_seconds, ref_timer_useconds;
            uint64_t middle_timer_seconds, middle_timer_useconds;
            uint64_t test_timer_seconds, test_timer_useconds;
            double ref_time, tst_time;

            encoder_bit_depth = 10;
            svt_av1_get_time(&ref_timer_seconds, &ref_timer_useconds);
            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
                    pred_ptr[C_Y],
                    stride_pred[C_Y],
                    pred_ptr[C_U],
                    pred_ptr[C_V],
                    stride_pred[C_U],
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);
            }
            svt_av1_get_time(&middle_timer_seconds, &middle_timer_useconds);

            for (int j = 0; j < run_times; j++) {
                tst_func_(me_ctx,
                          pred_ptr[C_Y],
                          stride_pred[C_Y],
                          pred_ptr[C_U],
                          pred_ptr[C_V],
                          stride_pred[C_U],
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);
            }
            svt_av1_get_time(&test_timer_seconds, &test_timer_useconds);

            ref_time =
                svt_av1_compute_overall_elapsed_time_ms(ref_timer_seconds,
                                                        ref_timer_useconds,
                                                        middle_timer_seconds,
                                                        middle_timer_useconds);

            tst_time =
                svt_av1_compute_overall_elapsed_time_ms(middle_timer_seconds,
                                                        middle_timer_useconds,
                                                        test_timer_seconds,
                                                        test_timer_useconds);

            printf(
                "c_time=%lf \t simd_time=%lf \t "
                "gain=%lf\t width=%d\t height=%d \t encoder_bit_depth=%d \n",
                ref_time / 1000,
                tst_time / 1000,
                ref_time / tst_time,
                width,
                height,
                encoder_bit_depth);

            encoder_bit_depth = 12;
            svt_av1_get_time(&ref_timer_seconds, &ref_timer_useconds);
            for (int j = 0; j < run_times; j++) {
                if (j % 2 == 0) {
                    me_ctx = &context1;
                } else {
                    me_ctx = &context2;
                }
                me_ctx->tf_decay_factor_fp16[C_Y] =
                    FLOAT2FP(tf_decay_factor[C_Y], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_U] =
                    FLOAT2FP(tf_decay_factor[C_U], 16, uint32_t);
                me_ctx->tf_decay_factor_fp16[C_V] =
                    FLOAT2FP(tf_decay_factor[C_V], 16, uint32_t);
                me_ctx->tf_decay_factor[C_Y] = tf_decay_factor[C_Y];
                me_ctx->tf_decay_factor[C_U] = tf_decay_factor[C_U];
                me_ctx->tf_decay_factor[C_V] = tf_decay_factor[C_V];
                svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_c(
                    me_ctx,
                    pred_ptr[C_Y],
                    stride_pred[C_Y],
                    pred_ptr[C_U],
                    pred_ptr[C_V],
                    stride_pred[C_U],
                    width,
                    height,
                    1,  // subsampling
                    1,  // subsampling
                    accum_ref_ptr[C_Y],
                    count_ref_ptr[C_Y],
                    accum_ref_ptr[C_U],
                    count_ref_ptr[C_U],
                    accum_ref_ptr[C_V],
                    count_ref_ptr[C_V],
                    encoder_bit_depth);
            }
            svt_av1_get_time(&middle_timer_seconds, &middle_timer_useconds);

            for (int j = 0; j < run_times; j++) {
                tst_func_(me_ctx,
                          pred_ptr[C_Y],
                          stride_pred[C_Y],
                          pred_ptr[C_U],
                          pred_ptr[C_V],
                          stride_pred[C_U],
                          width,
                          height,
                          1,  // subsampling
                          1,  // subsampling
                          accum_tst_ptr[C_Y],
                          count_tst_ptr[C_Y],
                          accum_tst_ptr[C_U],
                          count_tst_ptr[C_U],
                          accum_tst_ptr[C_V],
                          count_tst_ptr[C_V],
                          encoder_bit_depth);
            }
            svt_av1_get_time(&test_timer_seconds, &test_timer_useconds);

            ref_time =
                svt_av1_compute_overall_elapsed_time_ms(ref_timer_seconds,
                                                        ref_timer_useconds,
                                                        middle_timer_seconds,
                                                        middle_timer_useconds);

            tst_time =
                svt_av1_compute_overall_elapsed_time_ms(middle_timer_seconds,
                                                        middle_timer_useconds,
                                                        test_timer_seconds,
                                                        test_timer_useconds);

            printf(
                "c_time=%lf \t simd_time=%lf \t "
                "gain=%lf\t width=%d\t height=%d \t encoder_bit_depth=%d \n",
                ref_time / 1000,
                tst_time / 1000,
                ref_time / tst_time,
                width,
                height,
                encoder_bit_depth);
        }
    }
};

TEST_P(TemporalFilterZZTestPlanewiseMediumHbd, OperationCheck) {
    RunTest(100);
}

TEST_P(TemporalFilterZZTestPlanewiseMediumHbd, DISABLED_Speed) {
    RunTest(100000);
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterZZTestPlanewiseMediumHbd,
    ::testing::Values(
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterZZTestPlanewiseMediumHbd,
    ::testing::Values(
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterZZTestPlanewiseMediumHbd,
    ::testing::Values(
        svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_neon));

#endif  // ARCH_AARCH64

typedef void (*get_final_filtered_pixels_fn)(
    MeContext *me_ctx, EbByte *src_center_ptr_start,
    uint16_t **altref_buffer_highbd_start, uint32_t **accum, uint16_t **count,
    const uint32_t *stride, int blk_y_src_offset, int blk_ch_src_offset,
    uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd);

class TemporalFilterTestGetFinalFilteredPixels
    : public ::testing::TestWithParam<get_final_filtered_pixels_fn> {
  private:
    get_final_filtered_pixels_fn test_func_;
    uint32_t width;
    uint32_t height;
    uint32_t width_stride[3];
    EbByte org_src_center_ptr_start[3];
    EbByte ref_src_center_ptr_start[3];
    size_t src_center_ptr_start_size;
    uint16_t *org_altref_buffer_highbd_start[3];
    uint16_t *ref_altref_buffer_highbd_start[3];
    size_t altref_buffer_highbd_start_size;

    uint32_t *accum[3];
    uint16_t *count[3];
    MeContext *me_ctx;

  public:
    TemporalFilterTestGetFinalFilteredPixels() : test_func_(GetParam()) {
        width = BW;
        height = BH;
        src_center_ptr_start_size = height * (width + 5);
        altref_buffer_highbd_start_size = height * (width + 5);
        me_ctx = (MeContext *)malloc(sizeof(*me_ctx));

        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            width_stride[color_channel] = width + 5;
            org_src_center_ptr_start[color_channel] =
                (EbByte)malloc(src_center_ptr_start_size * sizeof(uint8_t));
            ref_src_center_ptr_start[color_channel] =
                (EbByte)malloc(src_center_ptr_start_size * sizeof(uint8_t));
            org_altref_buffer_highbd_start[color_channel] = (uint16_t *)malloc(
                altref_buffer_highbd_start_size * sizeof(uint16_t));
            ref_altref_buffer_highbd_start[color_channel] = (uint16_t *)malloc(
                altref_buffer_highbd_start_size * sizeof(uint16_t));
            memset(org_src_center_ptr_start[color_channel],
                   1,
                   src_center_ptr_start_size * sizeof(uint8_t));
            memset(ref_src_center_ptr_start[color_channel],
                   1,
                   src_center_ptr_start_size * sizeof(uint8_t));
            memset(org_altref_buffer_highbd_start[color_channel],
                   1,
                   src_center_ptr_start_size * sizeof(uint16_t));
            memset(ref_altref_buffer_highbd_start[color_channel],
                   1,
                   src_center_ptr_start_size * sizeof(uint16_t));
            accum[color_channel] =
                (uint32_t *)malloc(BW * BH * sizeof(uint32_t));
            count[color_channel] =
                (uint16_t *)malloc(BW * BH * sizeof(uint16_t));
        }
    }

    ~TemporalFilterTestGetFinalFilteredPixels() {
        free(me_ctx);
        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            free(org_src_center_ptr_start[color_channel]);
            free(ref_src_center_ptr_start[color_channel]);
            free(org_altref_buffer_highbd_start[color_channel]);
            free(ref_altref_buffer_highbd_start[color_channel]);
            free(accum[color_channel]);
            free(count[color_channel]);
        }
    }

    void SetRandData(bool is_highbd) {
        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            for (int i = 0; i < BH * BW; ++i) {
                if (is_highbd) {
                    accum[color_channel][i] = rand() % 1024;
                } else {
                    accum[color_channel][i] = rand() % 256;
                }
                uint16_t rand16 = rand() % 256;
                if (rand16 == 0) {
                    rand16 = 1;
                }
                count[color_channel][i] = rand16;  // Never 0
            }
            me_ctx->tf_decay_factor[color_channel] = 1;
        }

        *me_ctx = {};
        me_ctx->tf_chroma = rand() % 2;
    }

    void RunTest(bool is_highbd) {
        int blk_y_src_offset = 1;
        int blk_ch_src_offset = 2;
        uint16_t blk_width_ch = 48;
        uint16_t blk_height_ch = 61;

        SetRandData(is_highbd);

        svt_aom_get_final_filtered_pixels_c(me_ctx,
                                            org_src_center_ptr_start,
                                            org_altref_buffer_highbd_start,
                                            accum,
                                            count,
                                            width_stride,
                                            blk_y_src_offset,
                                            blk_ch_src_offset,
                                            blk_width_ch,
                                            blk_height_ch,
                                            is_highbd);

        test_func_(me_ctx,
                   ref_src_center_ptr_start,
                   ref_altref_buffer_highbd_start,
                   accum,
                   count,
                   width_stride,
                   blk_y_src_offset,
                   blk_ch_src_offset,
                   blk_width_ch,
                   blk_height_ch,
                   is_highbd);

        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            EXPECT_EQ(memcmp(org_src_center_ptr_start[color_channel],
                             ref_src_center_ptr_start[color_channel],
                             src_center_ptr_start_size),
                      0);

            EXPECT_EQ(memcmp(org_altref_buffer_highbd_start[color_channel],
                             ref_altref_buffer_highbd_start[color_channel],
                             altref_buffer_highbd_start_size),
                      0);
        }
    }
};

TEST_P(TemporalFilterTestGetFinalFilteredPixels, RunTest) {
    for (int i = 0; i < 100; ++i) {
        RunTest(false);
        RunTest(true);
    }
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterTestGetFinalFilteredPixels,
    ::testing::Values(svt_aom_get_final_filtered_pixels_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterTestGetFinalFilteredPixels,
    ::testing::Values(svt_aom_get_final_filtered_pixels_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterTestGetFinalFilteredPixels,
    ::testing::Values(svt_aom_get_final_filtered_pixels_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, TemporalFilterTestGetFinalFilteredPixels,
    ::testing::Values(svt_aom_get_final_filtered_pixels_sve));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

template <typename SrcType, typename FuncType>
class TemporalFilterTestApplyFilteringCentral
    : public ::testing::TestWithParam<FuncType> {
  protected:
    FuncType ref_func_;
    FuncType test_func_;
    uint32_t width;
    uint32_t height;
    SrcType *src[3];
    int src_size;

    uint32_t *ref_accum[3];
    uint32_t *test_accum[3];
    uint16_t *ref_count[3];
    uint16_t *test_count[3];
    MeContext *me_ctx;
    EbPictureBufferDesc input_picture_central;

  public:
    TemporalFilterTestApplyFilteringCentral() {
        width = BW;
        height = BH;
        src_size = height * (width + 5);
        me_ctx = (MeContext *)malloc(sizeof(*me_ctx));

        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            src[color_channel] = (SrcType *)malloc(src_size * sizeof(SrcType));
            ref_accum[color_channel] =
                (uint32_t *)malloc(BW * BH * sizeof(uint32_t));
            test_accum[color_channel] =
                (uint32_t *)malloc(BW * BH * sizeof(uint32_t));
            ref_count[color_channel] =
                (uint16_t *)malloc(BW * BH * sizeof(uint16_t));
            test_count[color_channel] =
                (uint16_t *)malloc(BW * BH * sizeof(uint16_t));
            memset(ref_accum[color_channel], 1, BW * BH * sizeof(uint32_t));
            memset(test_accum[color_channel], 1, BW * BH * sizeof(uint32_t));
            memset(ref_count[color_channel], 1, BW * BH * sizeof(uint16_t));
            memset(test_count[color_channel], 1, BW * BH * sizeof(uint16_t));
        }
    }

    ~TemporalFilterTestApplyFilteringCentral() {
        free(me_ctx);
        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            free(src[color_channel]);
            free(ref_accum[color_channel]);
            free(test_accum[color_channel]);
            free(ref_count[color_channel]);
            free(test_count[color_channel]);
        }
    }

    void SetRandData() {
        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            for (int i = 0; i < src_size; ++i) {
                src[color_channel][i] = rand();
            }
            me_ctx->tf_decay_factor[color_channel] = 1;
        }

        *me_ctx = {};
        me_ctx->tf_chroma = rand() % 2;

        input_picture_central = {};
        input_picture_central.stride_y = BW + 5;
    }

    void RunTest() {
        uint16_t blk_width_ch = 48;
        uint16_t blk_height_ch = 61;
        uint32_t ss_x = rand() % 2;
        uint32_t ss_y = rand() % 2;
        SetRandData();

        ref_func_(me_ctx,
                  &input_picture_central,
                  src,
                  ref_accum,
                  ref_count,
                  blk_width_ch,
                  blk_height_ch,
                  ss_x,
                  ss_y);
        test_func_(me_ctx,
                   &input_picture_central,
                   src,
                   test_accum,
                   test_count,
                   blk_width_ch,
                   blk_height_ch,
                   ss_x,
                   ss_y);

        for (int color_channel = 0; color_channel < 3; ++color_channel) {
            EXPECT_EQ(memcmp(ref_accum[color_channel],
                             test_accum[color_channel],
                             BW * BH * sizeof(uint32_t)),
                      0);

            EXPECT_EQ(memcmp(ref_count[color_channel],
                             test_count[color_channel],
                             BW * BH * sizeof(uint16_t)),
                      0);
        }
    }
};

typedef void (*apply_filtering_central_fn)(
    MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central,
    EbByte *src, uint32_t **accum, uint16_t **count, uint16_t blk_width,
    uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);

class TemporalFilterTestApplyFilteringCentralLbd
    : public TemporalFilterTestApplyFilteringCentral<
          uint8_t, apply_filtering_central_fn> {
  public:
    TemporalFilterTestApplyFilteringCentralLbd() {
        ref_func_ = svt_aom_apply_filtering_central_c;
        test_func_ = GetParam();
    }
};

TEST_P(TemporalFilterTestApplyFilteringCentralLbd, RunTest) {
    for (int i = 0; i < 100; ++i) {
        RunTest();
    }
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterTestApplyFilteringCentralLbd,
    ::testing::Values(svt_aom_apply_filtering_central_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterTestApplyFilteringCentralLbd,
    ::testing::Values(svt_aom_apply_filtering_central_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterTestApplyFilteringCentralLbd,
    ::testing::Values(svt_aom_apply_filtering_central_neon));
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
typedef void (*apply_filtering_central_highbd_fn)(
    MeContext *me_ctx, EbPictureBufferDesc *input_picture_ptr_central,
    uint16_t **src_16bit, uint32_t **accum, uint16_t **count,
    uint16_t blk_width, uint16_t blk_height, uint32_t ss_x, uint32_t ss_y);

class TemporalFilterTestApplyFilteringCentralHbd
    : public TemporalFilterTestApplyFilteringCentral<
          uint16_t, apply_filtering_central_highbd_fn> {
  public:
    TemporalFilterTestApplyFilteringCentralHbd() {
        ref_func_ = svt_aom_apply_filtering_central_highbd_c;
        test_func_ = GetParam();
    }
};

TEST_P(TemporalFilterTestApplyFilteringCentralHbd, RunTest) {
    for (int i = 0; i < 100; ++i) {
        RunTest();
    }
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, TemporalFilterTestApplyFilteringCentralHbd,
    ::testing::Values(svt_aom_apply_filtering_central_highbd_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, TemporalFilterTestApplyFilteringCentralHbd,
    ::testing::Values(svt_aom_apply_filtering_central_highbd_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, TemporalFilterTestApplyFilteringCentralHbd,
    ::testing::Values(svt_aom_apply_filtering_central_highbd_neon));
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

#ifdef ARCH_X86_64

int32_t estimate_noise_fp16_c_wrapper(const uint16_t *src, int width,
                                      int height, int stride, int bd) {
    UNUSED(bd);
    return svt_estimate_noise_fp16_c(
        (const uint8_t *)src, width, height, stride);
}
int32_t estimate_noise_fp16_avx2_wrapper(const uint16_t *src, int width,
                                         int height, int stride, int bd) {
    UNUSED(bd);
    return svt_estimate_noise_fp16_avx2(
        (const uint8_t *)src, width, height, stride);
}

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

int32_t estimate_noise_fp16_c_wrapper(const uint16_t *src, int width,
                                      int height, int stride, int bd) {
    UNUSED(bd);
    return svt_estimate_noise_fp16_c(
        (const uint8_t *)src, width, height, stride);
}

int32_t estimate_noise_fp16_neon_wrapper(const uint16_t *src, int width,
                                         int height, int stride, int bd) {
    UNUSED(bd);
    return svt_estimate_noise_fp16_neon(
        (const uint8_t *)src, width, height, stride);
}

#endif  // ARCH_AARCH64

typedef int32_t (*EstimateNoiseFuncFP)(const uint16_t *src, int width,
                                       int height, int stride, int bd);

typedef std::tuple<EstimateNoiseFuncFP, EstimateNoiseFuncFP, int, int, int>
    EstimateNoiseParamFP;
class EstimateNoiseTestFP
    : public ::testing::TestWithParam<EstimateNoiseParamFP> {
  public:
    EstimateNoiseTestFP() : rnd_(0, (1 << TEST_GET_PARAM(4)) - 1) {};
    ~EstimateNoiseTestFP() {
    }

    void SetUp() {
        ref_func = TEST_GET_PARAM(0);
        tst_func = TEST_GET_PARAM(1);
        width = TEST_GET_PARAM(2);
        height = TEST_GET_PARAM(3);
        encoder_bit_depth = TEST_GET_PARAM(4);
        src_ptr = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(8, 4000 * 2500 * sizeof(uint16_t)));
        GenRandomData(4000 * 2500);
    }

    void TearDown() {
        svt_aom_free(src_ptr);
    }
    void RunTest() {
        for (int i = 0; i < 10; i++) {
            stride = width + rnd_.random() % 100;
            int32_t ref_out =
                ref_func(src_ptr, width, height, stride, encoder_bit_depth);
            int32_t tst_out =
                tst_func(src_ptr, width, height, stride, encoder_bit_depth);

            EXPECT_EQ(ref_out, tst_out);
        }
    }

    void GenRandomData(int size) {
        if (encoder_bit_depth == 8) {
            for (int ii = 0; ii < size; ii++)
                src_ptr[ii] = rnd_.Rand16();
        } else
            for (int ii = 0; ii < size; ii++)
                src_ptr[ii] = rnd_.random();
    }

  private:
    EstimateNoiseFuncFP ref_func;
    EstimateNoiseFuncFP tst_func;
    SVTRandom rnd_;
    uint16_t *src_ptr;
    int width;
    int height;
    uint32_t encoder_bit_depth;
    int stride;
};

TEST_P(EstimateNoiseTestFP, fixed_point) {
    RunTest();
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
using EstimateNoiseTestFPHbd = EstimateNoiseTestFP;

TEST_P(EstimateNoiseTestFPHbd, fixed_point) {
    RunTest();
}
#endif

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    AVX2, EstimateNoiseTestFP,
    ::testing::Combine(::testing::Values(estimate_noise_fp16_c_wrapper),
                       ::testing::Values(estimate_noise_fp16_avx2_wrapper),
                       ::testing::Values(3840, 1920, 1280, 800, 640, 360, 357),
                       ::testing::Values(2160, 1080, 720, 600, 480, 240, 237),
                       ::testing::Values(8)));

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(
    AVX2, EstimateNoiseTestFPHbd,
    ::testing::Combine(::testing::Values(svt_estimate_noise_highbd_fp16_c),
                       ::testing::Values(svt_estimate_noise_highbd_fp16_avx2),
                       ::testing::Values(3840, 1920, 1280, 800, 640, 360, 357),
                       ::testing::Values(2160, 1080, 720, 600, 480, 240, 237),
                       ::testing::Values(10)));
#endif

#endif

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, EstimateNoiseTestFP,
    ::testing::Combine(::testing::Values(estimate_noise_fp16_c_wrapper),
                       ::testing::Values(estimate_noise_fp16_neon_wrapper),
                       ::testing::Values(3840, 1920, 1280, 800, 640, 360, 357),
                       ::testing::Values(2160, 1080, 720, 600, 480, 240, 237),
                       ::testing::Values(8)));

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
INSTANTIATE_TEST_SUITE_P(
    NEON, EstimateNoiseTestFPHbd,
    ::testing::Combine(::testing::Values(svt_estimate_noise_highbd_fp16_c),
                       ::testing::Values(svt_estimate_noise_highbd_fp16_neon),
                       ::testing::Values(3840, 1920, 1280, 800, 640, 360, 357),
                       ::testing::Values(2160, 1080, 720, 600, 480, 240, 237),
                       ::testing::Values(10)));
#endif

#endif

typedef double (*EstimateNoiseFuncDbl)(const uint16_t *src, int width,
                                       int height, int stride, int bd);

typedef std::tuple<EstimateNoiseFuncDbl, EstimateNoiseFuncDbl, int, int, int>
    EstimateNoiseParamDbl;
class EstimateNoiseTestDbl
    : public ::testing::TestWithParam<EstimateNoiseParamDbl> {
  public:
    EstimateNoiseTestDbl() : rnd_(0, (1 << TEST_GET_PARAM(4)) - 1) {};
    ~EstimateNoiseTestDbl() {
    }

    void SetUp() {
        ref_func = TEST_GET_PARAM(0);
        tst_func = TEST_GET_PARAM(1);
        width = TEST_GET_PARAM(2);
        height = TEST_GET_PARAM(3);
        encoder_bit_depth = TEST_GET_PARAM(4);
        src_ptr = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(8, 4000 * 2500 * sizeof(uint16_t)));
        GenRandomData(4000 * 2500);
    }

    void TearDown() {
        svt_aom_free(src_ptr);
    }
    void RunTest() {
        for (int i = 0; i < 10; i++) {
            stride = width + rnd_.random() % 100;
            double ref_out =
                ref_func(src_ptr, width, height, stride, encoder_bit_depth);
            double tst_out =
                tst_func(src_ptr, width, height, stride, encoder_bit_depth);

            EXPECT_EQ(ref_out, tst_out);
        }
    }

    void GenRandomData(int size) {
        if (encoder_bit_depth == 8) {
            for (int ii = 0; ii < size; ii++)
                src_ptr[ii] = rnd_.Rand16();
        } else
            for (int ii = 0; ii < size; ii++)
                src_ptr[ii] = rnd_.random();
    }

  private:
    EstimateNoiseFuncDbl ref_func;
    EstimateNoiseFuncDbl tst_func;
    SVTRandom rnd_;
    uint16_t *src_ptr;
    int width;
    int height;
    uint32_t encoder_bit_depth;
    int stride;
};
