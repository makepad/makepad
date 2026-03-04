/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
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
#include "acm_random.h"
#include "svt_time.h"
#include "util.h"
#include "utility.h"

using libaom_test::ACMRandom;
using std::make_tuple;

namespace {
const int kNumIterations = 1000;

using BlockErrorFunc = int64_t (*)(const TranLow *coeff, const TranLow *dqcoeff,
                                   intptr_t block_size, int64_t *ssz);

class BlockErrorTest : public ::testing::TestWithParam<BlockErrorFunc> {
  public:
    BlockErrorTest() : test_func_(GetParam()) {
    }

    ~BlockErrorTest() override = default;

  protected:
    BlockErrorFunc test_func_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(BlockErrorTest);

TEST_P(BlockErrorTest, OperationCheck) {
    ACMRandom rnd(ACMRandom::DeterministicSeed());
    DECLARE_ALIGNED(16, TranLow, coeff[4096]);
    DECLARE_ALIGNED(16, TranLow, dqcoeff[4096]);
    int err_count_total = 0;
    int first_failure = -1;
    intptr_t block_size;
    int64_t ssz;
    int64_t ret;
    int64_t ref_ssz;
    int64_t ref_ret;
    int bit_depth = 8;
    const int msb = bit_depth + 8 - 1;
    for (int i = 0; i < kNumIterations; ++i) {
        int err_count = 0;
        block_size = 16 << (i % 9);  // All block sizes from 4x4, 8x4 ..64x64
        for (int j = 0; j < block_size; j++) {
            // coeff and dqcoeff will always have at least the same sign, and
            // this can be used for optimization, so generate test input
            // precisely.
            if (rnd(2)) {
                // Positive number
                coeff[j] = rnd(1 << msb);
                dqcoeff[j] = rnd(1 << msb);
            } else {
                // Negative number
                coeff[j] = -rnd(1 << msb);
                dqcoeff[j] = -rnd(1 << msb);
            }
        }
        ref_ret = svt_av1_block_error_c(coeff, dqcoeff, block_size, &ref_ssz);
        ret = test_func_(coeff, dqcoeff, block_size, &ssz);

        err_count += (ref_ret != ret) | (ref_ssz != ssz);
        if (err_count && !err_count_total) {
            first_failure = i;
        }
        err_count_total += err_count;
    }
    EXPECT_EQ(0, err_count_total)
        << "Error: Error Block Test, C output doesn't match optimized output. "
        << "First failed at test case " << first_failure;
}

TEST_P(BlockErrorTest, ExtremeValues) {
    ACMRandom rnd(ACMRandom::DeterministicSeed());
    DECLARE_ALIGNED(16, TranLow, coeff[4096]);
    DECLARE_ALIGNED(16, TranLow, dqcoeff[4096]);
    int err_count_total = 0;
    int first_failure = -1;
    int bit_depth = 8;
    intptr_t block_size;
    int64_t ssz;
    int64_t ret;
    int64_t ref_ssz;
    int64_t ref_ret;
    const int msb = bit_depth + 8 - 1;
    int max_val = ((1 << msb) - 1);
    for (int i = 0; i < kNumIterations; ++i) {
        int err_count = 0;
        int k = (i / 9) % 9;

        // Change the maximum coeff value, to test different bit boundaries
        if (k == 8 && (i % 9) == 0) {
            max_val >>= 1;
        }
        block_size = 16 << (i % 9);  // All block sizes from 4x4, 8x4 ..64x64
        for (int j = 0; j < block_size; j++) {
            if (k < 4) {
                // Test at positive maximum values
                coeff[j] = k % 2 ? max_val : 0;
                dqcoeff[j] = (k >> 1) % 2 ? max_val : 0;
            } else if (k < 8) {
                // Test at negative maximum values
                coeff[j] = k % 2 ? -max_val : 0;
                dqcoeff[j] = (k >> 1) % 2 ? -max_val : 0;
            } else {
                if (rnd(2)) {
                    // Positive number
                    coeff[j] = rnd(1 << 14);
                    dqcoeff[j] = rnd(1 << 14);
                } else {
                    // Negative number
                    coeff[j] = -rnd(1 << 14);
                    dqcoeff[j] = -rnd(1 << 14);
                }
            }
        }
        ref_ret = svt_av1_block_error_c(coeff, dqcoeff, block_size, &ref_ssz);
        ret = test_func_(coeff, dqcoeff, block_size, &ssz);
        err_count += (ref_ret != ret) | (ref_ssz != ssz);
        if (err_count && !err_count_total) {
            first_failure = i;
        }
        err_count_total += err_count;
    }
    EXPECT_EQ(0, err_count_total)
        << "Error: Error Block Test, C output doesn't match optimized output. "
        << "First failed at test case " << first_failure;
}

TEST_P(BlockErrorTest, DISABLED_Speed) {
    ACMRandom rnd(ACMRandom::DeterministicSeed());
    DECLARE_ALIGNED(16, TranLow, coeff[4096]);
    DECLARE_ALIGNED(16, TranLow, dqcoeff[4096]);
    intptr_t block_size;
    int64_t ssz;
    int num_iters = 100000;
    int64_t ref_ssz;
    int bit_depth = 8;
    const int msb = bit_depth + 8 - 1;
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;

    for (int i = 0; i < 9; ++i) {
        block_size = 16 << (i % 9);  // All block sizes from 4x4, 8x4 ..64x64
        for (int k = 0; k < 9; k++) {
            for (int j = 0; j < block_size; j++) {
                if (k < 5) {
                    if (rnd(2)) {
                        // Positive number
                        coeff[j] = rnd(1 << msb);
                        dqcoeff[j] = rnd(1 << msb);
                    } else {
                        // Negative number
                        coeff[j] = -rnd(1 << msb);
                        dqcoeff[j] = -rnd(1 << msb);
                    }
                } else {
                    if (rnd(2)) {
                        // Positive number
                        coeff[j] = rnd(1 << 14);
                        dqcoeff[j] = rnd(1 << 14);
                    } else {
                        // Negative number
                        coeff[j] = -rnd(1 << 14);
                        dqcoeff[j] = -rnd(1 << 14);
                    }
                }
            }

            svt_av1_get_time(&start_time_seconds, &start_time_useconds);
            for (int iter = 0; iter < num_iters; ++iter) {
                svt_av1_block_error_c(coeff, dqcoeff, block_size, &ref_ssz);
            }
            svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
            time_c =
                svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                        start_time_useconds,
                                                        finish_time_seconds,
                                                        finish_time_useconds);

            svt_av1_get_time(&start_time_seconds, &start_time_useconds);
            for (int iter = 0; iter < num_iters; ++iter) {
                test_func_(coeff, dqcoeff, block_size, &ssz);
            }
            svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
            time_o =
                svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                        start_time_useconds,
                                                        finish_time_seconds,
                                                        finish_time_useconds);

            printf(
                " c_time=%6.2f \t simd_time=%6.2f \t "
                "gain=%5.2f \n",
                time_c,
                time_o,
                (time_c / time_o));
        }
    }
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(AVX2, BlockErrorTest,
                         ::testing::Values(svt_av1_block_error_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, BlockErrorTest,
                         ::testing::Values(svt_av1_block_error_neon));
#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(SVE, BlockErrorTest,
                         ::testing::Values(svt_av1_block_error_sve));
#endif  // HAVE_SVE
#endif  // HAVE_AARCH64
}  // namespace
