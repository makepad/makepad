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
 * @file VarianceTest.cc
 *
 * @brief Unit test for variance, mse, sum square functions:
 * - svt_aom_variance{4-128}x{4-128}_{c,sse2,avx2}
 * - svt_aom_get_mb_ss_sse2
 * - aom_mse16x16_{c,avx2}
 * - highbd_variance64_{c,avx2}
 *
 * @author Cidana-Ryan,Cidana-Ivy
 *
 ******************************************************************************/
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <new>

#include "aom_dsp_rtcd.h"
#include "random.h"
#include "util.h"
#include "gtest/gtest.h"
#include "utility.h"

using svt_av1_test_tool::SVTRandom;  // to generate the random
namespace {
#define MAX_BLOCK_SIZE (128 * 128)

using MSE_NXM_FUNC = uint32_t (*)(const uint8_t *src_ptr, int32_t src_stride,
                                  const uint8_t *ref_ptr, int32_t ref_stride);

using MSE_HIGHBD_NXM_FUNC = uint32_t (*)(const uint8_t *src_ptr,
                                         int32_t src_stride,
                                         const uint8_t *ref_ptr,
                                         int32_t ref_stride);

typedef std::tuple<uint32_t, uint32_t, MSE_NXM_FUNC, MSE_NXM_FUNC> TestMseParam;
typedef std::tuple<uint32_t, uint32_t, MSE_HIGHBD_NXM_FUNC, MSE_HIGHBD_NXM_FUNC>
    TestMseParamHighbd;
/**
 * @brief Unit test for mse functions, target functions include:
 *  - aom_mse16x16_{c,avx2}
 *
 * Test strategy:
 *  This test case use random source, max source, zero source as test
 * pattern.
 *
 *
 * Expect result:
 *  Results come from reference functon and target function are
 * equal.
 *
 * Test coverage:
 *
 * Test cases:
 *
 */
class MseTest : public ::testing::TestWithParam<TestMseParam> {
  public:
    MseTest()
        : width_(TEST_GET_PARAM(0)),
          height_(TEST_GET_PARAM(1)),
          mse_tst_(TEST_GET_PARAM(2)),
          mse_ref_(TEST_GET_PARAM(3)) {
        src_data_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, MAX_BLOCK_SIZE));
        ref_data_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, MAX_BLOCK_SIZE));
    }

    ~MseTest() {
        svt_aom_free(src_data_);
        src_data_ = nullptr;
        svt_aom_free(ref_data_);
        ref_data_ = nullptr;
    }

    void run_match_test() {
        const int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        for (int i = 0; i < 10; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
                src_data_[j] = rnd.random();
                ref_data_[j] = rnd.random();
            }

            unsigned int res_tst =
                mse_tst_(src_data_, width_, ref_data_, height_);
            unsigned int res_ref =
                mse_ref_(src_data_, width_, ref_data_, height_);
            ASSERT_EQ(res_tst, res_ref) << "Return value error at index: " << i;
        }
    }

    void run_max_test() {
        memset(src_data_, 255, MAX_BLOCK_SIZE);
        memset(ref_data_, 0, MAX_BLOCK_SIZE);
        unsigned int res_tst = mse_tst_(src_data_, width_, ref_data_, width_);
        const uint32_t expected = width_ * height_ * 255 * 255;
        ASSERT_EQ(res_tst, expected)
            << "Return value error at MSE maximum test";
    }

  private:
    uint8_t *src_data_;
    uint8_t *ref_data_;
    uint32_t width_;
    uint32_t height_;
    MSE_NXM_FUNC mse_tst_;
    MSE_NXM_FUNC mse_ref_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(MseTest);

TEST_P(MseTest, MatchTest) {
    run_match_test();
};

TEST_P(MseTest, MaxTest) {
    run_max_test();
};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE2, MseTest,
                         ::testing::Values(TestMseParam(16, 16,
                                                        &svt_aom_mse16x16_sse2,
                                                        &svt_aom_mse16x16_c)));

INSTANTIATE_TEST_SUITE_P(AVX2, MseTest,
                         ::testing::Values(TestMseParam(16, 16,
                                                        &svt_aom_mse16x16_avx2,
                                                        &svt_aom_mse16x16_c)));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, MseTest,
                         ::testing::Values(TestMseParam(16, 16,
                                                        &svt_aom_mse16x16_neon,
                                                        &svt_aom_mse16x16_c)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, MseTest,
    ::testing::Values(TestMseParam(16, 16, &svt_aom_mse16x16_neon_dotprod,
                                   &svt_aom_mse16x16_c)));
#endif  // HAVE_NEON_DOTPROD

#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
class MseTestHighbd : public ::testing::TestWithParam<TestMseParamHighbd> {
  public:
    MseTestHighbd()
        : width_(TEST_GET_PARAM(0)),
          height_(TEST_GET_PARAM(1)),
          mse_tst_(TEST_GET_PARAM(2)),
          mse_ref_(TEST_GET_PARAM(3)) {
        src_data_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, MAX_BLOCK_SIZE * 2));
        ref_data_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, MAX_BLOCK_SIZE * 2));
    }

    ~MseTestHighbd() {
        svt_aom_free(src_data_);
        src_data_ = nullptr;
        svt_aom_free(ref_data_);
        ref_data_ = nullptr;
    }

    void run_match_test() {
        const int32_t mask = (1 << 10) - 1;
        SVTRandom rnd(0, mask);
        for (int i = 0; i < 10; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
                src_data_[j] = rnd.random();
                ref_data_[j] = rnd.random();
            }

            uint32_t sse_ref = mse_ref_(CONVERT_TO_BYTEPTR(src_data_),
                                        width_,
                                        CONVERT_TO_BYTEPTR(ref_data_),
                                        height_);
            uint32_t sse_tst = mse_tst_(CONVERT_TO_BYTEPTR(src_data_),
                                        width_,
                                        CONVERT_TO_BYTEPTR(ref_data_),
                                        height_);
            ASSERT_EQ(sse_tst, sse_ref) << "SSE Error at index: " << i;
        }
    }

    void run_max_test() {
        const int32_t mask = (1 << 10) - 1;
        for (int j = 0; j < MAX_BLOCK_SIZE; ++j) {
            src_data_[j] = mask;
            ref_data_[j] = 0;
        }
        uint32_t sse_ref = mse_ref_(CONVERT_TO_BYTEPTR(src_data_),
                                    width_,
                                    CONVERT_TO_BYTEPTR(ref_data_),
                                    height_);
        uint32_t sse_tst = mse_tst_(CONVERT_TO_BYTEPTR(src_data_),
                                    width_,
                                    CONVERT_TO_BYTEPTR(ref_data_),
                                    height_);

        ASSERT_EQ(sse_tst, sse_ref) << "Error at MSE maximum test ";
    }

  private:
    uint16_t *src_data_;
    uint16_t *ref_data_;
    uint32_t width_;
    uint32_t height_;
    MSE_HIGHBD_NXM_FUNC mse_tst_;
    MSE_HIGHBD_NXM_FUNC mse_ref_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(MseTestHighbd);

