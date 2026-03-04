/*
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */
#include <stdlib.h>

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "random.h"
#include "svt_time.h"
#include "util.h"
#include "utility.h"

using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

namespace {

using SatdFunc = int (*)(const TranLow *coeffs, int length);
using SatdTestParam = std::tuple<int, SatdFunc>;

class SatdTest : public ::testing::TestWithParam<SatdTestParam> {
  protected:
    SatdTest() {
        satd_size_ = TEST_GET_PARAM(0);
        satd_func_test_ = TEST_GET_PARAM(1);
    }

    void SetUp() override {
        src_ = reinterpret_cast<TranLow *>(
            svt_aom_memalign(32, sizeof(*src_) * satd_size_));
        ASSERT_NE(src_, nullptr);
    }

    void TearDown() override {
        svt_aom_free(src_);
    }

    void FillConstant(const TranLow val) {
        for (int i = 0; i < satd_size_; ++i)
            src_[i] = val;
    }

    void FillRandom() {
        SVTRandom rnd(-32640, 32640);
        for (int i = 0; i < satd_size_; ++i) {
            src_[i] = static_cast<int16_t>(rnd.random());
        }
    }

    void Check(int expected) {
        int total = satd_func_test_(src_, satd_size_);
        EXPECT_EQ(expected, total);
    }

    void RunComparison() {
        int total_ref = svt_aom_satd_c(src_, satd_size_);
        int total_simd = satd_func_test_(src_, satd_size_);

        EXPECT_EQ(total_ref, total_simd);
    }

    void RunSpeedTest() {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const int numIter = 500000;
        printf("size = %d number of iteration is %d \n", satd_size_, numIter);

        int total_ref;
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < numIter; i++) {
            total_ref = svt_aom_satd_c(src_, satd_size_);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        int total_simd;
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < numIter; i++) {
            total_simd = satd_func_test_(src_, satd_size_);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("c_time = %f \t o_time = %f \t Gain = %4.2f \n",
               time_c,
               time_o,
               (static_cast<float>(time_c) / static_cast<float>(time_o)));

        EXPECT_EQ(total_ref, total_simd) << "Output mismatch \n";
    }

    int satd_size_;

  private:
    TranLow *src_;
    SatdFunc satd_func_test_;
};

TEST_P(SatdTest, MinValue) {
    const int kMin = -524287;
    const int expected = -kMin * satd_size_;
    FillConstant(kMin);
    Check(expected);
}

TEST_P(SatdTest, MaxValue) {
    const int kMax = 524287;
    const int expected = kMax * satd_size_;
    FillConstant(kMax);
    Check(expected);
}

TEST_P(SatdTest, Match) {
    FillRandom();
    RunComparison();
}

TEST_P(SatdTest, DISABLED_Speed) {
    FillRandom();
    RunSpeedTest();
}
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(SatdTest);

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, SatdTest,
    ::testing::Combine(::testing::Values(16, 64, 256, 1024),
                       ::testing::Values(svt_aom_satd_avx2)));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, SatdTest,
    ::testing::Combine(::testing::Values(16, 64, 256, 1024),
                       ::testing::Values(svt_aom_satd_neon)));
#endif  // ARCH_AARCH64
}  // namespace
