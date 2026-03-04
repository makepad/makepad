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
 * @file HbdVarianceTest.cc
 *
 * @brief Unit test for HBD variance
 * functions:
 * - svt_aom_highbd_BD{8,10,12}_varianceW{8,16,32,64}xH{4,8,16,32,64}
 *
 * @author  Cidana-Wenyao, Cidana-Edmond
 *
 ******************************************************************************/
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <new>

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "utility.h"
#include "random.h"
#include "util.h"

using svt_av1_test_tool::SVTRandom;  // to generate the random
namespace {
#define MAX_BLOCK_SIZE (128 * 128)

#if CONFIG_ENABLE_HIGH_BIT_DEPTH

using HighBdGetVarianceFunc = void (*)(const uint8_t *src8, int32_t src_stride,
                                       const uint8_t *ref8, int32_t ref_stride,
                                       uint32_t *sse, int32_t *sum);

using HighBdVarianceFunc = uint32_t (*)(const uint8_t *src8, int32_t src_stride,
                                        const uint8_t *ref8, int32_t ref_stride,
                                        uint32_t *sse);

// Truncate high bit depth results by downshifting (with rounding) by:
// 2 * (bit_depth - 8) for sse
// (bit_depth - 8) for se
static void round_hbd(const uint8_t bd, int64_t *se, uint64_t *sse) {
    switch (bd) {
    case 12:
        *sse = (*sse + 128) >> 8;
        *se = (*se + 8) >> 4;
        break;
    case 10:
        *sse = (*sse + 8) >> 4;
        *se = (*se + 2) >> 2;
        break;
    case 8:
    default: break;
    }
}

static void hbd_get_variance_ref(const uint32_t width, const uint32_t height,
                                 const uint8_t bd, const uint8_t *src8,
                                 int32_t src_stride, const uint8_t *ref8,
                                 int32_t ref_stride, uint32_t *sse,
                                 int32_t *sum) {
    int64_t sum_tmp = 0;
    uint64_t sse_tmp = 0;
    *sse = 0;
    for (uint32_t y = 0; y < height; y++) {
        for (uint32_t x = 0; x < width; x++) {
            int diff = CONVERT_TO_SHORTPTR(src8)[y * src_stride + x] -
                       CONVERT_TO_SHORTPTR(ref8)[y * ref_stride + x];
            sum_tmp += diff;
            sse_tmp += diff * diff;
        }
    }
    round_hbd(bd, &sum_tmp, &sse_tmp);
    *sse = static_cast<uint32_t>(sse_tmp);
    *sum = static_cast<int32_t>(sum_tmp);
}

static uint32_t hbd_variance_ref(const uint32_t width, const uint32_t height,
                                 const uint8_t bd, const uint8_t *src8,
                                 int32_t src_stride, const uint8_t *ref8,
                                 int32_t ref_stride, uint32_t *sse) {
    int32_t sum = 0;
    hbd_get_variance_ref(
        width, height, bd, src8, src_stride, ref8, ref_stride, sse, &sum);
    return static_cast<uint32_t>(
        *sse - ((((int64_t)sum * sum)) / ((int64_t)width * height)));
}

// High bit-depth variance test
using HbdVarianceParam = std::tuple<uint32_t,            /**< width */
                                    uint32_t,            /**< height */
                                    uint32_t,            /**< bit-depth */
                                    HighBdVarianceFunc>; /**< test function */

/**
 * @brief Unit test for HBD variance
 * functions:
 * - svt_aom_highbd_BD{8,10,12}_varianceW{8,16,32,64}xH{4,8,16,32,64}
 *
 * Test strategy:
 *  This test case use random source, max source, zero source as test
 * pattern.
 *
 *
 * Expected result:
 *  Results come from reference function and target function are
 * equal.
 *
 * Test cases:
 * - ZeroTest
 * - MaximumTest
 * - MatchTest
 */
class HbdVarianceTest : public ::testing::TestWithParam<HbdVarianceParam> {
  public:
    HbdVarianceTest()
        : rnd_(16, false),
          width_(TEST_GET_PARAM(0)),
          height_(TEST_GET_PARAM(1)),
          bd_(TEST_GET_PARAM(2)),
          tst_func_(TEST_GET_PARAM(3)) {
        src_data_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, 2 * MAX_BLOCK_SIZE));
        ref_data_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, 2 * MAX_BLOCK_SIZE));
    }

    ~HbdVarianceTest() {
        svt_aom_free(src_data_);
        src_data_ = nullptr;
        svt_aom_free(ref_data_);
        ref_data_ = nullptr;
    }

    void run_zero_test(int times) {
        for (int i = 0; i < times; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
                src_data_[j] = rnd_.random() & ((1 << bd_) - 1);
                ref_data_[j] = src_data_[j];
            }
            uint32_t sse_tst = 0;
            uint32_t var_tst = tst_func_(CONVERT_TO_BYTEPTR(src_data_),
                                         width_,
                                         CONVERT_TO_BYTEPTR(ref_data_),
                                         width_,
                                         &sse_tst);
            ASSERT_EQ(var_tst, 0u) << "Expect 0 variance, got: " << var_tst;
        }
    }

    void run_maximum_test() {
        for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
            src_data_[j] = 0;
            ref_data_[j] = (1 << bd_) - 1;
        }
        uint32_t sse_tst = 0, sse_ref = 0;
        uint32_t var_tst = tst_func_(CONVERT_TO_BYTEPTR(src_data_),
                                     width_,
                                     CONVERT_TO_BYTEPTR(ref_data_),
                                     width_,
                                     &sse_tst);
        uint32_t var_ref = hbd_variance_ref(width_,
                                            height_,
                                            bd_,
                                            CONVERT_TO_BYTEPTR(src_data_),
                                            width_,
                                            CONVERT_TO_BYTEPTR(ref_data_),
                                            width_,
                                            &sse_ref);
        ASSERT_EQ(var_tst, var_ref)
            << "Expect var " << var_ref << " got " << var_tst
            << " size: " << width_ << "x" << height_;
        ASSERT_EQ(sse_tst, sse_ref)
            << "Expect sse " << sse_ref << " got " << sse_tst
            << " size: " << width_ << "x" << height_;
    }

    void run_match_test(int times) {
        for (int i = 0; i < times; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
                src_data_[j] = rnd_.random() & ((1 << bd_) - 1);
                ref_data_[j] = rnd_.random() & ((1 << bd_) - 1);
            }
            uint32_t sse_tst = 0, sse_ref = 0;
            uint32_t var_tst = tst_func_(CONVERT_TO_BYTEPTR(src_data_),
                                         width_,
                                         CONVERT_TO_BYTEPTR(ref_data_),
                                         width_,
                                         &sse_tst);
            uint32_t var_ref = hbd_variance_ref(width_,
                                                height_,
                                                bd_,
                                                CONVERT_TO_BYTEPTR(src_data_),
                                                width_,
                                                CONVERT_TO_BYTEPTR(ref_data_),
                                                width_,
                                                &sse_ref);
            ASSERT_EQ(var_tst, var_ref)
                << "Error at variance test index: " << i << " size: " << width_
                << "x" << height_;
            ASSERT_EQ(sse_tst, sse_ref)
                << "Error at sse test index: " << i << " size: " << width_
                << "x" << height_;
        }
    }

  private:
    SVTRandom rnd_;
    uint16_t *src_data_;
    uint16_t *ref_data_;
    uint32_t width_;
    uint32_t height_;
    uint32_t bd_;
    HighBdVarianceFunc tst_func_;
};