TEST_P(MseTestHighbd, MatchTest) {
    run_match_test();
};

TEST_P(MseTestHighbd, MaxTest) {
    run_max_test();
};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, MseTestHighbd,
    ::testing::Values(TestMseParamHighbd(16, 16, &svt_aom_highbd_mse16x16_sse2,
                                         &svt_aom_highbd_mse16x16_c)));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, MseTestHighbd,
    ::testing::Values(TestMseParamHighbd(16, 16, &svt_aom_highbd_mse16x16_neon,
                                         &svt_aom_highbd_mse16x16_c)));

#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

// sum of squares test
static uint32_t mb_ss_ref(const int16_t *src) {
    uint32_t res = 0;
    for (int i = 0; i < 256; ++i)
        res += src[i] * src[i];
    return res;
}

using SUM_SQUARE_FUNC = uint32_t (*)(const int16_t *src);
/**
 * @brief Unit test for sum square functions, target functions include:
 *  - aom_get_mb_ss_sse2
 *
 * Test strategy:
 *  This test case use random value, const value (0-255) as test
 * pattern.
 *
 *
 * Expect result:
 *  Results come from reference functon and target function are
 * equal.
 *
 * Test coverage:
 *
 * Test cases:
 *
 */
class SumSquareTest : public ::testing::TestWithParam<SUM_SQUARE_FUNC> {
  protected:
    void run_const_test() {
        int16_t mem[256];
        uint32_t res;
        for (int16_t v = 0; v < 256; ++v) {
            for (int i = 0; i < 256; ++i)
                mem[i] = v;

            SUM_SQUARE_FUNC sum_square_func = GetParam();
            res = sum_square_func(mem);
            ASSERT_EQ(res, 256u * (v * v))
                << "Error at sum test in const value: " << v;
        }
    }

    void run_match_test() {
        int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        int16_t mem[256];
        for (int i = 0; i < 100; ++i) {
            for (int j = 0; j < 256; ++j)
                mem[j] = rnd.random() - rnd.random();

            const uint32_t expected = mb_ss_ref(mem);
            SUM_SQUARE_FUNC sum_square_func = GetParam();
            uint32_t res = sum_square_func(mem);
            ASSERT_EQ(res, expected) << "Error at sum test in index: " << i;
        }
    }
};

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(SumSquareTest);

TEST_P(SumSquareTest, ConstTest) {
    run_const_test();
};

TEST_P(SumSquareTest, MatchTest) {
    run_match_test();
};

// Variance test
using VARIANCE_NXM_FUNC = uint32_t (*)(const uint8_t *src_ptr,
                                       int32_t src_stride,
                                       const uint8_t *ref_ptr,
                                       int32_t recon_stride, uint32_t *sse);

using VarianceParam =
    std::tuple<uint32_t, uint32_t, VARIANCE_NXM_FUNC, VARIANCE_NXM_FUNC>;

/**
 * @brief Unit test for variance functions, target functions include:
 *  - - svt_aom_variance{4-128}x{4-128}_{c,avx2}
 *
 * Test strategy:
 *  This test case contains zero test, random value test, one quarter test as
 * test pattern.
 *
 *
 * Expect result:
 *  Results come from reference functon and target function are
 * equal.
 *
 * Test coverage:
 *
 * Test cases:
 *
 */
class VarianceTest : public ::testing::TestWithParam<VarianceParam> {
  public:
    VarianceTest()
        : width_(TEST_GET_PARAM(0)),
          height_(TEST_GET_PARAM(1)),
          func_c_(TEST_GET_PARAM(2)),
          func_asm_(TEST_GET_PARAM(3)) {
        src_data_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, MAX_BLOCK_SIZE));
        ref_data_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, MAX_BLOCK_SIZE));
    }

    ~VarianceTest() {
        svt_aom_free(src_data_);
        src_data_ = nullptr;
        svt_aom_free(ref_data_);
        ref_data_ = nullptr;
    }

    void run_zero_test() {
        for (int i = 0; i < 256; ++i) {
            memset(src_data_, i, MAX_BLOCK_SIZE);
            for (int j = 0; j < 256; ++j) {
                memset(ref_data_, j, MAX_BLOCK_SIZE);

                uint32_t sse, var;
                var = func_asm_(src_data_, width_, ref_data_, width_, &sse);
                ASSERT_EQ(var, 0u)
                    << "Variance is mismatched, src values: " << i
                    << " ref values: " << j;
            }
        }
    }

    void run_match_test() {
        int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        for (int i = 0; i < 10; ++i) {
            for (int j = 0; j < MAX_BLOCK_SIZE; j++) {
                src_data_[j] = rnd.random();
                ref_data_[j] = rnd.random();
            }
            uint32_t sse_c, sse_asm, var_c, var_asm;

            var_c = func_c_(src_data_, width_, ref_data_, width_ + 1, &sse_c);
            var_asm =
                func_asm_(src_data_, width_, ref_data_, width_ + 1, &sse_asm);
            ASSERT_EQ(sse_c, sse_asm) << "Error at test index: " << i;
            ASSERT_EQ(var_c, var_asm) << "Error at test index: " << i;
        }
    }

    void run_one_quarter_test() {
        const int half = width_ * height_ / 2;
        memset(src_data_, 255, MAX_BLOCK_SIZE);
        memset(ref_data_, 255, half);
        memset(ref_data_ + half, 0, half);
        uint32_t sse_asm, var_asm, expected;
        var_asm = func_asm_(src_data_, width_, ref_data_, width_, &sse_asm);
        expected = width_ * height_ * 255 * 255 / 4;
        ASSERT_EQ(var_asm, expected);
    }

  private:
    uint8_t *src_data_;
    uint8_t *ref_data_;
    uint32_t width_;
    uint32_t height_;
    VARIANCE_NXM_FUNC func_c_;
    VARIANCE_NXM_FUNC func_asm_;
};

