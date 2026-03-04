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
 * @file intrapred_dr_test.cc
 *
 * @brief Unit test for intra directional prediction :
 * - svt_av1_highbd_dr_prediction_z{1, 2, 3}_avx2
 * - svt_av1_dr_prediction_z{1, 2, 3}_avx2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "common_utils.h"
#include "intra_prediction.h"
#include "random.h"
#include "util.h"

namespace {
using svt_av1_test_tool::SVTRandom;

// copied from EbIntraPrediction.c
const uint16_t dr_intra_derivative[90] = {
    // More evenly spread out angles and limited to 10-bit
    // Values that are 0 will never be used
    //                    Approx angle
    0,    0, 0,        //
    1023, 0, 0,        // 3, ...
    547,  0, 0,        // 6, ...
    372,  0, 0, 0, 0,  // 9, ...
    273,  0, 0,        // 14, ...
    215,  0, 0,        // 17, ...
    178,  0, 0,        // 20, ...
    151,  0, 0,        // 23, ... (113 & 203 are base angles)
    132,  0, 0,        // 26, ...
    116,  0, 0,        // 29, ...
    102,  0, 0, 0,     // 32, ...
    90,   0, 0,        // 36, ...
    80,   0, 0,        // 39, ...
    71,   0, 0,        // 42, ...
    64,   0, 0,        // 45, ... (45 & 135 are base angles)
    57,   0, 0,        // 48, ...
    51,   0, 0,        // 51, ...
    45,   0, 0, 0,     // 54, ...
    40,   0, 0,        // 58, ...
    35,   0, 0,        // 61, ...
    31,   0, 0,        // 64, ...
    27,   0, 0,        // 67, ... (67 & 157 are base angles)
    23,   0, 0,        // 70, ...
    19,   0, 0,        // 73, ...
    15,   0, 0, 0, 0,  // 76, ...
    11,   0, 0,        // 81, ...
    7,    0, 0,        // 84, ...
    3,    0, 0,        // 87, ...
};

static INLINE uint16_t get_dy(int32_t angle) {
    if (angle > 90 && angle < 180) {
        return dr_intra_derivative[angle - 90];
    } else if (angle > 180 && angle < 270) {
        return dr_intra_derivative[270 - angle];
    } else {
        // In this case, we are not really going to use dy. We may return any
        // value.
        return 1;
    }
}
// Get the shift (up-scaled by 256) in X w.r.t a unit change in Y.
// If angle > 0 && angle < 90, dx = -((int32_t)(256 / t));
// If angle > 90 && angle < 180, dx = (int32_t)(256 / t);
// If angle > 180 && angle < 270, dx = 1;
static INLINE uint16_t get_dx(int32_t angle) {
    if (angle > 0 && angle < 90) {
        return dr_intra_derivative[angle];
    } else if (angle > 90 && angle < 180) {
        return dr_intra_derivative[180 - angle];
    } else {
        // In this case, we are not really going to use dx. We may return any
        // value.
        return 1;
    }
}

// define the function types
using Z1_LBD = void (*)(uint8_t *dst, ptrdiff_t stride, int bw, int bh,
                        const uint8_t *above, const uint8_t *left,
                        int upsample_above, int dx, int dy);

using Z2_LBD = void (*)(uint8_t *dst, ptrdiff_t stride, int bw, int bh,
                        const uint8_t *above, const uint8_t *left,
                        int upsample_above, int upsample_left, int dx, int dy);

using Z3_LBD = void (*)(uint8_t *dst, ptrdiff_t stride, int bw, int bh,
                        const uint8_t *above, const uint8_t *left,
                        int upsample_left, int dx, int dy);

using Z1_HBD = void (*)(uint16_t *dst, ptrdiff_t stride, int bw, int bh,
                        const uint16_t *above, const uint16_t *left,
                        int upsample_above, int dx, int dy, int bd);

using Z2_HBD = void (*)(uint16_t *dst, ptrdiff_t stride, int bw, int bh,
                        const uint16_t *above, const uint16_t *left,
                        int upsample_above, int upsample_left, int dx, int dy,
                        int bd);
using Z3_HBD = void (*)(uint16_t *dst, ptrdiff_t stride, int bw, int bh,
                        const uint16_t *above, const uint16_t *left,
                        int upsample_left, int dx, int dy, int bd);

using Z1_LbdPredParam = std::tuple<Z1_LBD, int>;
using Z2_LbdPredParam = std::tuple<Z2_LBD, int>;
using Z3_LbdPredParam = std::tuple<Z3_LBD, int>;

/**
 * @brief Unit test for intra directional prediction:
 * - svt_av1_highbd_dr_prediction_z{1, 2, 3}_avx2
 * - svt_av1_dr_prediction_z{1, 2, 3}_avx2
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
 * TxSize: all the TxSize.
 * BitDepth: 8bit and 10bit
 *
 */
template <typename Sample, typename FuncType>
class DrPredTest {
  public:
    static const int num_tests = 10;
    static const int pred_buf_size = MAX_TX_SQUARE;
    static const int start_offset = 16;
    static const int neighbor_buf_size = ((2 * MAX_TX_SIZE) << 1) + 16;
    static const int z1_start_angle = 0;
    static const int z2_start_angle = 90;
    static const int z3_start_angle = 180;

