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
 * @file ResizeTest.cc
 *
 * @brief Unit test for resize of downsampling functions:
 * - svt_av1_resize_plane
 * - svt_av1_highbd_resize_plane
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "utility.h"
#include "unit_test_utility.h"
#include "random.h"
#include "util.h"

extern "C" void calculate_scaled_size_helper(uint16_t *dim, uint8_t denom);

namespace {
using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

static const int min_test_times = 10;
static const int default_speed_test_times = 1000;
static const uint8_t REF_STUFF = 0xAA;
static const uint8_t TST_STUFF = 0xBB;

/** setup_test_env and reset_test_env are implemented in test/TestEnv.c */
extern "C" void setup_test_env();
extern "C" void reset_test_env();

extern "C" EbErrorType svt_av1_resize_plane_horizontal(
    const uint8_t *const input, int height, int width, int in_stride,
    uint8_t *output, int height2, int width2, int out_stride);
extern "C" EbErrorType svt_av1_highbd_resize_plane_horizontal(
    const uint16_t *const input, int height, int width, int in_stride,
    uint16_t *output, int height2, int width2, int out_stride, int bd);

typedef std::tuple<int, /**< width of source/upscaled picture */
                   int, /**< height of source picture */
                   int> /**< stride of source picture */
    PicSizeParam;

typedef std::tuple<PicSizeParam, int, /**< denominator of scaling: 8~16 */
                   int>               /**< bit depth: 8, 10, 12 */
    ResizeTestParam;

/**
 * @brief Unit test for resize down sampling:
 * - svt_av1_resize_plane
 * - svt_av1_highbd_resize_plane
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same random data and check test output and reference output.
 * Define a template class to handle the common process, and
 * declare sub class to handle different bit depth.
 *
 * Expected result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * input value: Fill with zero, extreme values and random values
 * test mode: common video frame resolution, 2D data blocks with random
 * rectangle size
 *
 */
template <typename Sample>
class ResizePlaneTest : public ::testing::TestWithParam<ResizeTestParam> {
  public:
    ResizePlaneTest() {
        src_width_ = std::get<0>(TEST_GET_PARAM(0));
        src_height_ = std::get<1>(TEST_GET_PARAM(0));
        src_stride_ = std::get<2>(TEST_GET_PARAM(0));
        denom_ = TEST_GET_PARAM(1);
        bd_ = TEST_GET_PARAM(2);
        uint16_t scaled_width = (uint16_t)src_width_;
        calculate_scaled_size_helper(&scaled_width, denom_);
        scaled_width_ = (int)scaled_width;
        uint16_t scaled_height = (uint16_t)src_height_;
        calculate_scaled_size_helper(&scaled_height, denom_);
        scaled_height_ = (int)scaled_height;
        width_only_ = false;
        rnd_ = new SVTRandom(bd_, false);
        setup_test_env();
    }

    virtual ~ResizePlaneTest() {
        delete rnd_;
    }

    virtual void SetUp() {
        ASSERT_LE(src_width_, src_stride_)
            << "picture width must be less equal than stride";
        ASSERT_LE(scaled_width_, src_width_)
            << "width of scaled picture must be less equal than source";
        src_ = (Sample *)(svt_aom_malloc(src_stride_ * src_height_ *
                                         sizeof(*src_)));
        ASSERT_NE(src_, nullptr);

        scaled_ref_ = (Sample *)(svt_aom_malloc(src_stride_ * src_height_ *
                                                sizeof(*scaled_ref_)));
        ASSERT_NE(scaled_ref_, nullptr);
        memset(scaled_ref_,
               REF_STUFF,
               src_stride_ * src_height_ * sizeof(*scaled_ref_));

        scaled_tst_ = (Sample *)(svt_aom_malloc(src_stride_ * src_height_ *
                                                sizeof(*scaled_tst_)));
        ASSERT_NE(scaled_tst_, nullptr);
        memset(scaled_tst_,
               TST_STUFF,
               src_stride_ * src_height_ * sizeof(*scaled_tst_));
    }

    virtual void TearDown() {
        svt_aom_free(src_);
        svt_aom_free(scaled_ref_);
        svt_aom_free(scaled_tst_);
    }

    void prepare_zero_data() {
        memset(src_, 0, src_stride_ * src_height_ * sizeof(*src_));
    }
    void prepare_random_data() {
        for (int i = 0; i < src_stride_ * src_height_; ++i) {
            src_[i] = rnd_->random();
        }
    }

    void prepare_extreme_data() {
        for (int i = 0; i < src_stride_ * src_height_; ++i) {
            src_[i] = (1 << bd_) - 1;
        }
    }
    virtual void run_test(bool width_only = false) = 0;
    virtual void speed_test(int loop, bool width_only = false) = 0;

