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
 * @file intrapred_edge_filter_test.cc
 *
 * @brief Unit test for upsample and edge filter:
 * - svt_av1_upsample_intra_edge
 * - svt_av1_filter_intra_edge
 * - svt_av1_filter_intra_edge_high
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "gtest/gtest.h"
#include "random.h"
#include "util.h"

namespace {
using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

// -----------------------------------------------------------------------------
// Upsample test
using UPSAMPLE_LBD = void (*)(uint8_t *p, int size);

/**
 * @brief Unit test for upsample in intra prediction specified in
 * spec 7.11.2.11:
 * - svt_av1_upsample_intra_edge
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check test output and reference output.
 * Define a templete class to handle the common process, and
 * declare sub class to handle different bitdepth and function types.
 *
 * Expect result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * Neighbor pixel buffer: Fill with random values
 * numPx: [4, 16] (w + (pAngle < 90 ? h : 0)) and and max blkWh is 16
 * according to spec 7.11.2
 * BitDepth: 8bit
 */
class UpsampleTest : public ::testing::TestWithParam<UPSAMPLE_LBD> {
  public:
    UpsampleTest() {
        ref_func_ = svt_av1_upsample_intra_edge_c;
        tst_func_ = GetParam();
        bd_ = 8;
        common_init();
    }

    void RunTest() {
        SVTRandom pix_rnd(0, (1 << bd_) - 1);
        for (int iter = 0; iter < num_tests; ++iter) {
            for (int i = 1; i < 5; ++i) {  // [1, 4]
                numPx_ = 4 * i;            // blkWh is increased with step 4.
                prepare_data(pix_rnd);

                ref_func_(edge_ref_, numPx_);
                tst_func_(edge_tst_, numPx_);

                // When the process completes, entries -2 to 2*numPx-2
                // are valid in buf;
                const int max_idx = (numPx_ - 1) * 2;
                for (int j = -2; j <= max_idx; ++j)
                    ASSERT_EQ(edge_ref_[j], edge_tst_[j]);
            }
        }
    }

  protected:
    static const int num_tests = 1000000;
    static const int edge_buf_size = 2 * 64 + 32;
    static const int start_offset = 16;

    void common_init() {
        edge_ref_ = &edge_ref_data_[start_offset];
        edge_tst_ = &edge_tst_data_[start_offset];
    }

    void prepare_data(SVTRandom &pix_rnd) {
        // When the process starts, entries -1 to numPx-1 are valid in buf
        int i = 0;
        for (; i < start_offset + numPx_; ++i)
            edge_ref_data_[i] = edge_tst_data_[i] = (uint8_t)pix_rnd.random();

        uint8_t last = edge_ref_data_[start_offset + numPx_ - 1];
        for (; i < edge_buf_size; ++i)
            edge_ref_data_[i] = edge_tst_data_[i] = last;
    }

    uint8_t edge_ref_data_[edge_buf_size];
    uint8_t edge_tst_data_[edge_buf_size];

    uint8_t *edge_ref_;
    uint8_t *edge_tst_;

    UPSAMPLE_LBD ref_func_;
    UPSAMPLE_LBD tst_func_;
    int numPx_;
    int bd_;
};

TEST_P(UpsampleTest, RunTest) {
    RunTest();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, UpsampleTest,
                         ::testing::Values(svt_av1_upsample_intra_edge_sse4_1));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, UpsampleTest,
                         ::testing::Values(svt_av1_upsample_intra_edge_neon));
#endif  // ARCH_AARCH64

// -----------------------------------------------------------------------------
// Filter edge Tests
// Declare macros and functions required
#define INTRA_EDGE_FILT 3
#define INTRA_EDGE_TAPS 5
#define MAX_UPSAMPLE_SZ 16

extern "C" void reset_test_env();

using FILTER_EDGE_LBD = void (*)(uint8_t *p, int size, int strength);
using FILTER_EDGE_HBD = void (*)(uint16_t *p, int size, int strength);