TEST_P(HbdVarianceTest, ZeroTest) {
    run_zero_test(10);
};

TEST_P(HbdVarianceTest, MaximumTest) {
    run_maximum_test();
};

TEST_P(HbdVarianceTest, MatchTest) {
    run_match_test(10);
};

#ifdef ARCH_X86_64

static const HbdVarianceParam HbdTestVector_sse2[] = {
    HbdVarianceParam(8, 8, 10, svt_aom_highbd_10_variance8x8_sse2),
    HbdVarianceParam(8, 16, 10, svt_aom_highbd_10_variance8x16_sse2),
    HbdVarianceParam(8, 32, 10, svt_aom_highbd_10_variance8x32_sse2),
    HbdVarianceParam(16, 4, 10, svt_aom_highbd_10_variance16x4_sse2),
    HbdVarianceParam(16, 8, 10, svt_aom_highbd_10_variance16x8_sse2),
    HbdVarianceParam(16, 16, 10, svt_aom_highbd_10_variance16x16_sse2),
    HbdVarianceParam(16, 32, 10, svt_aom_highbd_10_variance16x32_sse2),
    HbdVarianceParam(16, 64, 10, svt_aom_highbd_10_variance16x64_sse2),
    HbdVarianceParam(32, 8, 10, svt_aom_highbd_10_variance32x8_sse2),
    HbdVarianceParam(32, 16, 10, svt_aom_highbd_10_variance32x16_sse2),
    HbdVarianceParam(32, 32, 10, svt_aom_highbd_10_variance32x32_sse2),
    HbdVarianceParam(32, 64, 10, svt_aom_highbd_10_variance32x64_sse2),
    HbdVarianceParam(64, 16, 10, svt_aom_highbd_10_variance64x16_sse2),
    HbdVarianceParam(64, 32, 10, svt_aom_highbd_10_variance64x32_sse2),
    HbdVarianceParam(64, 64, 10, svt_aom_highbd_10_variance64x64_sse2),
    HbdVarianceParam(64, 128, 10, svt_aom_highbd_10_variance64x128_sse2),
    HbdVarianceParam(128, 64, 10, svt_aom_highbd_10_variance128x64_sse2),
    HbdVarianceParam(128, 128, 10, svt_aom_highbd_10_variance128x128_sse2),
};