    void check_data(const int index) {
        const uint16_t value_limit = 1 << bd_;
        Sample ref_guard, tst_guard;
        memset(&ref_guard, REF_STUFF, sizeof(ref_guard));
        memset(&tst_guard, TST_STUFF, sizeof(tst_guard));

        for (int y = 0; y < (width_only_ ? src_height_ : scaled_height_); y++) {
            // check upscaled data
            for (int x = 0; x < scaled_width_; x++) {
                ASSERT_LT(scaled_ref_[y * src_stride_ + x], value_limit);
                ASSERT_LT(scaled_tst_[y * src_stride_ + x], value_limit);
                ASSERT_EQ(scaled_ref_[y * src_stride_ + x],
                          scaled_tst_[y * src_stride_ + x])
                    << "scaled pixel mismatch at test(" << index
                    << ") row: " << y << ", col: " << x << ", "
                    << scaled_ref_[y * src_stride_ + x] << "<-->"
                    << scaled_tst_[y * src_stride_ + x];
            }
            // check padding data
            for (int x = scaled_width_; x < src_stride_; x++) {
                EXPECT_EQ(scaled_ref_[y * src_stride_ + x], ref_guard);
                EXPECT_EQ(scaled_tst_[y * src_stride_ + x], tst_guard);
            }
        }
    }

    virtual void run_zero_test(bool width_only = false) {
        const int iters = min_test_times;
        for (int iter = 0; iter < iters && !HasFatalFailure(); ++iter) {
            prepare_zero_data();
            run_test(width_only);
            check_data(iter);
        }
    }

    virtual void run_random_test(const int run_times, bool width_only = false) {
        const int iters = AOMMAX(run_times, min_test_times);
        for (int iter = 0; iter < iters && !HasFatalFailure(); ++iter) {
            prepare_random_data();
            run_test(width_only);
            check_data(iter);
        }
    }

    virtual void run_extreme_test(bool width_only = false) {
        const int iters = min_test_times;
        for (int iter = 0; iter < iters && !HasFatalFailure(); ++iter) {
            prepare_extreme_data();
            run_test(width_only);
            check_data(iter);
        }
    }

    virtual void run_speed_test(bool width_only = false) {
        prepare_random_data();
        speed_test(default_speed_test_times, width_only);
    }

  protected:
    int src_width_;
    int src_height_;
    int src_stride_;
    uint8_t denom_;
    int bd_;
    int scaled_width_;
    int scaled_height_;
    bool width_only_;

    Sample *src_;
    Sample *scaled_ref_;
    Sample *scaled_tst_;
    SVTRandom *rnd_;
};

class ResizePlaneLbdTest : public ResizePlaneTest<uint8_t> {
  public:
    void run_test(bool width_only) override {
        // setup using c code
        reset_test_env();
        if (width_only) {
            svt_av1_resize_plane_horizontal(src_,
                                            src_height_,
                                            src_width_,
                                            src_stride_,
                                            scaled_ref_,
                                            src_height_,
                                            scaled_width_,
                                            src_stride_);
        } else {
            svt_av1_resize_plane(src_,
                                 src_height_,
                                 src_width_,
                                 src_stride_,
                                 scaled_ref_,
                                 scaled_height_,
                                 scaled_width_,
                                 src_stride_);
        }
        // setup using simd accelerating
        setup_test_env();
        if (width_only) {
            svt_av1_resize_plane_horizontal(src_,
                                            src_height_,
                                            src_width_,
                                            src_stride_,
                                            scaled_tst_,
                                            src_height_,
                                            scaled_width_,
                                            src_stride_);
        } else {
            svt_av1_resize_plane(src_,
                                 src_height_,
                                 src_width_,
                                 src_stride_,
                                 scaled_tst_,
                                 scaled_height_,
                                 scaled_width_,
                                 src_stride_);
        }
    }
    void speed_test(int loop, bool width_only) override {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        // setup using c code
        reset_test_env();
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < loop; i++) {
            if (width_only) {
                svt_av1_resize_plane_horizontal(src_,
                                                src_height_,
                                                src_width_,
                                                src_stride_,
                                                scaled_ref_,
                                                src_height_,
                                                scaled_width_,
                                                src_stride_);
            } else {
                svt_av1_resize_plane(src_,
                                     src_height_,
                                     src_width_,
                                     src_stride_,
                                     scaled_ref_,
                                     scaled_height_,
                                     scaled_width_,
                                     src_stride_);
            }
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        // setup using simd accelerating
        setup_test_env();
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < loop; i++) {
            if (width_only) {
                svt_av1_resize_plane_horizontal(src_,
                                                src_height_,
                                                src_width_,
                                                src_stride_,
                                                scaled_tst_,
                                                src_height_,
                                                scaled_width_,
                                                src_stride_);
            } else {
                svt_av1_resize_plane(src_,
                                     src_height_,
                                     src_width_,
                                     src_stride_,
                                     scaled_tst_,
                                     scaled_height_,
                                     scaled_width_,
                                     src_stride_);
            }
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);
        printf("Resize %ux%u --> %ux%u\n",
               src_width_,
               src_height_,
               scaled_width_,
               width_only ? src_height_ : scaled_height_);
        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_av1_resize_plane_c   : %6.2f\n",
               1000000 * time_c / loop);
        printf(
            "    svt_av1_resize_plane_avx2 : %6.2f   (Comparison: "
            "%5.2fx)\n",
            1000000 * time_o / loop,
            time_c / time_o);
    }
};

