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
 * @file EncodeTxbAsmTest.cc
 *
 * @brief Unit test for svt_av1_txb_init_levels_avx2:
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/

#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <random>

#include "gtest/gtest.h"

#include "definitions.h"
#include "transforms.h"
#include "util.h"
#include "aom_dsp_rtcd.h"
#include "TxfmCommon.h"
#include "random.h"
#include "svt_time.h"
#include "encode_txb_ref_c.h"

using svt_av1_test_tool::SVTRandom;  // to generate the random

/** setup_test_env and reset_test_env are implemented in test/TestEnv.c */
extern "C" void setup_test_env();
extern "C" void reset_test_env();

namespace {

static INLINE uint8_t *set_levels(uint8_t *const levels_buf,
                                  const int32_t width) {
    return levels_buf + TX_PAD_TOP * (width + TX_PAD_HOR);
}

static INLINE int get_padded_idx(const int idx, const int bwl) {
    return idx + ((idx >> bwl) << TX_PAD_HOR_LOG2);
}

// test assembly code of av1_get_nz_map_contexts
extern "C" void svt_av1_get_nz_map_contexts_sse2(
    const uint8_t *const levels, const int16_t *const scan, const uint16_t eob,
    TxSize tx_size, const TxClass tx_class, int8_t *const coeff_contexts);
using GetNzMapContextsFunc = void (*)(const uint8_t *const levels,
                                      const int16_t *const scan,
                                      const uint16_t eob, const TxSize tx_size,
                                      const TxClass tx_class,
                                      int8_t *const coeff_contexts);
using GetNzMapContextParam = std::tuple<GetNzMapContextsFunc, int, int>;

class EncodeTxbTest : public ::testing::TestWithParam<GetNzMapContextParam> {
  public:
    EncodeTxbTest()
        : level_rnd_(0, INT8_MAX),
          coeff_rnd_(0, UINT8_MAX),
          ref_func_(&svt_av1_get_nz_map_contexts_c) {
    }

    virtual ~EncodeTxbTest() {
    }

    void check_get_nz_map_context_assembly(GetNzMapContextsFunc test_func,
                                           const int tx_type,
                                           const int tx_size) {
        const int num_tests = 10;
        const TxClass tx_class = tx_type_to_class[tx_type];

        const int bwl = get_txb_bwl((TxSize)tx_size);
        const int width = get_txb_wide((TxSize)tx_size);
        const int height = get_txb_high((TxSize)tx_size);
        const int real_width = tx_size_wide[tx_size];
        const int real_height = tx_size_high[tx_size];
        const int16_t *const scan = get_scan_order(tx_size, tx_type)->scan;

        levels_ = set_levels(levels_buf_, width);
        for (int i = 0; i < num_tests; ++i) {
            for (int eob = 1; eob <= width * height; ++eob) {
                init_levels(scan, bwl, eob);
                init_coeff_contexts(scan, bwl, eob);

                ref_func_(levels_,
                          scan,
                          eob,
                          (TxSize)tx_size,
                          tx_class,
                          coeff_contexts_ref_);
                test_func(levels_,
                          scan,
                          eob,
                          (TxSize)tx_size,
                          tx_class,
                          coeff_contexts_test_);

                for (int j = 0; j < eob; ++j) {
                    const int pos = scan[j];
                    ASSERT_EQ(coeff_contexts_ref_[pos],
                              coeff_contexts_test_[pos])
                        << " tx_class " << tx_class << " width " << real_width
                        << " height " << real_height << " eob " << eob;
                }
            }
        }
    }

  private:
    void init_levels(const int16_t *const scan, const int bwl, const int eob) {
        memset(levels_buf_, 0, sizeof(levels_buf_));
        for (int c = 0; c < eob; ++c) {
            levels_[get_padded_idx(scan[c], bwl)] =
                static_cast<uint8_t>(level_rnd_.random());
        }
    }

    void init_coeff_contexts(const int16_t *const scan, const int /*bwl*/,
                             const int eob) {
        memset(coeff_contexts_test_,
               0,
               sizeof(*coeff_contexts_test_) * MAX_TX_SQUARE);
        memset(coeff_contexts_ref_,
               0,
               sizeof(*coeff_contexts_ref_) * MAX_TX_SQUARE);
        // Generate oppsite value for these two buffers
        for (int c = 0; c < eob; ++c) {
            const int pos = scan[c];
            coeff_contexts_test_[pos] = coeff_rnd_.random() - INT8_MAX;
            coeff_contexts_ref_[pos] = -coeff_contexts_test_[pos];
        }
    }