    DrPredTest() {
        start_angle_ = 0;
        stop_angle_ = 0 + 90;
        ref_func_ = tst_func_ = nullptr;
        bd_ = 8;
        common_init();
    }

    virtual ~DrPredTest() {
    }

    void RunAllTest() {
        for (int angle = start_angle_; angle < stop_angle_; ++angle) {
            dx_ = get_dx(angle);
            dy_ = get_dy(angle);
            if (dx_ & dy_)
                RunTest(angle);
        }
    }

  protected:
    void common_init() {
        bw_ = bh_ = 0;
        dx_ = dy_ = 1;
        dst_ref_ = &dst_ref_data_[0];
        dst_tst_ = &dst_tst_data_[0];
        dst_stride_ = MAX_TX_SIZE;
        above_ = &above_data_[start_offset];
        left_ = &left_data_[start_offset];
    }

    virtual void Predict() {
    }

    void prepare_neighbor_pixel(SVTRandom &rnd, int cnt) {
        if (cnt == 0) {
            for (int i = 0; i < neighbor_buf_size; ++i)
                above_data_[i] = left_data_[i] = (1 << bd_) - 1;
        } else {
            for (int i = 0; i < neighbor_buf_size; ++i) {
                above_data_[i] = (Sample)rnd.random();
                left_data_[i] = (Sample)rnd.random();
            }
        }
    }

    void clear_output_pixel() {
        for (int i = 0; i < pred_buf_size; ++i)
            dst_ref_[i] = 0;

        for (int i = 0; i < pred_buf_size; ++i)
            dst_tst_[i] = 0;
    }

    void RunTest(int angle) {
        SVTRandom rnd(0, (1 << bd_) - 1);
        for (int cnt = 0; cnt < num_tests; ++cnt) {
            prepare_neighbor_pixel(rnd, cnt);
            for (int tx = 0; tx < TX_SIZES_ALL; ++tx) {
                // clear the output for each tx
                clear_output_pixel();
                bw_ = tx_size_wide[tx];
                bh_ = tx_size_high[tx];

                if (enable_upsample_) {
                    upsample_above_ = svt_aom_use_intra_edge_upsample(
                        bw_, bh_, angle - 90, 0);
                    upsample_left_ = svt_aom_use_intra_edge_upsample(
                        bw_, bh_, angle - 180, 0);
                } else {
                    upsample_above_ = upsample_left_ = 0;
                }

                Predict();

                // check the result
                for (int r = 0; r < bh_; ++r) {
                    for (int c = 0; c < bw_; ++c) {
                        ASSERT_EQ(dst_ref_[r * dst_stride_ + c],
                                  dst_tst_[r * dst_stride_ + c])
                            << bw_ << "x" << bh_ << " r: " << r << " c: " << c
                            << " dx: " << dx_ << " dy: " << dy_
                            << " upsample_above: " << upsample_above_
                            << " upsample_left: " << upsample_left_;
                    }
                }
            }
        }
    }

