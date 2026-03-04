/*
 * Copyright(c) 2019 Netflix, Inc.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file ResidualTest.cc
 *
 * @brief Unit test for Residual related functions:
 * - residual_kernel_avx2
 * - svt_residual_kernel16bit_sse2_intrin
 * - residual_kernel_sub_sampled{w}x{h}_sse_intrin
 *
 * @author Cidana-Ivy, Cidana-Wenyao
 *
 ******************************************************************************/
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <limits.h>
#include <new>

#include "definitions.h"
#include "pic_operators.h"
#include "enc_intra_prediction.h"
#include "utility.h"
#include "unit_test_utility.h"
#include "aom_dsp_rtcd.h"
#include "random.h"
#include "util.h"

using svt_av1_test_tool::SVTRandom;  // to generate the random

namespace {
typedef enum { VAL_MIN, VAL_MAX, VAL_RANDOM } TestPattern;
TestPattern TEST_PATTERNS[] = {VAL_MIN, VAL_MAX, VAL_RANDOM};

typedef void (*svt_residual_kernel8bit_func)(
    uint8_t *input, uint32_t input_stride, uint8_t *pred, uint32_t pred_stride,
    int16_t *residual, uint32_t residual_stride, uint32_t area_width,
    uint32_t area_height);

typedef std::tuple<uint32_t, uint32_t> AreaSize;
AreaSize TEST_AREA_SIZES[] = {
    AreaSize(4, 4),    AreaSize(4, 8),   AreaSize(8, 4),   AreaSize(8, 8),
    AreaSize(16, 16),  AreaSize(4, 16),  AreaSize(16, 4),  AreaSize(16, 8),
    AreaSize(8, 16),   AreaSize(32, 32), AreaSize(32, 8),  AreaSize(16, 32),
    AreaSize(8, 32),   AreaSize(32, 16), AreaSize(16, 64), AreaSize(64, 16),
    AreaSize(64, 64),  AreaSize(64, 32), AreaSize(32, 64), AreaSize(128, 128),
    AreaSize(64, 128), AreaSize(128, 64)};

typedef std::tuple<AreaSize, TestPattern, svt_residual_kernel8bit_func>
    TestResidual8BitParam;

/**
 * @Brief Base class for Residual related test, it will create input buffer,
 * prediction buffer and two residual buffers to store the results.
 */
class ResidualKernel8BitTest
    : public ::testing::Test,
      public ::testing::WithParamInterface<TestResidual8BitParam> {
  public:
    ResidualKernel8BitTest() {
        area_width_ = std::get<0>(TEST_GET_PARAM(0));
        area_height_ = std::get<1>(TEST_GET_PARAM(0));
        test_pattern_ = TEST_GET_PARAM(1);
        test_func_ = TEST_GET_PARAM(2);
        input_stride_ = pred_stride_ = residual_stride_ = MAX_SB_SIZE;
        test_size_ = MAX_SB_SQUARE;
    }

    void SetUp() override {
        input_ = (uint8_t *)svt_aom_memalign(8, test_size_);
        pred_ = (uint8_t *)svt_aom_memalign(8, test_size_);
        residual1_ = (int16_t *)svt_aom_memalign(16, 2 * test_size_);
        residual2_ = (int16_t *)svt_aom_memalign(16, 2 * test_size_);
    }

    void TearDown() override {
        if (input_)
            svt_aom_free(input_);
        if (pred_)
            svt_aom_free(pred_);
        if (residual1_)
            svt_aom_free(residual1_);
        if (residual2_)
            svt_aom_free(residual2_);
    }

  protected:
    virtual void prepare_data() {
        const uint8_t mask = (1 << 8) - 1;
        switch (test_pattern_) {
        case VAL_MIN: {
            memset(input_, 0, test_size_ * sizeof(input_[0]));
            memset(pred_, 0, test_size_ * sizeof(pred_[0]));
            break;
        }
        case VAL_MAX: {
            memset(input_, mask, test_size_ * sizeof(input_[0]));
            memset(pred_, 0, test_size_ * sizeof(pred_[0]));
            break;
        }
        case VAL_RANDOM: {
            svt_buf_random_u8(input_, test_size_);
            svt_buf_random_u8(pred_, test_size_);
            break;
        }
        default: break;
        }
    }

    void check_residuals(uint32_t width, uint32_t height) {
        int mismatched_count_ = 0;
        for (uint32_t j = 0; j < height; j++) {
            for (uint32_t k = 0; k < width; k++) {
                if (residual1_[k + j * residual_stride_] !=
                    residual2_[k + j * residual_stride_])
                    ++mismatched_count_;
            }
        }
        EXPECT_EQ(0, mismatched_count_)
            << "compare residual result error"
            << "in test area for " << mismatched_count_ << " times";
    }

    void run_test() {
        prepare_data();

        svt_residual_kernel8bit_c(input_,
                                  input_stride_,
                                  pred_,
                                  pred_stride_,
                                  residual1_,
                                  residual_stride_,
                                  area_width_,
                                  area_height_);

        svt_buf_random_s16(residual2_, test_size_);
        test_func_(input_,
                   input_stride_,
                   pred_,
                   pred_stride_,
                   residual2_,
                   residual_stride_,
                   area_width_,
                   area_height_);
        check_residuals(area_width_, area_height_);
    }

    void speed_test() {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const uint64_t num_loop = 10000000000 / (area_width_ * area_height_);

        prepare_data();

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            svt_residual_kernel8bit_c(input_,
                                      input_stride_,
                                      pred_,
                                      pred_stride_,
                                      residual1_,
                                      residual_stride_,
                                      area_width_,
                                      area_height_);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);

        svt_buf_random_s16(residual2_, test_size_);

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t j = 0; j < num_loop; j++) {
            test_func_(input_,
                       input_stride_,
                       pred_,
                       pred_stride_,
                       residual2_,
                       residual_stride_,
                       area_width_,
                       area_height_);
        }
        check_residuals(area_width_, area_height_);

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("svt_residual_kernel8bit(%3dx%3d): %6.2f\n",
               area_width_,
               area_height_,
               time_c / time_o);
    }

    TestPattern test_pattern_;
    uint32_t area_width_, area_height_;
    svt_residual_kernel8bit_func test_func_;
    uint32_t input_stride_, pred_stride_, residual_stride_;
    uint8_t *input_, *pred_;
    int16_t *residual1_, *residual2_;
    uint32_t test_size_;
};