    SVTRandom level_rnd_;
    SVTRandom coeff_rnd_;
    uint8_t levels_buf_[TX_PAD_2D];
    uint8_t *levels_;
    DECLARE_ALIGNED(16, int8_t, coeff_contexts_ref_[MAX_TX_SQUARE]);
    DECLARE_ALIGNED(16, int8_t, coeff_contexts_test_[MAX_TX_SQUARE]);
    const GetNzMapContextsFunc ref_func_;
};

TEST_P(EncodeTxbTest, GetNzMapTest) {
    check_get_nz_map_context_assembly(
        TEST_GET_PARAM(0), TEST_GET_PARAM(1), TEST_GET_PARAM(2));
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, EncodeTxbTest,
    ::testing::Combine(::testing::Values(&svt_av1_get_nz_map_contexts_sse2),
                       ::testing::Range(0, static_cast<int>(TX_TYPES), 1),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, EncodeTxbTest,
    ::testing::Combine(::testing::Values(&svt_av1_get_nz_map_contexts_avx2),
                       ::testing::Range(0, static_cast<int>(TX_TYPES), 1),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, EncodeTxbTest,
    ::testing::Combine(::testing::Values(&svt_av1_get_nz_map_contexts_neon),
                       ::testing::Range(0, static_cast<int>(TX_TYPES), 1),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));
#endif  // ARCH_AARCH64

// test assembly code of svt_av1_txb_init_levels
using TxbInitLevelsFunc = void (*)(const TranLow *const coeff, const int width,
                                   const int height, uint8_t *const levels);
using TxbInitLevelParam = std::tuple<TxbInitLevelsFunc, int>;
/**
 * @brief Unit test for svt_av1_txb_init_levels_avx2:
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check the difference between test output
 * and reference output.
 *
 * Expect result:
 * Output from assemble function should be exactly same as output from c.
 *
 * Test coverage:
 * Input buffer: Fill with random values
 * width: deduced from valid tx_size
 * height: deduced from valid tx_size
 *
 */
class EncodeTxbInitLevelTest
    : public ::testing::TestWithParam<TxbInitLevelParam> {
  public:
    EncodeTxbInitLevelTest() : ref_func_(&svt_av1_txb_init_levels_c) {
        // fill input_coeff_ with 16bit signed random
        // The random is only used in prepare_data function, however
        // we should not declare in that function, otherwise
        // we will get repeated random numbers.
        rnd_ = new SVTRandom(16, true);
    }

    virtual ~EncodeTxbInitLevelTest() {
        delete rnd_;
    }

    void SetUp() {
        reset_test_env();
    }

    void run_test(const TxbInitLevelsFunc test_func, const int tx_size,
                  const bool is_speed) {
        const int width = get_txb_wide((TxSize)tx_size);
        const int height = get_txb_high((TxSize)tx_size);
        const uint64_t num_loop =
            is_speed ? (50000000000 / (width * height)) : 1;
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        ASSERT_NE(rnd_, nullptr) << "Fail to create SVTRandom";

        // prepare data, same input, differente output by default.
        prepare_data(tx_size);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            ref_func_(input_coeff_, width, height, levels_ref_);

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            test_func(input_coeff_, width, height, levels_test_);

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        // compare the result
        const int stride = width + TX_PAD_HOR;
        for (int r = 0; r < height + TX_PAD_VER; ++r) {
            for (int c = 0; c < stride; ++c) {
                ASSERT_EQ(levels_buf_test_[c + r * stride],
                          levels_buf_ref_[c + r * stride])
                    << "[" << r << "," << c << "] " << width << "x" << height;
            }
        }

        if (is_speed) {
            time_c =
                svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                        start_time_useconds,
                                                        middle_time_seconds,
                                                        middle_time_useconds);
            time_o =
                svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                        middle_time_useconds,
                                                        finish_time_seconds,
                                                        finish_time_useconds);
            printf("svt_av1_txb_init_levels(%2dx%2d): %6.2f\n",
                   width,
                   height,
                   time_c / time_o);
        }
    }

  private:
    void prepare_data(int tx_size) {
        const int width = get_txb_wide((TxSize)tx_size);
        const int height = get_txb_high((TxSize)tx_size);

        // make output different by default
        memset(levels_buf_test_, 111, sizeof(levels_buf_test_));
        memset(levels_buf_ref_, 128, sizeof(levels_buf_test_));

        // fill the input buffer with random
        levels_test_ = set_levels(levels_buf_test_, width);
        levels_ref_ = set_levels(levels_buf_ref_, width);

        for (int i = 0; i < width * height; i++) {
            input_coeff_[i] = rnd_->random();
        }
    }

  private:
    SVTRandom *rnd_;
    uint8_t levels_buf_test_[TX_PAD_2D];
    uint8_t levels_buf_ref_[TX_PAD_2D];
    TranLow input_coeff_[MAX_TX_SQUARE];

    uint8_t *levels_test_;
    uint8_t *levels_ref_;
    const TxbInitLevelsFunc ref_func_;
};

TEST_P(EncodeTxbInitLevelTest, txb_init_levels_match) {
    const int loops = 100;
    for (int i = 0; i < loops; ++i) {
        run_test(TEST_GET_PARAM(0), TEST_GET_PARAM(1), false);
    }
}

TEST_P(EncodeTxbInitLevelTest, DISABLED_txb_init_levels_speed) {
    run_test(TEST_GET_PARAM(0), TEST_GET_PARAM(1), true);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, EncodeTxbInitLevelTest,
    ::testing::Combine(::testing::Values(&svt_av1_txb_init_levels_avx2),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, EncodeTxbInitLevelTest,
    ::testing::Combine(::testing::Values(&svt_av1_txb_init_levels_sse4_1),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, EncodeTxbInitLevelTest,
    ::testing::Combine(::testing::Values(&svt_av1_txb_init_levels_avx512),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));
#endif
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, EncodeTxbInitLevelTest,
    ::testing::Combine(::testing::Values(&svt_av1_txb_init_levels_neon),
                       ::testing::Range(0, static_cast<int>(TX_SIZES_ALL), 1)));
#endif  // ARCH_AARCH64
}  // namespace