/**
 * @brief Unit test for edge filter in intra prediction:
 * - svt_av1_filter_intra_edge
 * - svt_av1_filter_intra_edge_high
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check test output and reference output.
 * Define a templete class to handle the common process, and
 * declare sub class to handle different bitdepth and function types.
 *
 * Expect result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * Neighbor pixel buffer: Fill with random values
 * Strength: [0, 3] // spec 7.11.2.9 Intra edge filter strength selection
 * numPx: [5, 129] // Min(w, (MaxX-x+1)) + (pAngle < 90 ? h : 0) + 1
 * BitDepth: 8bit and 10bit
 */
template <typename Sample, typename Func>
class FilterEdgeTest : public testing::TestWithParam<Func> {
  public:
    FilterEdgeTest() {
        common_init();
    }

    virtual ~FilterEdgeTest() {
    }

    void RunTest() {
        SVTRandom numPx_rnd(1, 32);    // range [1, 32]
        SVTRandom strength_rnd(0, 3);  // range [0, 3]
        SVTRandom pix_rnd(0, (1 << bd_) - 1);
        for (int iter = 0; iter < num_tests; ++iter) {
            // random strength and size
            strength_ = strength_rnd.random();
            numPx_ = 4 * numPx_rnd.random() + 1;

            prepare_data(pix_rnd);

            run_filter_edge();

            for (int i = 0; i < numPx_; ++i)
                ASSERT_EQ(edge_ref_[i], edge_tst_[i]);
        }
    }

  protected:
    static const int num_tests = 1000000;
    static const int edge_buf_size = 2 * MAX_TX_SIZE + 32;
    static const int start_offset = 15;

    void common_init() {
        edge_ref_ = &edge_ref_data_[start_offset];
        edge_tst_ = &edge_tst_data_[start_offset];
    }

    void prepare_data(SVTRandom &pix_rnd) {
        int i = 0;
        for (; i < start_offset + numPx_; ++i)
            edge_ref_data_[i] = edge_tst_data_[i] = (Sample)pix_rnd.random();
    }

    void run_filter_edge() {
        // svt_av1_filter_intra_edge_c calls svt_memcpy through the rtcd
        // pointers, reset the environment to make sure we call the C version.
        reset_test_env();

        ref_func_(edge_ref_, numPx_, strength_);
        tst_func_(edge_tst_, numPx_, strength_);
    }

    Sample edge_ref_data_[edge_buf_size];
    Sample edge_tst_data_[edge_buf_size];

    Sample *edge_ref_;
    Sample *edge_tst_;

    Func ref_func_;
    Func tst_func_;
    int numPx_;
    int bd_;
    int strength_;
};

class LowbdFilterEdgeTest : public FilterEdgeTest<uint8_t, FILTER_EDGE_LBD> {
  public:
    LowbdFilterEdgeTest() {
        ref_func_ = svt_av1_filter_intra_edge_c;
        tst_func_ = GetParam();
        bd_ = 8;
    }
};

TEST_P(LowbdFilterEdgeTest, RunTest) {
    RunTest();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, LowbdFilterEdgeTest,
                         ::testing::Values(svt_av1_filter_intra_edge_sse4_1));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, LowbdFilterEdgeTest,
                         ::testing::Values(svt_av1_filter_intra_edge_neon));
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
class HighbdFilterEdgeTest : public FilterEdgeTest<uint16_t, FILTER_EDGE_HBD> {
  public:
    HighbdFilterEdgeTest() {
        ref_func_ = svt_av1_filter_intra_edge_high_c;
        tst_func_ = GetParam();
        bd_ = 10;
    }
};

TEST_P(HighbdFilterEdgeTest, RunTest) {
    RunTest();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, HighbdFilterEdgeTest,
    ::testing::Values(svt_av1_filter_intra_edge_high_sse4_1));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HighbdFilterEdgeTest,
    ::testing::Values(svt_av1_filter_intra_edge_high_neon));
#endif  // ARCH_AARCH64
#endif
}  // namespace