TEST_P(ResizePlaneLbdTest, MatchTestWithZeroValue) {
    run_zero_test();
    run_zero_test(true);
}
TEST_P(ResizePlaneLbdTest, MatchTestWithRandomValue) {
    run_random_test(10);
    run_random_test(10, true);
}
TEST_P(ResizePlaneLbdTest, MatchTestWithExtremeValue) {
    run_extreme_test();
    run_extreme_test(true);
}
TEST_P(ResizePlaneLbdTest, DISABLED_SpeedTestWithRandomValue) {
    run_speed_test();
    run_speed_test(true);
}

static PicSizeParam pic_size_vector[] = {
    make_tuple(1280, 720, 1520),
    make_tuple(1920, 1080, 2240),
    make_tuple(3840, 2160, 4480),
};

INSTANTIATE_TEST_SUITE_P(
    Resize, ResizePlaneLbdTest,
    ::testing::Combine(::testing::ValuesIn(pic_size_vector),
                       ::testing::Range(8, 16), ::testing::Values(8)));

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
class ResizePlaneHbdTest : public ResizePlaneTest<uint16_t> {
  protected:
    void run_test(bool width_only) override {
        // setup using c code
        reset_test_env();
        if (width_only) {
            svt_av1_highbd_resize_plane_horizontal(src_,
                                                   src_height_,
                                                   src_width_,
                                                   src_stride_,
                                                   scaled_ref_,
                                                   src_height_,
                                                   scaled_width_,
                                                   src_stride_,
                                                   bd_);
        } else {
            svt_av1_highbd_resize_plane(src_,
                                        src_height_,
                                        src_width_,
                                        src_stride_,
                                        scaled_ref_,
                                        scaled_height_,
                                        scaled_width_,
                                        src_stride_,
                                        bd_);
        }
        // setup using simd accelerating
        setup_test_env();
        if (width_only) {
            svt_av1_highbd_resize_plane_horizontal(src_,
                                                   src_height_,
                                                   src_width_,
                                                   src_stride_,
                                                   scaled_tst_,
                                                   src_height_,
                                                   scaled_width_,
                                                   src_stride_,
                                                   bd_);
        } else {
            svt_av1_highbd_resize_plane(src_,
                                        src_height_,
                                        src_width_,
                                        src_stride_,
                                        scaled_tst_,
                                        scaled_height_,
                                        scaled_width_,
                                        src_stride_,
                                        bd_);
        }
    }
    void speed_test(int loop, bool width_only) override {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        // setup using c code
        reset_test_env();
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < loop; i++) {
            if (width_only) {
                svt_av1_highbd_resize_plane_horizontal(src_,
                                                       src_height_,
                                                       src_width_,
                                                       src_stride_,
                                                       scaled_ref_,
                                                       src_height_,
                                                       scaled_width_,
                                                       src_stride_,
                                                       bd_);
            } else {
                svt_av1_highbd_resize_plane(src_,
                                            src_height_,
                                            src_width_,
                                            src_stride_,
                                            scaled_ref_,
                                            scaled_height_,
                                            scaled_width_,
                                            src_stride_,
                                            bd_);
            }
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        // setup using simd accelerating
        setup_test_env();
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < loop; i++) {
            if (width_only) {
                svt_av1_highbd_resize_plane_horizontal(src_,
                                                       src_height_,
                                                       src_width_,
                                                       src_stride_,
                                                       scaled_tst_,
                                                       src_height_,
                                                       scaled_width_,
                                                       src_stride_,
                                                       bd_);
            } else {
                svt_av1_highbd_resize_plane(src_,
                                            src_height_,
                                            src_width_,
                                            src_stride_,
                                            scaled_tst_,
                                            scaled_height_,
                                            scaled_width_,
                                            src_stride_,
                                            bd_);
            }
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);
        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_av1_highbd_resize_plane_c   : %6.2f\n",
               1000000 * time_c / loop);
        printf(
            "    svt_av1_highbd_resize_plane_avx2 : %6.2f   (Comparison: "
            "%5.2fx)\n",
            1000000 * time_o / loop,
            time_c / time_o);
    }
};

TEST_P(ResizePlaneHbdTest, MatchTestWithZeroValue) {
    run_zero_test();
    run_zero_test(true);
}
TEST_P(ResizePlaneHbdTest, MatchTestWithRandomValue) {
    run_random_test(10);
    run_random_test(10, true);
}
TEST_P(ResizePlaneHbdTest, MatchTestWithExtremeValue) {
    run_extreme_test();
    run_extreme_test(true);
}
TEST_P(ResizePlaneHbdTest, DISABLED_SpeedTestWithRandomValue) {
    run_speed_test();
    run_speed_test(true);
}

INSTANTIATE_TEST_SUITE_P(
    Resize, ResizePlaneHbdTest,
    ::testing::Combine(::testing::ValuesIn(pic_size_vector),
                       ::testing::Range(8, 16), ::testing::Values(10, 12)));
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH
}  // namespace