TEST_P(VarianceTest, ZeroTest) {
    run_zero_test();
};

TEST_P(VarianceTest, MatchTest) {
    run_match_test();
};

TEST_P(VarianceTest, OneQuarterTest) {
    run_one_quarter_test();
};

#ifdef ARCH_X86_64
VarianceParam variance_func_sse2[] = {
    VarianceParam(4, 4, &svt_aom_variance4x4_c, &svt_aom_variance4x4_sse2),
    VarianceParam(4, 8, &svt_aom_variance4x8_c, &svt_aom_variance4x8_sse2),
    VarianceParam(4, 16, &svt_aom_variance4x16_c, &svt_aom_variance4x16_sse2),
    VarianceParam(8, 4, &svt_aom_variance8x4_c, &svt_aom_variance8x4_sse2),
    VarianceParam(8, 8, &svt_aom_variance8x8_c, &svt_aom_variance8x8_sse2),
    VarianceParam(8, 16, &svt_aom_variance8x16_c, &svt_aom_variance8x16_sse2),
    VarianceParam(8, 32, &svt_aom_variance8x32_c, &svt_aom_variance8x32_sse2),
    VarianceParam(16, 4, &svt_aom_variance16x4_c, &svt_aom_variance16x4_sse2),
    VarianceParam(16, 8, &svt_aom_variance16x8_c, &svt_aom_variance16x8_sse2),
    VarianceParam(16, 16, &svt_aom_variance16x16_c,
                  &svt_aom_variance16x16_sse2),
    VarianceParam(16, 32, &svt_aom_variance16x32_c,
                  &svt_aom_variance16x32_sse2),
    VarianceParam(16, 64, &svt_aom_variance16x64_c,
                  &svt_aom_variance16x64_sse2),
    VarianceParam(32, 8, &svt_aom_variance32x8_c, &svt_aom_variance32x8_sse2),
    VarianceParam(32, 16, &svt_aom_variance32x16_c,
                  &svt_aom_variance32x16_sse2),
    VarianceParam(32, 32, &svt_aom_variance32x32_c,
                  &svt_aom_variance32x32_sse2),
    VarianceParam(32, 64, &svt_aom_variance32x64_c,
                  &svt_aom_variance32x64_sse2),
    VarianceParam(64, 16, &svt_aom_variance64x16_c,
                  &svt_aom_variance64x16_sse2),
    VarianceParam(64, 32, &svt_aom_variance64x32_c,
                  &svt_aom_variance64x32_sse2),
    VarianceParam(64, 64, &svt_aom_variance64x64_c,
                  &svt_aom_variance64x64_sse2),
    VarianceParam(64, 128, &svt_aom_variance64x128_c,
                  &svt_aom_variance64x128_sse2),
    VarianceParam(128, 64, &svt_aom_variance128x64_c,
                  &svt_aom_variance128x64_sse2),
    VarianceParam(128, 128, &svt_aom_variance128x128_c,
                  &svt_aom_variance128x128_sse2)};

VarianceParam variance_func_avx2[] = {
    VarianceParam(8, 4, &svt_aom_variance8x4_c, &svt_aom_variance8x4_avx2),
    VarianceParam(8, 8, &svt_aom_variance8x8_c, &svt_aom_variance8x8_avx2),
    VarianceParam(8, 16, &svt_aom_variance8x16_c, &svt_aom_variance8x16_avx2),
    VarianceParam(8, 32, &svt_aom_variance8x32_c, &svt_aom_variance8x32_avx2),
    VarianceParam(16, 4, &svt_aom_variance16x4_c, &svt_aom_variance16x4_avx2),
    VarianceParam(16, 8, &svt_aom_variance16x8_c, &svt_aom_variance16x8_avx2),
    VarianceParam(16, 16, &svt_aom_variance16x16_c,
                  &svt_aom_variance16x16_avx2),
    VarianceParam(16, 32, &svt_aom_variance16x32_c,
                  &svt_aom_variance16x32_avx2),
    VarianceParam(16, 64, &svt_aom_variance16x64_c,
                  &svt_aom_variance16x64_avx2),
    VarianceParam(32, 8, &svt_aom_variance32x8_c, &svt_aom_variance32x8_avx2),
    VarianceParam(32, 16, &svt_aom_variance32x16_c,
                  &svt_aom_variance32x16_avx2),
    VarianceParam(32, 32, &svt_aom_variance32x32_c,
                  &svt_aom_variance32x32_avx2),
    VarianceParam(32, 64, &svt_aom_variance32x64_c,
                  &svt_aom_variance32x64_avx2),
    VarianceParam(64, 16, &svt_aom_variance64x16_c,
                  &svt_aom_variance64x16_avx2),
    VarianceParam(64, 32, &svt_aom_variance64x32_c,
                  &svt_aom_variance64x32_avx2),
    VarianceParam(64, 64, &svt_aom_variance64x64_c,
                  &svt_aom_variance64x64_avx2),
    VarianceParam(64, 128, &svt_aom_variance64x128_c,
                  &svt_aom_variance64x128_avx2),
    VarianceParam(128, 64, &svt_aom_variance128x64_c,
                  &svt_aom_variance128x64_avx2),
    VarianceParam(128, 128, &svt_aom_variance128x128_c,
                  &svt_aom_variance128x128_avx2)};

