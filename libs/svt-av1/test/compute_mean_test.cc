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
 * @file compute_mean_test.cc
 *
 * @brief Unit test for compute mean function:
 * - compute_mean8x8_sse2_intrin
 * - svt_compute_mean_of_squared_values8x8_sse2_intrin
 * - compute_sub_mean8x8_sse2_intrin
 * - compute_mean8x8_avx2_intrin
 * - svt_compute_interm_var_four8x8_avx2_intrin
 *
 * @author Cidana-Edmond,Cidana-Ivy
 *
 ******************************************************************************/

#include <sstream>
#include "gtest/gtest.h"

#include "aom_dsp_rtcd.h"
#include "compute_mean.h"
#include "random.h"
#include "util.h"
#include "utility.h"
/**
 * @brief Unit test for compute mean function:
 * - compute_mean8x8_sse2_intrin
 * - svt_compute_mean_of_squared_values8x8_sse2_intrin
 * - svt_compute_sub_mean8x8_sse2_intrin
 * - compute_mean8x8_avx2_intrin
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check test output and reference output.
 *
 * Expected result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * data buffer:
 * Buffer is filled with test data. The values of data in normal test are the
 * random 8-bit integer, and in boundary test are the large random integer
 * between 0xE0 and 0xFF
 */

namespace {

using svt_av1_test_tool::SVTRandom;

static const int block_size = 8 * 8;
static const int test_times = 10000;

static const uint8_t* prepare_data_8x8(uint8_t* data, SVTRandom* rnd) {
    for (size_t i = 0; i < block_size; i++) {
        data[i] = (uint8_t)rnd->random();
    }
    return data;
}

typedef uint64_t (*compute_mean)(uint8_t* input_samples, uint32_t input_stride,
                                 uint32_t input_area_width,
                                 uint32_t input_area_height);

template <compute_mean ref_impl>
class ComputeMeanTest : public ::testing::TestWithParam<compute_mean> {
  public:
    ComputeMeanTest() : test_impl_(GetParam()) {
    }

    void SetUp() override {
        input_ = (uint8_t*)svt_aom_memalign(8, block_size * sizeof(uint8_t));
    }

    void TearDown() override {
        svt_aom_free(input_);
    }

    void run_test() {
        SVTRandom rnd[2] = {
            SVTRandom(8, false),   // Random generator of normal test vector.
            SVTRandom(0xE0, 0xFF)  // Random generator of boundary test vector.
        };
        for (size_t vi = 0; vi < 2; vi++) {
            for (int i = 0; i < test_times; i++) {
                prepare_data_8x8(input_, &rnd[vi]);

                uint64_t ref_mean = ref_impl(input_, 8, 8, 8);
                uint64_t test_mean = test_impl_(input_, 8, 8, 8);

                // Compare results.
                ASSERT_EQ(test_mean, ref_mean)
                    << "compute mean with ref failed!\n";
            }
        }
    }

  private:
    static const int block_size = 8 * 8;
    compute_mean test_impl_;
    uint8_t* input_;
};

using ComputeMeanValueTest = ComputeMeanTest<svt_compute_mean_c>;
using ComputeMeanSquaredValueTest =
    ComputeMeanTest<svt_compute_mean_squared_values_c>;

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(ComputeMeanValueTest);
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(ComputeMeanSquaredValueTest);

TEST_P(ComputeMeanValueTest, MatchTest) {
    run_test();
}
TEST_P(ComputeMeanSquaredValueTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE2, ComputeMeanValueTest,
                         ::testing::Values(svt_compute_mean8x8_sse2_intrin));

INSTANTIATE_TEST_SUITE_P(
    SSE2, ComputeMeanSquaredValueTest,
    ::testing::Values(svt_compute_mean_of_squared_values8x8_sse2_intrin));

INSTANTIATE_TEST_SUITE_P(AVX2, ComputeMeanValueTest,
                         ::testing::Values(svt_compute_mean8x8_avx2_intrin));
#endif  // ARCH_X86_64

typedef uint64_t (*compute_mean8x8)(uint8_t* input_samples,
                                    uint16_t input_stride);

class ComputeMean8x8Test : public ::testing::TestWithParam<compute_mean8x8> {
  public:
    ComputeMean8x8Test() : test_impl_(GetParam()) {
    }