INSTANTIATE_TEST_SUITE_P(SSE2, HbdVarianceTest,
                         ::testing::ValuesIn(HbdTestVector_sse2));

static const HbdVarianceParam HbdTestVector_avx2[] = {
    HbdVarianceParam(8, 8, 10, svt_aom_highbd_10_variance8x8_avx2),
    HbdVarianceParam(8, 16, 10, svt_aom_highbd_10_variance8x16_avx2),
    HbdVarianceParam(8, 32, 10, svt_aom_highbd_10_variance8x32_avx2),
    HbdVarianceParam(16, 8, 10, svt_aom_highbd_10_variance16x8_avx2),
    HbdVarianceParam(16, 16, 10, svt_aom_highbd_10_variance16x16_avx2),
    HbdVarianceParam(16, 32, 10, svt_aom_highbd_10_variance16x32_avx2),
    HbdVarianceParam(16, 64, 10, svt_aom_highbd_10_variance16x64_avx2),
    HbdVarianceParam(32, 8, 10, svt_aom_highbd_10_variance32x8_avx2),
    HbdVarianceParam(32, 16, 10, svt_aom_highbd_10_variance32x16_avx2),
    HbdVarianceParam(32, 32, 10, svt_aom_highbd_10_variance32x32_avx2),
    HbdVarianceParam(32, 64, 10, svt_aom_highbd_10_variance32x64_avx2),
    HbdVarianceParam(64, 16, 10, svt_aom_highbd_10_variance64x16_avx2),
    HbdVarianceParam(64, 32, 10, svt_aom_highbd_10_variance64x32_avx2),
    HbdVarianceParam(64, 64, 10, svt_aom_highbd_10_variance64x64_avx2),
    HbdVarianceParam(64, 128, 10, svt_aom_highbd_10_variance64x128_avx2),
    HbdVarianceParam(128, 64, 10, svt_aom_highbd_10_variance128x64_avx2),
    HbdVarianceParam(128, 128, 10, svt_aom_highbd_10_variance128x128_avx2),
};