INSTANTIATE_TEST_SUITE_P(SSE2, VarianceTest,
                         ::testing::ValuesIn(variance_func_sse2));

INSTANTIATE_TEST_SUITE_P(AVX2, VarianceTest,
                         ::testing::ValuesIn(variance_func_avx2));

#if EN_AVX512_SUPPORT
VarianceParam variance_func_avx512[] = {
    VarianceParam(32, 8, &svt_aom_variance32x8_c, &svt_aom_variance32x8_avx512),
    VarianceParam(32, 16, &svt_aom_variance32x16_c,
                  &svt_aom_variance32x16_avx512),
    VarianceParam(32, 32, &svt_aom_variance32x32_c,
                  &svt_aom_variance32x32_avx512),
    VarianceParam(32, 64, &svt_aom_variance32x64_c,
                  &svt_aom_variance32x64_avx512),
    VarianceParam(64, 16, &svt_aom_variance64x16_c,
                  &svt_aom_variance64x16_avx512),
    VarianceParam(64, 32, &svt_aom_variance64x32_c,
                  &svt_aom_variance64x32_avx512),
    VarianceParam(64, 64, &svt_aom_variance64x64_c,
                  &svt_aom_variance64x64_avx512),
    VarianceParam(64, 128, &svt_aom_variance64x128_c,
                  &svt_aom_variance64x128_avx512),
    VarianceParam(128, 64, &svt_aom_variance128x64_c,
                  &svt_aom_variance128x64_avx512),
    VarianceParam(128, 128, &svt_aom_variance128x128_c,
                  &svt_aom_variance128x128_avx512)};

INSTANTIATE_TEST_SUITE_P(AVX512, VarianceTest,
                         ::testing::ValuesIn(variance_func_avx512));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

VarianceParam variance_func_neon[] = {
    VarianceParam(4, 4, &svt_aom_variance4x4_c, &svt_aom_variance4x4_neon),
    VarianceParam(4, 8, &svt_aom_variance4x8_c, &svt_aom_variance4x8_neon),
    VarianceParam(4, 16, &svt_aom_variance4x16_c, &svt_aom_variance4x16_neon),
    VarianceParam(8, 4, &svt_aom_variance8x4_c, &svt_aom_variance8x4_neon),
    VarianceParam(8, 8, &svt_aom_variance8x8_c, &svt_aom_variance8x8_neon),
    VarianceParam(8, 16, &svt_aom_variance8x16_c, &svt_aom_variance8x16_neon),
    VarianceParam(8, 32, &svt_aom_variance8x32_c, &svt_aom_variance8x32_neon),
    VarianceParam(16, 4, &svt_aom_variance16x4_c, &svt_aom_variance16x4_neon),
    VarianceParam(16, 8, &svt_aom_variance16x8_c, &svt_aom_variance16x8_neon),
    VarianceParam(16, 16, &svt_aom_variance16x16_c,
                  &svt_aom_variance16x16_neon),
    VarianceParam(16, 32, &svt_aom_variance16x32_c,
                  &svt_aom_variance16x32_neon),
    VarianceParam(16, 64, &svt_aom_variance16x64_c,
                  &svt_aom_variance16x64_neon),
    VarianceParam(32, 8, &svt_aom_variance32x8_c, &svt_aom_variance32x8_neon),
    VarianceParam(32, 16, &svt_aom_variance32x16_c,
                  &svt_aom_variance32x16_neon),
    VarianceParam(32, 32, &svt_aom_variance32x32_c,
                  &svt_aom_variance32x32_neon),
    VarianceParam(32, 64, &svt_aom_variance32x64_c,
                  &svt_aom_variance32x64_neon),
    VarianceParam(64, 16, &svt_aom_variance64x16_c,
                  &svt_aom_variance64x16_neon),
    VarianceParam(64, 32, &svt_aom_variance64x32_c,
                  &svt_aom_variance64x32_neon),
    VarianceParam(64, 64, &svt_aom_variance64x64_c,
                  &svt_aom_variance64x64_neon),
    VarianceParam(64, 128, &svt_aom_variance64x128_c,
                  &svt_aom_variance64x128_neon),
    VarianceParam(128, 64, &svt_aom_variance128x64_c,
                  &svt_aom_variance128x64_neon),
    VarianceParam(128, 128, &svt_aom_variance128x128_c,
                  &svt_aom_variance128x128_neon)};

INSTANTIATE_TEST_SUITE_P(NEON, VarianceTest,
                         ::testing::ValuesIn(variance_func_neon));

#if HAVE_NEON_DOTPROD
VarianceParam variance_func_neon_dotprod[] = {
    VarianceParam(4, 8, &svt_aom_variance4x8_c,
                  &svt_aom_variance4x8_neon_dotprod),
    VarianceParam(4, 16, &svt_aom_variance4x16_c,
                  &svt_aom_variance4x16_neon_dotprod),
    VarianceParam(8, 4, &svt_aom_variance8x4_c,
                  &svt_aom_variance8x4_neon_dotprod),
    VarianceParam(8, 8, &svt_aom_variance8x8_c,
                  &svt_aom_variance8x8_neon_dotprod),
    VarianceParam(8, 16, &svt_aom_variance8x16_c,
                  &svt_aom_variance8x16_neon_dotprod),
    VarianceParam(8, 32, &svt_aom_variance8x32_c,
                  &svt_aom_variance8x32_neon_dotprod),
    VarianceParam(16, 4, &svt_aom_variance16x4_c,
                  &svt_aom_variance16x4_neon_dotprod),
    VarianceParam(16, 8, &svt_aom_variance16x8_c,
                  &svt_aom_variance16x8_neon_dotprod),
    VarianceParam(16, 16, &svt_aom_variance16x16_c,
                  &svt_aom_variance16x16_neon_dotprod),
    VarianceParam(16, 32, &svt_aom_variance16x32_c,
                  &svt_aom_variance16x32_neon_dotprod),
    VarianceParam(16, 64, &svt_aom_variance16x64_c,
                  &svt_aom_variance16x64_neon_dotprod),
    VarianceParam(32, 8, &svt_aom_variance32x8_c,
                  &svt_aom_variance32x8_neon_dotprod),
    VarianceParam(32, 16, &svt_aom_variance32x16_c,
                  &svt_aom_variance32x16_neon_dotprod),
    VarianceParam(32, 32, &svt_aom_variance32x32_c,
                  &svt_aom_variance32x32_neon_dotprod),
    VarianceParam(32, 64, &svt_aom_variance32x64_c,
                  &svt_aom_variance32x64_neon_dotprod),
    VarianceParam(64, 16, &svt_aom_variance64x16_c,
                  &svt_aom_variance64x16_neon_dotprod),
    VarianceParam(64, 32, &svt_aom_variance64x32_c,
                  &svt_aom_variance64x32_neon_dotprod),
    VarianceParam(64, 64, &svt_aom_variance64x64_c,
                  &svt_aom_variance64x64_neon_dotprod),
    VarianceParam(64, 128, &svt_aom_variance64x128_c,
                  &svt_aom_variance64x128_neon_dotprod),
    VarianceParam(128, 64, &svt_aom_variance128x64_c,
                  &svt_aom_variance128x64_neon_dotprod),
    VarianceParam(128, 128, &svt_aom_variance128x128_c,
                  &svt_aom_variance128x128_neon_dotprod)};