    void SetUp() override {
        input_ = (uint8_t*)svt_aom_memalign(8, block_size * sizeof(uint8_t));
    }

    void TearDown() override {
        svt_aom_free(input_);
    }

    void run_test() {
        SVTRandom rnd[2] = {
            SVTRandom(8, false),   // Random generator of normal test vector.
            SVTRandom(0xE0, 0xFF)  // Random generator of boundary test vector.
        };

        for (size_t vi = 0; vi < 2; vi++) {
            for (int i = 0; i < test_times; i++) {
                prepare_data_8x8(input_, &rnd[vi]);

                uint64_t ref_mean = svt_compute_sub_mean_8x8_c(input_, 8);
                uint64_t test_mean = test_impl_(input_, 8);

                // Compare results.
                ASSERT_EQ(test_mean, ref_mean)
                    << "compute mean with ref failed!\n";
            }
        }
    }

  private:
    static const int block_size = 8 * 8;
    compute_mean8x8 test_impl_;
    uint8_t* input_;
};

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(ComputeMean8x8Test);

TEST_P(ComputeMean8x8Test, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, ComputeMean8x8Test,
    ::testing::Values(svt_compute_sub_mean8x8_sse2_intrin));
#endif  // ARCH_X86_64

typedef void (*compute_mean_four8x8)(uint8_t* input_samples,
                                     uint16_t input_stride,
                                     uint64_t* mean_of8x8_blocks,
                                     uint64_t* mean_of_squared8x8_blocks);

class ComputeMeanFour8x8Test
    : public ::testing::TestWithParam<compute_mean_four8x8> {
  public:
    ComputeMeanFour8x8Test() : test_impl_(GetParam()) {
    }

    void SetUp() override {
        input_ = (uint8_t*)svt_aom_memalign(8, block_size * sizeof(uint8_t));
    }

    void TearDown() override {
        svt_aom_free(input_);
    }

    void run_test() {
        SVTRandom rnd[2] = {
            SVTRandom(8, false),   // Random generator of normal test vector.
            SVTRandom(0xE0, 0xFF)  // Random generator of boundary test vector.
        };
        uint64_t output_ref[num_block];
        uint64_t output_squared_ref[num_block];
        uint64_t output_tst[num_block];
        uint64_t output_squared_tst[num_block];

        for (size_t vi = 0; vi < 2; vi++) {
            for (int k = 0; k < test_times; k++) {
                prepare_data_8x8(input_, &rnd[vi]);

                svt_compute_interm_var_four8x8_c(
                    input_, 8, output_ref, output_squared_ref);

                test_impl_(input_, 8, output_tst, output_squared_tst);

                // Compare results.
                for (size_t i = 0; i < num_block; ++i) {
                    EXPECT_EQ(output_tst[i], output_ref[i])
                        << "compare mean of 8x8 block error, mismatch at index "
                        << i << ": Reference = " << output_ref[i]
                        << ", Test = " << output_tst[i];
                }

                for (size_t i = 0; i < num_block; ++i) {
                    EXPECT_EQ(output_squared_tst[i], output_squared_ref[i])
                        << "compare mean of 8x8 squared block error, mismatch "
                           "at index "
                        << i << ": Reference = " << output_squared_ref[i]
                        << ", Test = " << output_squared_tst[i];
                }
            }
        }
    }

  private:
    static const int block_size = 8 * 8;
    static const int num_block = 4;
    compute_mean_four8x8 test_impl_;
    uint8_t* input_;
};

TEST_P(ComputeMeanFour8x8Test, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, ComputeMeanFour8x8Test,
    ::testing::Values(svt_compute_interm_var_four8x8_avx2_intrin));

INSTANTIATE_TEST_SUITE_P(
    SSE2, ComputeMeanFour8x8Test,
    ::testing::Values(svt_compute_interm_var_four8x8_helper_sse2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, ComputeMeanFour8x8Test,
    ::testing::Values(svt_compute_interm_var_four8x8_neon));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, ComputeMeanFour8x8Test,
    ::testing::Values(svt_compute_interm_var_four8x8_neon_dotprod));
#endif  // HAVE_NEON_DOTPROD
#endif  // ARCH_AARCH64

}  // namespace