INSTANTIATE_TEST_SUITE_P(AVX2, HbdVarianceTest,
                         ::testing::ValuesIn(HbdTestVector_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

static const HbdVarianceParam HbdTestVector_neon[] = {
    HbdVarianceParam(4, 4, 10, svt_aom_highbd_10_variance4x4_neon),
    HbdVarianceParam(4, 8, 10, svt_aom_highbd_10_variance4x8_neon),
    HbdVarianceParam(4, 16, 10, svt_aom_highbd_10_variance4x16_neon),
    HbdVarianceParam(8, 4, 10, svt_aom_highbd_10_variance8x4_neon),
    HbdVarianceParam(8, 8, 10, svt_aom_highbd_10_variance8x8_neon),
    HbdVarianceParam(8, 16, 10, svt_aom_highbd_10_variance8x16_neon),
    HbdVarianceParam(8, 32, 10, svt_aom_highbd_10_variance8x32_neon),
    HbdVarianceParam(16, 4, 10, svt_aom_highbd_10_variance16x4_neon),
    HbdVarianceParam(16, 8, 10, svt_aom_highbd_10_variance16x8_neon),
    HbdVarianceParam(16, 16, 10, svt_aom_highbd_10_variance16x16_neon),
    HbdVarianceParam(16, 32, 10, svt_aom_highbd_10_variance16x32_neon),
    HbdVarianceParam(16, 64, 10, svt_aom_highbd_10_variance16x64_neon),
    HbdVarianceParam(32, 8, 10, svt_aom_highbd_10_variance32x8_neon),
    HbdVarianceParam(32, 16, 10, svt_aom_highbd_10_variance32x16_neon),
    HbdVarianceParam(32, 32, 10, svt_aom_highbd_10_variance32x32_neon),
    HbdVarianceParam(32, 64, 10, svt_aom_highbd_10_variance32x64_neon),
    HbdVarianceParam(64, 16, 10, svt_aom_highbd_10_variance64x16_neon),
    HbdVarianceParam(64, 32, 10, svt_aom_highbd_10_variance64x32_neon),
    HbdVarianceParam(64, 64, 10, svt_aom_highbd_10_variance64x64_neon),
    HbdVarianceParam(64, 128, 10, svt_aom_highbd_10_variance64x128_neon),
    HbdVarianceParam(128, 64, 10, svt_aom_highbd_10_variance128x64_neon),
    HbdVarianceParam(128, 128, 10, svt_aom_highbd_10_variance128x128_neon),
};

INSTANTIATE_TEST_SUITE_P(NEON, HbdVarianceTest,
                         ::testing::ValuesIn(HbdTestVector_neon));

#if HAVE_SVE

static const HbdVarianceParam HbdTestVector_sve[] = {
    HbdVarianceParam(4, 4, 10, svt_aom_highbd_10_variance4x4_sve),
    HbdVarianceParam(4, 8, 10, svt_aom_highbd_10_variance4x8_sve),
    HbdVarianceParam(4, 16, 10, svt_aom_highbd_10_variance4x16_sve),
    HbdVarianceParam(8, 4, 10, svt_aom_highbd_10_variance8x4_sve),
    HbdVarianceParam(8, 8, 10, svt_aom_highbd_10_variance8x8_sve),
    HbdVarianceParam(8, 16, 10, svt_aom_highbd_10_variance8x16_sve),
    HbdVarianceParam(8, 32, 10, svt_aom_highbd_10_variance8x32_sve),
    HbdVarianceParam(16, 4, 10, svt_aom_highbd_10_variance16x4_sve),
    HbdVarianceParam(16, 8, 10, svt_aom_highbd_10_variance16x8_sve),
    HbdVarianceParam(16, 16, 10, svt_aom_highbd_10_variance16x16_sve),
    HbdVarianceParam(16, 32, 10, svt_aom_highbd_10_variance16x32_sve),
    HbdVarianceParam(16, 64, 10, svt_aom_highbd_10_variance16x64_sve),
    HbdVarianceParam(32, 8, 10, svt_aom_highbd_10_variance32x8_sve),
    HbdVarianceParam(32, 16, 10, svt_aom_highbd_10_variance32x16_sve),
    HbdVarianceParam(32, 32, 10, svt_aom_highbd_10_variance32x32_sve),
    HbdVarianceParam(32, 64, 10, svt_aom_highbd_10_variance32x64_sve),
    HbdVarianceParam(64, 16, 10, svt_aom_highbd_10_variance64x16_sve),
    HbdVarianceParam(64, 32, 10, svt_aom_highbd_10_variance64x32_sve),
    HbdVarianceParam(64, 64, 10, svt_aom_highbd_10_variance64x64_sve),
    HbdVarianceParam(64, 128, 10, svt_aom_highbd_10_variance64x128_sve),
    HbdVarianceParam(128, 64, 10, svt_aom_highbd_10_variance128x64_sve),
    HbdVarianceParam(128, 128, 10, svt_aom_highbd_10_variance128x128_sve),
};

INSTANTIATE_TEST_SUITE_P(SVE, HbdVarianceTest,
                         ::testing::ValuesIn(HbdTestVector_sve));
#endif  // HAVE_SVE

#endif  // ARCH_AARCH64

/**
 * @brief Unit test for different implementation of HBD variance with size 16x16
 * and 32x32 functions:
 * - svt_aom_variance_highbd_c
 * - svt_aom_variance_highbd_sse4_1
 * - svt_aom_variance_highbd_avx2
 *
 * Test strategy:
 *  This test case use random source, max source, zero source as test
 * pattern.
 *
 * Expected result:
 *  Results come from reference function and target function are
 * equal.
 *
 * Test cases:
 * - ZeroTest
 * - MaximumTest
 * - MatchTest
 *
 * @author  intel tszumski
 *
 */
using HbdSquareVarianceNoRoundFunc = uint32_t (*)(const uint16_t *src,
                                                  int32_t src_stride,
                                                  const uint16_t *ref,
                                                  int32_t ref_stride, int w,
                                                  int h, uint32_t *sse);

using HbdSquareVarianceNoRoundParam =
    std::tuple<uint32_t,                      /**< square length */
               HbdSquareVarianceNoRoundFunc>; /**< test function */

class HbdSquareVarianceNoRoundTest
    : public ::testing::TestWithParam<HbdSquareVarianceNoRoundParam> {
  public:
    HbdSquareVarianceNoRoundTest()
        : rnd_(16, false),
          bd_(10),
          length_(TEST_GET_PARAM(0)),
          tst_func_(TEST_GET_PARAM(1)) {
        src_data_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, 2 * MAX_BLOCK_SIZE));
        ref_data_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, 2 * MAX_BLOCK_SIZE));
    }

    ~HbdSquareVarianceNoRoundTest() {
        svt_aom_free(src_data_);
        src_data_ = nullptr;
        svt_aom_free(ref_data_);
        ref_data_ = nullptr;
    }

    void run_zero_test(int times) {
        for (int i = 0; i < times; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
                src_data_[j] = rnd_.random() & ((1 << bd_) - 1);
                ref_data_[j] = src_data_[j];
            }
            int32_t distortion_tst = 0;
            uint32_t sse_tst = 0;
            distortion_tst = tst_func_(src_data_,
                                       length_,
                                       ref_data_,
                                       length_,
                                       length_,
                                       length_,
                                       &sse_tst);
            ASSERT_EQ(sse_tst, 0u) << "Expect 0 sse, got: " << sse_tst;
            ASSERT_EQ(distortion_tst, 0)
                << "Expect 0 distortion, got: " << distortion_tst;
        }
    }

    void run_maximum_test() {
        for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
            src_data_[j] = 0;
            ref_data_[j] = (1 << bd_) - 1;
        }

        int32_t distortion_tst = 0, distortion_ref = 0;
        uint32_t sse_tst = 0, sse_ref = 0;

        distortion_tst = tst_func_(
            src_data_, length_, ref_data_, length_, length_, length_, &sse_tst);

        distortion_ref = svt_aom_variance_highbd_c(
            src_data_, length_, ref_data_, length_, length_, length_, &sse_ref);
        ASSERT_EQ(distortion_tst, distortion_ref)
            << "Error at distortion in variance test";
        ASSERT_EQ(sse_tst, sse_ref) << "Error at error sse in variance test";
    }

    void run_match_test(int times) {
        for (int i = 0; i < times; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
                src_data_[j] = rnd_.random() & ((1 << bd_) - 1);
                ref_data_[j] = rnd_.random() & ((1 << bd_) - 1);
            }
            int32_t distortion_tst = 0, distortion_ref = 0;
            uint32_t sse_tst = 0, sse_ref = 0;

            distortion_tst = tst_func_(src_data_,
                                       length_,
                                       ref_data_,
                                       length_,
                                       length_,
                                       length_,
                                       &sse_tst);

            distortion_ref = svt_aom_variance_highbd_c(src_data_,
                                                       length_,
                                                       ref_data_,
                                                       length_,
                                                       length_,
                                                       length_,
                                                       &sse_ref);
            ASSERT_EQ(distortion_tst, distortion_ref)
                << "Error at distortion in variance test";
            ASSERT_EQ(sse_tst, sse_ref)
                << "Error at error sse in variance test";
        }
    }

  private:
    SVTRandom rnd_;
    uint32_t bd_;
    uint32_t length_;
    HbdSquareVarianceNoRoundFunc tst_func_;
    uint16_t *src_data_;
    uint16_t *ref_data_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(HbdSquareVarianceNoRoundTest);

TEST_P(HbdSquareVarianceNoRoundTest, ZeroTest) {
    run_zero_test(10);
};

TEST_P(HbdSquareVarianceNoRoundTest, MaximumTest) {
    run_maximum_test();
};

TEST_P(HbdSquareVarianceNoRoundTest, MatchTest) {
    run_match_test(10);
};

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, HbdSquareVarianceNoRoundTest,
    ::testing::Combine(::testing::Values(16, 32),
                       ::testing::Values(svt_aom_variance_highbd_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, HbdSquareVarianceNoRoundTest,
    ::testing::Combine(::testing::Values(16, 32),
                       ::testing::Values(svt_aom_variance_highbd_avx2)));
#endif

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

}  // namespace
