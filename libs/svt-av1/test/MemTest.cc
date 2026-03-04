/*
 * Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
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
#include "common_dsp_rtcd.h"
#include "definitions.h"
#include "random.h"
#include "svt_time.h"
#include "util.h"
#include "utility.h"

using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

namespace {

typedef std::tuple<uint32_t, uint32_t, uint32_t, uint32_t> MemCopyParams;
MemCopyParams MemCopyTestParams[] = {
    // 4xh
    MemCopyParams(4, 4, 4, 4),
    MemCopyParams(4, 4, 16, 16),
    MemCopyParams(4, 4, 8, 32),
    MemCopyParams(4, 2, 64, 32),
    MemCopyParams(4, 16, 64, 32),
    // 8xh
    MemCopyParams(8, 8, 8, 8),
    MemCopyParams(8, 8, 16, 16),
    MemCopyParams(8, 8, 8, 32),
    MemCopyParams(8, 4, 64, 32),
    MemCopyParams(8, 32, 64, 32),
    // 16xh
    MemCopyParams(16, 16, 16, 16),
    MemCopyParams(16, 16, 64, 64),
    MemCopyParams(16, 16, 16, 32),
    MemCopyParams(16, 4, 64, 32),
    MemCopyParams(16, 64, 64, 128),
    // 32xh
    MemCopyParams(32, 32, 32, 32),
    MemCopyParams(32, 32, 64, 64),
    MemCopyParams(32, 32, 32, 64),
    MemCopyParams(32, 8, 64, 32),
    MemCopyParams(32, 64, 64, 128),
    // 64xh
    MemCopyParams(64, 64, 64, 64),
    MemCopyParams(64, 64, 128, 128),
    MemCopyParams(64, 64, 64, 128),
    MemCopyParams(64, 16, 128, 64),
    MemCopyParams(64, 128, 64, 128),
};

template <typename T>
using MemFunc = void (*)(T* src, uint32_t src_stride, T* dst,
                         uint32_t dst_stride, uint32_t height, uint32_t width);

template <typename T>
class MemTest : public ::testing::TestWithParam<
                    std::tuple<MemCopyParams, MemFunc<T>, MemFunc<T>>> {
  protected:
    MemTest() {
        width_ = std::get<0>(TEST_GET_PARAM(0));
        height_ = std::get<1>(TEST_GET_PARAM(0));
        src_stride_ = std::get<2>(TEST_GET_PARAM(0));
        dst_stride_ = std::get<3>(TEST_GET_PARAM(0));
        mem_func_test_c_ = TEST_GET_PARAM(1);
        mem_func_test_o_ = TEST_GET_PARAM(2);
    }

    void SetUp() override {
        ASSERT_LE(width_, src_stride_);
        ASSERT_LE(width_, dst_stride_);

        uint32_t max_w = std::max(src_stride_, dst_stride_);
        size_ = (padding_ + max_w + padding_) * (padding_ + height_ + padding_);
        src_ = reinterpret_cast<T*>(svt_aom_memalign(32, sizeof(T) * size_));
        dst_o_ = reinterpret_cast<T*>(svt_aom_memalign(32, sizeof(T) * size_));
        dst_c_ = reinterpret_cast<T*>(svt_aom_memalign(32, sizeof(T) * size_));
        ASSERT_NE(src_, nullptr);

        FillConstant(dst_o_, 0);
        FillConstant(dst_c_, 0);
        FillRandom(src_);
    }

    void TearDown() override {
        svt_aom_free(src_);
        svt_aom_free(dst_o_);
        svt_aom_free(dst_c_);
    }

    void FillConstant(T* ptr, const T val) {
        for (uint32_t i = 0; i < size_; ++i)
            ptr[i] = val;
    }

    void FillRandom(T* ptr) {
        SVTRandom rnd(0, (1 << (sizeof(T) * 8)) - 1);
        for (uint32_t i = 0; i < size_; ++i) {
            ptr[i] = static_cast<T>(rnd.random());
        }
    }

    void Check() {
        uint32_t res = memcmp(dst_o_, dst_c_, size_ * sizeof(T));
        EXPECT_EQ(res, 0u);
    }

    void RunComparison() {
        T* src = src_ + src_stride_ * padding_ + padding_;
        T* dst_o = dst_o_ + dst_stride_ * padding_ + padding_;
        T* dst_c = dst_c_ + dst_stride_ * padding_ + padding_;

        mem_func_test_c_(src, src_stride_, dst_c, dst_stride_, height_, width_);
        mem_func_test_o_(src, src_stride_, dst_o, dst_stride_, height_, width_);

        Check();
    }

    void RunSpeedTest() {
        T* src = src_ + src_stride_ * padding_ + padding_;
        T* dst_o = dst_o_ + dst_stride_ * padding_ + padding_;
        T* dst_c = dst_c_ + dst_stride_ * padding_ + padding_;

        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const int numIter = 1000000;
        printf("config = (%u,%u,%u,%u) number of iteration is %d \n",
               width_,
               height_,
               src_stride_,
               dst_stride_,
               numIter);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < numIter; i++) {
            mem_func_test_c_(src + (i & 1),
                             src_stride_,
                             dst_c + (i & 1),
                             dst_stride_,
                             height_,
                             width_);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < numIter; i++) {
            mem_func_test_o_(src + (i & 1),
                             src_stride_,
                             dst_o + (i & 1),
                             dst_stride_,
                             height_,
                             width_);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("c_time = %f \t o_time = %f \t Gain = %4.2f\n",
               time_c,
               time_o,
               (static_cast<float>(time_c) / static_cast<float>(time_o)));
    }

  protected:
    static const uint32_t padding_ = 16;
    uint32_t size_;
    T* src_;
    T* dst_o_;
    T* dst_c_;
    uint32_t src_stride_;
    uint32_t dst_stride_;
    uint32_t width_;
    uint32_t height_;
    MemFunc<T> mem_func_test_o_;
    MemFunc<T> mem_func_test_c_;
};

using MemTestLBD = MemTest<uint8_t>;
using MemTestHBD = MemTest<uint16_t>;

TEST_P(MemTestLBD, Match) {
    RunComparison();
}

TEST_P(MemTestLBD, DISABLED_Speed) {
    RunSpeedTest();
}

TEST_P(MemTestHBD, Match) {
    RunComparison();
}

TEST_P(MemTestHBD, DISABLED_Speed) {
    RunSpeedTest();
}

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(MemTestLBD);
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(MemTestHBD);

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, MemTestLBD,
    ::testing::Combine(::testing::ValuesIn(MemCopyTestParams),
                       ::testing::Values(svt_av1_copy_wxh_8bit_c),
                       ::testing::Values(svt_av1_copy_wxh_8bit_c)));
INSTANTIATE_TEST_SUITE_P(
    AVX2, MemTestHBD,
    ::testing::Combine(::testing::ValuesIn(MemCopyTestParams),
                       ::testing::Values(svt_av1_copy_wxh_16bit_c),
                       ::testing::Values(svt_av1_copy_wxh_16bit_c)));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, MemTestLBD,
    ::testing::Combine(::testing::ValuesIn(MemCopyTestParams),
                       ::testing::Values(svt_av1_copy_wxh_8bit_c),
                       ::testing::Values(svt_av1_copy_wxh_8bit_neon)));
INSTANTIATE_TEST_SUITE_P(
    NEON, MemTestHBD,
    ::testing::Combine(::testing::ValuesIn(MemCopyTestParams),
                       ::testing::Values(svt_av1_copy_wxh_16bit_c),
                       ::testing::Values(svt_av1_copy_wxh_16bit_neon)));
#endif  // ARCH_AARCH64
}  // namespace