INSTANTIATE_TEST_SUITE_P(NEON_DOTPROD, VarianceTest,
                         ::testing::ValuesIn(variance_func_neon_dotprod));
#endif  // HAVE_NEON_DOTPROD

#endif  // ARCH_AARCH64

typedef unsigned int (*SubpixVarMxNFunc)(const uint8_t *a, int a_stride,
                                         int xoffset, int yoffset,
                                         const uint8_t *b, int b_stride,
                                         unsigned int *sse);

typedef std::tuple<uint32_t, uint32_t, SubpixVarMxNFunc, uint32_t,
                   SubpixVarMxNFunc>
    TestParams;

class SubpelVarianceTest : public ::testing::TestWithParam<TestParams> {
  public:
    SubpelVarianceTest()
        : log2width(TEST_GET_PARAM(0)),
          log2height(TEST_GET_PARAM(1)),
          func_tst(TEST_GET_PARAM(2)),
          use_high_bit_depth(TEST_GET_PARAM(3)),
          func_ref(TEST_GET_PARAM(4)) {
        width = 1 << log2width;
        height = 1 << log2height;
        block_size = width * height;
        if (use_high_bit_depth) {
            mask = (1u << static_cast<unsigned>(use_high_bit_depth)) - 1;
        } else {
            mask = (1u << EB_EIGHT_BIT) - 1;
        }

        if (!use_high_bit_depth) {
            src_ =
                reinterpret_cast<uint8_t *>(svt_aom_memalign(32, block_size));
            sec_ =
                reinterpret_cast<uint8_t *>(svt_aom_memalign(32, block_size));
            ref_ = reinterpret_cast<uint8_t *>(
                svt_aom_memalign(32, block_size + width + height + 1));
        } else {
            src_ = CONVERT_TO_BYTEPTR(reinterpret_cast<uint16_t *>(
                svt_aom_memalign(32, block_size * sizeof(uint16_t))));
            sec_ = CONVERT_TO_BYTEPTR(reinterpret_cast<uint16_t *>(
                svt_aom_memalign(32, block_size * sizeof(uint16_t))));
            ref_ = CONVERT_TO_BYTEPTR(svt_aom_memalign(
                32, (block_size + width + height + 1) * sizeof(uint16_t)));
        }
    }

    ~SubpelVarianceTest() {
        if (!use_high_bit_depth) {
            svt_aom_free(src_);
            svt_aom_free(ref_);
            svt_aom_free(sec_);
        } else {
            svt_aom_free(CONVERT_TO_SHORTPTR(src_));
            svt_aom_free(CONVERT_TO_SHORTPTR(ref_));
            svt_aom_free(CONVERT_TO_SHORTPTR(sec_));
        }
    }

  protected:
    void RefTest();
    void ExtremeRefTest();