    Sample dst_ref_data_[pred_buf_size];
    Sample dst_tst_data_[pred_buf_size];

    Sample left_data_[neighbor_buf_size];
    Sample above_data_[neighbor_buf_size];

    Sample *dst_ref_;
    Sample *dst_tst_;
    Sample *above_;
    Sample *left_;
    int dst_stride_;

    int upsample_above_;
    int upsample_left_;
    int enable_upsample_;
    int bw_;
    int bh_;
    int dx_;
    int dy_;
    int bd_;

    int start_angle_;
    int stop_angle_;

    FuncType ref_func_;
    FuncType tst_func_;
};

class LowbdZ1PredTest : public DrPredTest<uint8_t, Z1_LBD>,
                        public ::testing::TestWithParam<Z1_LbdPredParam> {
  public:
    LowbdZ1PredTest() {
        start_angle_ = z1_start_angle;
        stop_angle_ = start_angle_ + 90;
        ref_func_ = svt_av1_dr_prediction_z1_c;
        tst_func_ = TEST_GET_PARAM(0);
        enable_upsample_ = TEST_GET_PARAM(1);
        bd_ = 8;

        common_init();
    }

  protected:
    void Predict() override {
        ref_func_(dst_ref_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  dx_,
                  dy_);
        tst_func_(dst_tst_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  dx_,
                  dy_);
    }
};

TEST_P(LowbdZ1PredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, LowbdZ1PredTest,
    ::testing::Combine(::testing::Values(svt_av1_dr_prediction_z1_avx2),
                       ::testing::Values(0, 1)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LowbdZ1PredTest,
    ::testing::Combine(::testing::Values(svt_av1_dr_prediction_z1_neon),
                       ::testing::Values(0, 1)));
#endif  // ARCH_AARCH64

class LowbdZ2PredTest : public DrPredTest<uint8_t, Z2_LBD>,
                        public ::testing::TestWithParam<Z2_LbdPredParam> {
  public:
    LowbdZ2PredTest() {
        start_angle_ = z2_start_angle;
        stop_angle_ = start_angle_ + 90;
        ref_func_ = svt_av1_dr_prediction_z2_c;
        tst_func_ = TEST_GET_PARAM(0);
        enable_upsample_ = TEST_GET_PARAM(1);
        bd_ = 8;

        common_init();
    }

  protected:
    void Predict() override {
        ref_func_(dst_ref_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  upsample_left_,
                  dx_,
                  dy_);
        tst_func_(dst_tst_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  upsample_left_,
                  dx_,
                  dy_);
    }
};

TEST_P(LowbdZ2PredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, LowbdZ2PredTest,
    ::testing::Combine(::testing::Values(svt_av1_dr_prediction_z2_avx2),
                       ::testing::Values(0, 1)));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LowbdZ2PredTest,
    ::testing::Combine(::testing::Values(svt_av1_dr_prediction_z2_neon),
                       ::testing::Values(0, 1)));
#endif  // ARCH_AARCH64

class LowbdZ3PredTest : public DrPredTest<uint8_t, Z3_LBD>,
                        public ::testing::TestWithParam<Z3_LbdPredParam> {
  public:
    LowbdZ3PredTest() {
        start_angle_ = z3_start_angle;
        stop_angle_ = start_angle_ + 90;
        ref_func_ = svt_av1_dr_prediction_z3_c;
        tst_func_ = TEST_GET_PARAM(0);
        enable_upsample_ = TEST_GET_PARAM(1);
        bd_ = 8;

        common_init();
    }

  protected:
    void Predict() override {
        ref_func_(dst_ref_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_left_,
                  dx_,
                  dy_);
        tst_func_(dst_tst_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_left_,
                  dx_,
                  dy_);
    }
};

TEST_P(LowbdZ3PredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, LowbdZ3PredTest,
    ::testing::Combine(::testing::Values(svt_av1_dr_prediction_z3_avx2),
                       ::testing::Values(0, 1)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LowbdZ3PredTest,
    ::testing::Combine(::testing::Values(svt_av1_dr_prediction_z3_neon),
                       ::testing::Values(0, 1)));
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH

class HighbdZ1PredTest : public DrPredTest<uint16_t, Z1_HBD>,
                         public ::testing::TestWithParam<Z1_HBD> {
  public:
    HighbdZ1PredTest() {
        start_angle_ = z1_start_angle;
        stop_angle_ = start_angle_ + 90;
        ref_func_ = svt_av1_highbd_dr_prediction_z1_c;
        tst_func_ = GetParam();
        bd_ = 10;

        common_init();
    }

  protected:
    void Predict() override {
        ref_func_(dst_ref_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  dx_,
                  dy_,
                  bd_);
        tst_func_(dst_tst_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  dx_,
                  dy_,
                  bd_);
    }
};

TEST_P(HighbdZ1PredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, HighbdZ1PredTest,
    ::testing::Values(svt_av1_highbd_dr_prediction_z1_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HighbdZ1PredTest,
    ::testing::Values(svt_av1_highbd_dr_prediction_z1_neon));
#endif  // ARCH_AARCH64

class HighbdZ2PredTest : public DrPredTest<uint16_t, Z2_HBD>,
                         public ::testing::TestWithParam<Z2_HBD> {
  public:
    HighbdZ2PredTest() {
        start_angle_ = z2_start_angle;
        stop_angle_ = start_angle_ + 90;
        ref_func_ = svt_av1_highbd_dr_prediction_z2_c;
        tst_func_ = GetParam();
        bd_ = 10;

        common_init();
    }

  protected:
    void Predict() override {
        ref_func_(dst_ref_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  upsample_left_,
                  dx_,
                  dy_,
                  bd_);
        tst_func_(dst_tst_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_above_,
                  upsample_left_,
                  dx_,
                  dy_,
                  bd_);
    }
};

TEST_P(HighbdZ2PredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, HighbdZ2PredTest,
    ::testing::Values(svt_av1_highbd_dr_prediction_z2_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HighbdZ2PredTest,
    ::testing::Values(svt_av1_highbd_dr_prediction_z2_neon));
#endif  // ARCH_AARCH64

class HighbdZ3PredTest : public DrPredTest<uint16_t, Z3_HBD>,
                         public ::testing::TestWithParam<Z3_HBD> {
  public:
    HighbdZ3PredTest() {
        start_angle_ = z3_start_angle;
        stop_angle_ = start_angle_ + 90;
        ref_func_ = svt_av1_highbd_dr_prediction_z3_c;
        tst_func_ = GetParam();
        bd_ = 10;

        common_init();
    }

  protected:
    void Predict() override {
        ref_func_(dst_ref_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_left_,
                  dx_,
                  dy_,
                  bd_);
        tst_func_(dst_tst_,
                  dst_stride_,
                  bw_,
                  bh_,
                  above_,
                  left_,
                  upsample_left_,
                  dx_,
                  dy_,
                  bd_);
    }
};

TEST_P(HighbdZ3PredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, HighbdZ3PredTest,
    ::testing::Values(svt_av1_highbd_dr_prediction_z3_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HighbdZ3PredTest,
    ::testing::Values(svt_av1_highbd_dr_prediction_z3_neon));
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
}  // namespace