TEST_P(ResidualKernel8BitTest, MatchTest) {
    run_test();
};

TEST_P(ResidualKernel8BitTest, DISABLED_SpeedTest) {
    speed_test();
};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, ResidualKernel8BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_AREA_SIZES),
                       ::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_residual_kernel8bit_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, ResidualKernel8BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_AREA_SIZES),
                       ::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_residual_kernel8bit_avx2)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, ResidualKernel8BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_AREA_SIZES),
                       ::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_residual_kernel8bit_avx512)));
#endif
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, ResidualKernel8BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_AREA_SIZES),
                       ::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_residual_kernel8bit_neon)));
#endif  // ARCH_AARCH64

typedef void (*svt_residual_kernel16bit_func)(
    uint16_t *input, uint32_t input_stride, uint16_t *pred,
    uint32_t pred_stride, int16_t *residual, uint32_t residual_stride,
    uint32_t area_width, uint32_t area_height);

typedef std::tuple<AreaSize, TestPattern, svt_residual_kernel16bit_func>
    TestResidual16BitParam;

class ResidualKernel16BitTest
    : public ::testing::Test,
      public ::testing::WithParamInterface<TestResidual16BitParam> {
  public:
    ResidualKernel16BitTest() {
        area_width_ = std::get<0>(TEST_GET_PARAM(0));
        area_height_ = std::get<1>(TEST_GET_PARAM(0));
        test_pattern_ = TEST_GET_PARAM(1);
        test_func_ = TEST_GET_PARAM(2);
        input_stride_ = pred_stride_ = residual_stride_ = MAX_SB_SIZE;
        test_size_ = MAX_SB_SQUARE;
    }

    void SetUp() override {
        input_ = (uint16_t *)svt_aom_memalign(16, 2 * test_size_);
        pred_ = (uint16_t *)svt_aom_memalign(16, 2 * test_size_);
        residual1_ = (int16_t *)svt_aom_memalign(16, 2 * test_size_);
        residual2_ = (int16_t *)svt_aom_memalign(16, 2 * test_size_);
    }

    void TearDown() override {
        if (input_)
            svt_aom_free(input_);
        if (pred_)
            svt_aom_free(pred_);
        if (residual1_)
            svt_aom_free(residual1_);
        if (residual2_)
            svt_aom_free(residual2_);
    }

  protected:
    void prepare_data() {
        const uint16_t mask = (1 << 16) - 1;
        switch (test_pattern_) {
        case VAL_MIN: {
            memset(input_, 0, test_size_ * sizeof(input_[0]));
            memset(pred_, 0, test_size_ * sizeof(pred_[0]));
            break;
        }
        case VAL_MAX: {
            for (uint32_t i = 0; i < test_size_; ++i)
                input_[i] = mask;
            memset(pred_, 0, test_size_ * sizeof(pred_[0]));
            break;
        }
        case VAL_RANDOM: {
            svt_buf_random_u16(input_, test_size_);
            svt_buf_random_u16(pred_, test_size_);
            break;
        }
        default: break;
        }
    }

    void check_residuals(uint32_t width, uint32_t height) {
        int mismatched_count_ = 0;
        for (uint32_t j = 0; j < height; j++) {
            for (uint32_t k = 0; k < width; k++) {
                if (residual1_[k + j * residual_stride_] !=
                    residual2_[k + j * residual_stride_])
                    ++mismatched_count_;
            }
        }
        EXPECT_EQ(0, mismatched_count_)
            << "compare residual result error"
            << "in test area for " << mismatched_count_ << " times";
    }

    void run_test() {
        prepare_data();

        test_func_(input_,
                   input_stride_,
                   pred_,
                   pred_stride_,
                   residual1_,
                   residual_stride_,
                   area_width_,
                   area_height_);
        svt_residual_kernel16bit_c(input_,
                                   input_stride_,
                                   pred_,
                                   pred_stride_,
                                   residual2_,
                                   residual_stride_,
                                   area_width_,
                                   area_height_);

        check_residuals(area_width_, area_height_);
    }

    TestPattern test_pattern_;
    uint32_t area_width_, area_height_;
    svt_residual_kernel16bit_func test_func_;
    uint32_t input_stride_, pred_stride_, residual_stride_;
    uint16_t *input_, *pred_;
    int16_t *residual1_, *residual2_;
    uint32_t test_size_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(ResidualKernel16BitTest);

TEST_P(ResidualKernel16BitTest, MatchTest) {
    run_test();
};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, ResidualKernel16BitTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::Values(svt_residual_kernel16bit_sse2_intrin)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, ResidualKernel16BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_AREA_SIZES),
                       ::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_residual_kernel16bit_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, ResidualKernel16BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_AREA_SIZES),
                       ::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_residual_kernel16bit_neon)));
#endif  // ARCH_AARCH64

}  // namespace