    uint8_t *src_;
    uint8_t *ref_;
    uint8_t *sec_;
    int log2width, log2height;
    SubpixVarMxNFunc func_tst;
    bool use_high_bit_depth;
    SubpixVarMxNFunc func_ref;
    int width, height;
    int block_size;
    int32_t mask;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(SubpelVarianceTest);

void SubpelVarianceTest::RefTest() {
#if ARCH_AARCH64
    // Neon implementation of sub_pixel variance functions call variance
    // functions through the pointers setup by rtcd, so set them up properly.
    svt_aom_setup_rtcd_internal(EB_CPU_FLAGS_NEON);
#endif
    SVTRandom rnd_(0, mask);

    for (int x = 0; x < 8; ++x) {
        for (int y = 0; y < 8; ++y) {
            if (!use_high_bit_depth) {
                for (int j = 0; j < block_size; j++) {
                    src_[j] = rnd_.Rand8();
                }
                for (int j = 0; j < block_size + width + height + 1; j++) {
                    ref_[j] = rnd_.Rand8();
                }
            } else {
                for (int j = 0; j < block_size; j++) {
                    CONVERT_TO_SHORTPTR(src_)[j] = rnd_.Rand16() & mask;
                }
                for (int j = 0; j < block_size + width + height + 1; j++) {
                    CONVERT_TO_SHORTPTR(ref_)[j] = rnd_.Rand16() & mask;
                }
            }
            unsigned int sse1, sse2;
            unsigned int var1 =
                func_ref(ref_, width + 1, x, y, src_, width, &sse1);
            unsigned int var2 =
                func_tst(ref_, width + 1, x, y, src_, width, &sse2);
            EXPECT_EQ(sse1, sse2) << "at position " << x << ", " << y;
            EXPECT_EQ(var1, var2) << "at position " << x << ", " << y;
        }
    }
}

void SubpelVarianceTest::ExtremeRefTest() {
// TODO remove once PRs are merged
#if ARCH_AARCH64
    // Neon implementation of sub_pixel variance functions call variance
    // functions through the pointers setup by rtcd, so set them up properly.
    svt_aom_setup_rtcd_internal(EB_CPU_FLAGS_NEON);
#endif
    SVTRandom rnd_(0, mask);
    // Compare against reference.
    // Src: Set the first half of values to 0, the second half to the maximum.
    // Ref: Set the first half of values to the maximum, the second half to 0.
    for (int x = 0; x < 8; ++x) {
        for (int y = 0; y < 8; ++y) {
            const int half = block_size / 2;
            if (!use_high_bit_depth) {
                memset(src_, 0, half);
                memset(src_ + half, 255, half);
                memset(ref_, 255, half);
                memset(ref_ + half, 0, half + width + height + 1);
            } else {
                svt_aom_memset16(CONVERT_TO_SHORTPTR(src_), mask, half);
                svt_aom_memset16(CONVERT_TO_SHORTPTR(src_) + half, 0, half);
                svt_aom_memset16(CONVERT_TO_SHORTPTR(ref_), 0, half);
                svt_aom_memset16(CONVERT_TO_SHORTPTR(ref_) + half,
                                 mask,
                                 half + width + height + 1);
            }
            unsigned int sse1, sse2;
            unsigned int var1 =
                func_ref(ref_, width + 1, x, y, src_, width, &sse1);
            unsigned int var2 =
                func_tst(ref_, width + 1, x, y, src_, width, &sse2);
            EXPECT_EQ(sse1, sse2)
                << "for xoffset " << x << " and yoffset " << y;
            EXPECT_EQ(var1, var2)
                << "for xoffset " << x << " and yoffset " << y;
        }
    }
}

TEST_P(SubpelVarianceTest, Ref) {
    RefTest();
}
TEST_P(SubpelVarianceTest, ExtremeRef) {
    ExtremeRefTest();
}

#ifdef ARCH_X86_64
const TestParams kArraySubpelVariance_sse2[] = {
    // clang-format off
    { 7, 7, &svt_aom_sub_pixel_variance128x128_sse2, 0,
      &svt_aom_sub_pixel_variance128x128_c },
    { 7, 6, &svt_aom_sub_pixel_variance128x64_sse2, 0,
      &svt_aom_sub_pixel_variance128x64_c },
    { 6, 7, &svt_aom_sub_pixel_variance64x128_sse2, 0,
      &svt_aom_sub_pixel_variance64x128_c },
    { 6, 6, &svt_aom_sub_pixel_variance64x64_sse2, 0,
      &svt_aom_sub_pixel_variance64x64_c },
    { 6, 5, &svt_aom_sub_pixel_variance64x32_sse2, 0,
      &svt_aom_sub_pixel_variance64x32_c },
    { 5, 6, &svt_aom_sub_pixel_variance32x64_sse2, 0,
      &svt_aom_sub_pixel_variance32x64_c },
    { 5, 5, &svt_aom_sub_pixel_variance32x32_sse2, 0,
      &svt_aom_sub_pixel_variance32x32_c },
    { 5, 4, &svt_aom_sub_pixel_variance32x16_sse2, 0,
      &svt_aom_sub_pixel_variance32x16_c },
    { 4, 5, &svt_aom_sub_pixel_variance16x32_sse2, 0,
      &svt_aom_sub_pixel_variance16x32_c },
    { 4, 4, &svt_aom_sub_pixel_variance16x16_sse2, 0,
      &svt_aom_sub_pixel_variance16x16_c },
    { 4, 3, &svt_aom_sub_pixel_variance16x8_sse2, 0,
      &svt_aom_sub_pixel_variance16x8_c },
    { 3, 4, &svt_aom_sub_pixel_variance8x16_sse2, 0,
      &svt_aom_sub_pixel_variance8x16_c },
    { 3, 3, &svt_aom_sub_pixel_variance8x8_sse2, 0,
      &svt_aom_sub_pixel_variance8x8_c },
    { 3, 2, &svt_aom_sub_pixel_variance8x4_sse2, 0,
      &svt_aom_sub_pixel_variance8x4_c },
    { 2, 3, &svt_aom_sub_pixel_variance4x8_sse2, 0,
      &svt_aom_sub_pixel_variance4x8_c },
    { 2, 2, &svt_aom_sub_pixel_variance4x4_sse2, 0,
      &svt_aom_sub_pixel_variance4x4_c },
    { 6, 4, &svt_aom_sub_pixel_variance64x16_sse2, 0,
      &svt_aom_sub_pixel_variance64x16_c },
    { 4, 6, &svt_aom_sub_pixel_variance16x64_sse2, 0,
      &svt_aom_sub_pixel_variance16x64_c },
    { 5, 3, &svt_aom_sub_pixel_variance32x8_sse2, 0,
      &svt_aom_sub_pixel_variance32x8_c },
    { 3, 5, &svt_aom_sub_pixel_variance8x32_sse2, 0,
      &svt_aom_sub_pixel_variance8x32_c },
    { 4, 2, &svt_aom_sub_pixel_variance16x4_sse2, 0,
      &svt_aom_sub_pixel_variance16x4_c },
    { 2, 4, &svt_aom_sub_pixel_variance4x16_sse2, 0,
      &svt_aom_sub_pixel_variance4x16_c }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(SSE2, SubpelVarianceTest,
                         ::testing::ValuesIn(kArraySubpelVariance_sse2));

const TestParams kArraySubpelVariance_ssse3[] = {
    // clang-format off
    { 7, 7, &svt_aom_sub_pixel_variance128x128_ssse3, 0,
      &svt_aom_sub_pixel_variance128x128_c },
    { 7, 6, &svt_aom_sub_pixel_variance128x64_ssse3, 0,
      &svt_aom_sub_pixel_variance128x64_c },
    { 6, 7, &svt_aom_sub_pixel_variance64x128_ssse3, 0,
      &svt_aom_sub_pixel_variance64x128_c },
    { 6, 6, &svt_aom_sub_pixel_variance64x64_ssse3, 0,
      &svt_aom_sub_pixel_variance64x64_c },
    { 6, 5, &svt_aom_sub_pixel_variance64x32_ssse3, 0,
      &svt_aom_sub_pixel_variance64x32_c },
    { 5, 6, &svt_aom_sub_pixel_variance32x64_ssse3, 0,
      &svt_aom_sub_pixel_variance32x64_c },
    { 5, 5, &svt_aom_sub_pixel_variance32x32_ssse3, 0,
      &svt_aom_sub_pixel_variance32x32_c },
    { 5, 4, &svt_aom_sub_pixel_variance32x16_ssse3, 0,
      &svt_aom_sub_pixel_variance32x16_c },
    { 4, 5, &svt_aom_sub_pixel_variance16x32_ssse3, 0,
      &svt_aom_sub_pixel_variance16x32_c },
    { 4, 4, &svt_aom_sub_pixel_variance16x16_ssse3, 0,
      &svt_aom_sub_pixel_variance16x16_c },
    { 4, 3, &svt_aom_sub_pixel_variance16x8_ssse3, 0,
      &svt_aom_sub_pixel_variance16x8_c },
    { 3, 4, &svt_aom_sub_pixel_variance8x16_ssse3, 0,
      &svt_aom_sub_pixel_variance8x16_c },
    { 3, 3, &svt_aom_sub_pixel_variance8x8_ssse3, 0,
      &svt_aom_sub_pixel_variance8x8_c },
    { 3, 2, &svt_aom_sub_pixel_variance8x4_ssse3, 0,
      &svt_aom_sub_pixel_variance8x4_c },
    { 2, 3, &svt_aom_sub_pixel_variance4x8_ssse3, 0,
      &svt_aom_sub_pixel_variance4x8_c },
    { 2, 2, &svt_aom_sub_pixel_variance4x4_ssse3, 0,
      &svt_aom_sub_pixel_variance4x4_c },
    { 6, 4, &svt_aom_sub_pixel_variance64x16_ssse3, 0,
      &svt_aom_sub_pixel_variance64x16_c },
    { 4, 6, &svt_aom_sub_pixel_variance16x64_ssse3, 0,
      &svt_aom_sub_pixel_variance16x64_c },
    { 5, 3, &svt_aom_sub_pixel_variance32x8_ssse3, 0,
      &svt_aom_sub_pixel_variance32x8_c },
    { 3, 5, &svt_aom_sub_pixel_variance8x32_ssse3, 0,
      &svt_aom_sub_pixel_variance8x32_c },
    { 4, 2, &svt_aom_sub_pixel_variance16x4_ssse3, 0,
      &svt_aom_sub_pixel_variance16x4_c },
    { 2, 4, &svt_aom_sub_pixel_variance4x16_ssse3, 0,
      &svt_aom_sub_pixel_variance4x16_c }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(SSSE3, SubpelVarianceTest,
                         ::testing::ValuesIn(kArraySubpelVariance_ssse3));

const TestParams kArraySubpelVariance_avx2[] = {
    // clang-format off
    { 7, 7, &svt_aom_sub_pixel_variance128x128_avx2, 0,
      &svt_aom_sub_pixel_variance128x128_c },
    { 7, 6, &svt_aom_sub_pixel_variance128x64_avx2, 0,
      &svt_aom_sub_pixel_variance128x64_c },
    { 6, 7, &svt_aom_sub_pixel_variance64x128_avx2, 0,
      &svt_aom_sub_pixel_variance64x128_c },
    { 6, 6, &svt_aom_sub_pixel_variance64x64_avx2, 0,
      &svt_aom_sub_pixel_variance64x64_c },
    { 6, 5, &svt_aom_sub_pixel_variance64x32_avx2, 0,
      &svt_aom_sub_pixel_variance64x32_c },
    { 5, 6, &svt_aom_sub_pixel_variance32x64_avx2, 0,
      &svt_aom_sub_pixel_variance32x64_c },
    { 5, 5, &svt_aom_sub_pixel_variance32x32_avx2, 0,
      &svt_aom_sub_pixel_variance32x32_c },
    { 5, 4, &svt_aom_sub_pixel_variance32x16_avx2, 0,
      &svt_aom_sub_pixel_variance32x16_c },
    { 4, 5, &svt_aom_sub_pixel_variance16x32_avx2, 0,
      &svt_aom_sub_pixel_variance16x32_c },
    { 4, 4, &svt_aom_sub_pixel_variance16x16_avx2, 0,
      &svt_aom_sub_pixel_variance16x16_c },
    { 4, 3, &svt_aom_sub_pixel_variance16x8_avx2, 0,
      &svt_aom_sub_pixel_variance16x8_c },
    { 4, 6, &svt_aom_sub_pixel_variance16x64_avx2, 0,
      &svt_aom_sub_pixel_variance16x64_c },
    { 4, 2, &svt_aom_sub_pixel_variance16x4_avx2, 0,
      &svt_aom_sub_pixel_variance16x4_c }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(AVX2, SubpelVarianceTest,
                         ::testing::ValuesIn(kArraySubpelVariance_avx2));

#if EN_AVX512_SUPPORT
const TestParams kArraySubpelVariance_avx512[] = {
    // clang-format off
    { 7, 7, &svt_aom_sub_pixel_variance128x128_avx512, 0,
      &svt_aom_sub_pixel_variance128x128_c },
    { 7, 6, &svt_aom_sub_pixel_variance128x64_avx512, 0,
      &svt_aom_sub_pixel_variance128x64_c },
    { 6, 7, &svt_aom_sub_pixel_variance64x128_avx512, 0,
      &svt_aom_sub_pixel_variance64x128_c },
    { 6, 6, &svt_aom_sub_pixel_variance64x64_avx512, 0,
      &svt_aom_sub_pixel_variance64x64_c },
    { 6, 5, &svt_aom_sub_pixel_variance64x32_avx512, 0,
      &svt_aom_sub_pixel_variance64x32_c },
    { 5, 6, &svt_aom_sub_pixel_variance32x64_avx512, 0,
      &svt_aom_sub_pixel_variance32x64_c },
    { 5, 5, &svt_aom_sub_pixel_variance32x32_avx512, 0,
      &svt_aom_sub_pixel_variance32x32_c },
    { 5, 4, &svt_aom_sub_pixel_variance32x16_avx512, 0,
      &svt_aom_sub_pixel_variance32x16_c }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(AVX512, SubpelVarianceTest,
                         ::testing::ValuesIn(kArraySubpelVariance_avx512));
#endif
#endif  // ARCH_X86_64

#if ARCH_AARCH64
const TestParams kArraySubpelVariance_neon[] = {
    // clang-format off
    { 7, 7, &svt_aom_sub_pixel_variance128x128_neon, 0,
      &svt_aom_sub_pixel_variance128x128_c },
    { 7, 6, &svt_aom_sub_pixel_variance128x64_neon, 0,
      &svt_aom_sub_pixel_variance128x64_c },
    { 6, 7, &svt_aom_sub_pixel_variance64x128_neon, 0,
      &svt_aom_sub_pixel_variance64x128_c },
    { 6, 6, &svt_aom_sub_pixel_variance64x64_neon, 0,
      &svt_aom_sub_pixel_variance64x64_c },
    { 6, 5, &svt_aom_sub_pixel_variance64x32_neon, 0,
      &svt_aom_sub_pixel_variance64x32_c },
    { 5, 6, &svt_aom_sub_pixel_variance32x64_neon, 0,
      &svt_aom_sub_pixel_variance32x64_c },
    { 5, 5, &svt_aom_sub_pixel_variance32x32_neon, 0,
      &svt_aom_sub_pixel_variance32x32_c },
    { 5, 4, &svt_aom_sub_pixel_variance32x16_neon, 0,
      &svt_aom_sub_pixel_variance32x16_c },
    { 4, 5, &svt_aom_sub_pixel_variance16x32_neon, 0,
      &svt_aom_sub_pixel_variance16x32_c },
    { 4, 4, &svt_aom_sub_pixel_variance16x16_neon, 0,
      &svt_aom_sub_pixel_variance16x16_c },
    { 4, 3, &svt_aom_sub_pixel_variance16x8_neon, 0,
      &svt_aom_sub_pixel_variance16x8_c },
    { 3, 4, &svt_aom_sub_pixel_variance8x16_neon, 0,
      &svt_aom_sub_pixel_variance8x16_c },
    { 3, 3, &svt_aom_sub_pixel_variance8x8_neon, 0,
      &svt_aom_sub_pixel_variance8x8_c },
    { 3, 2, &svt_aom_sub_pixel_variance8x4_neon, 0,
      &svt_aom_sub_pixel_variance8x4_c },
    { 2, 3, &svt_aom_sub_pixel_variance4x8_neon, 0,
      &svt_aom_sub_pixel_variance4x8_c },
    { 2, 2, &svt_aom_sub_pixel_variance4x4_neon, 0,
      &svt_aom_sub_pixel_variance4x4_c },
    { 6, 4, &svt_aom_sub_pixel_variance64x16_neon, 0,
      &svt_aom_sub_pixel_variance64x16_c },
    { 4, 6, &svt_aom_sub_pixel_variance16x64_neon, 0,
      &svt_aom_sub_pixel_variance16x64_c },
    { 5, 3, &svt_aom_sub_pixel_variance32x8_neon, 0,
      &svt_aom_sub_pixel_variance32x8_c },
    { 3, 5, &svt_aom_sub_pixel_variance8x32_neon, 0,
      &svt_aom_sub_pixel_variance8x32_c },
    { 4, 2, &svt_aom_sub_pixel_variance16x4_neon, 0,
      &svt_aom_sub_pixel_variance16x4_c },
    { 2, 4, &svt_aom_sub_pixel_variance4x16_neon, 0,
      &svt_aom_sub_pixel_variance4x16_c }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(NEON, SubpelVarianceTest,
                         ::testing::ValuesIn(kArraySubpelVariance_neon));

#if HAVE_NEON_DOTPROD
const TestParams kArraySubpelVariance_neon_dotprod[] = {
    // clang-format off
    { 7, 7, &svt_aom_sub_pixel_variance128x128_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance128x128_c },
    { 7, 6, &svt_aom_sub_pixel_variance128x64_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance128x64_c },
    { 6, 7, &svt_aom_sub_pixel_variance64x128_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance64x128_c },
    { 6, 6, &svt_aom_sub_pixel_variance64x64_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance64x64_c },
    { 6, 5, &svt_aom_sub_pixel_variance64x32_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance64x32_c },
    { 5, 6, &svt_aom_sub_pixel_variance32x64_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance32x64_c },
    { 5, 5, &svt_aom_sub_pixel_variance32x32_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance32x32_c },
    { 5, 4, &svt_aom_sub_pixel_variance32x16_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance32x16_c },
    { 4, 5, &svt_aom_sub_pixel_variance16x32_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance16x32_c },
    { 4, 4, &svt_aom_sub_pixel_variance16x16_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance16x16_c },
    { 4, 3, &svt_aom_sub_pixel_variance16x8_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance16x8_c },
    { 3, 4, &svt_aom_sub_pixel_variance8x16_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance8x16_c },
    { 3, 3, &svt_aom_sub_pixel_variance8x8_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance8x8_c },
    { 3, 2, &svt_aom_sub_pixel_variance8x4_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance8x4_c },
    { 2, 3, &svt_aom_sub_pixel_variance4x8_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance4x8_c },
    { 6, 4, &svt_aom_sub_pixel_variance64x16_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance64x16_c },
    { 4, 6, &svt_aom_sub_pixel_variance16x64_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance16x64_c },
    { 5, 3, &svt_aom_sub_pixel_variance32x8_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance32x8_c },
    { 3, 5, &svt_aom_sub_pixel_variance8x32_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance8x32_c },
    { 4, 2, &svt_aom_sub_pixel_variance16x4_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance16x4_c },
    { 2, 4, &svt_aom_sub_pixel_variance4x16_neon_dotprod, 0,
      &svt_aom_sub_pixel_variance4x16_c }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, SubpelVarianceTest,
    ::testing::ValuesIn(kArraySubpelVariance_neon_dotprod));
#endif  // HAVE_NEON_DOTPROD
#endif  // ARCH_AARCH64

}  // namespace
