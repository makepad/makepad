/*
 * Copyright(c) 2022 Cidana Co.,Ltd.
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

namespace {
using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

static const int test_times = 20;

/** setup_test_env and reset_test_env are implemented in test/TestEnv.c */
extern "C" void setup_test_env();
extern "C" void reset_test_env();

typedef int32_t SsimTestParam; /**< bit depth: 8, 10, 12 */

template <typename Sample>
class SsimTest : public ::testing::TestWithParam<SsimTestParam> {
  public:
    SsimTest() {
        bd_ = GetParam();
        rnd_ = new SVTRandom(bd_, false);
        setup_test_env();
    }

    virtual ~SsimTest() {
        delete rnd_;
    }

    virtual void SetUp() {
        src_8x8_ = (Sample *)(svt_aom_malloc(64 * sizeof(*src_8x8_)));
        ASSERT_NE(src_8x8_, nullptr);

        rec_8x8_ = (Sample *)(svt_aom_malloc(64 * sizeof(*rec_8x8_)));
        ASSERT_NE(rec_8x8_, nullptr);

        src_4x4_ = (Sample *)(svt_aom_malloc(16 * sizeof(*src_4x4_)));
        ASSERT_NE(src_4x4_, nullptr);

        rec_4x4_ = (Sample *)(svt_aom_malloc(16 * sizeof(*rec_4x4_)));
        ASSERT_NE(rec_4x4_, nullptr);
    }

    virtual void TearDown() {
        svt_aom_free(src_8x8_);
        svt_aom_free(rec_8x8_);
        svt_aom_free(src_4x4_);
        svt_aom_free(rec_4x4_);
    }

    void prepare_zero_src() {
        memset(src_8x8_, 0, 64 * sizeof(*src_8x8_));
        memset(src_4x4_, 0, 16 * sizeof(*src_4x4_));
    }

    void prepare_extreme_src() {
        for (int i = 0; i < 64; ++i) {
            src_8x8_[i] = (1 << bd_) - 1;
        }
        for (int i = 0; i < 16; ++i) {
            src_4x4_[i] = (1 << bd_) - 1;
        }
    }

    void prepare_zero_rec() {
        memset(rec_8x8_, 0, 64 * sizeof(*rec_8x8_));
        memset(rec_4x4_, 0, 16 * sizeof(*rec_4x4_));
    }

    void prepare_extreme_rec() {
        for (int i = 0; i < 64; ++i) {
            rec_8x8_[i] = (1 << bd_) - 1;
        }
        for (int i = 0; i < 16; ++i) {
            rec_4x4_[i] = (1 << bd_) - 1;
        }
    }

    void prepare_random_data() {
        for (int i = 0; i < 64; ++i) {
            src_8x8_[i] = rnd_->random();
        }
        for (int i = 0; i < 64; ++i) {
            rec_8x8_[i] = rnd_->random();
        }

        for (int i = 0; i < 16; ++i) {
            src_4x4_[i] = rnd_->random();
        }
        for (int i = 0; i < 16; ++i) {
            rec_4x4_[i] = rnd_->random();
        }
    }

    virtual void run_8x8_test(double *score_ref, double *score_simd) = 0;
    virtual void run_4x4_test(double *score_ref, double *score_simd) = 0;

    void check_data(double score_ref, double score_simd, int32_t index) {
        ASSERT_EQ(score_ref, score_simd)
            << "SSIM score mismatch at test(" << index << ")";
    }

    virtual void run_random_test(const int run_times) {
        for (int iter = 0; iter < run_times;) {
            double score_ref;
            double score_simd;
            prepare_random_data();

            run_8x8_test(&score_ref, &score_simd);
            check_data(score_ref, score_simd, iter++);
            if (HasFatalFailure())
                return;
            run_4x4_test(&score_ref, &score_simd);
            check_data(score_ref, score_simd, iter++);
            if (HasFatalFailure())
                return;
        }
    }

    virtual void run_extreme_test() {
        int iter = 0;
        double score_ref;
        double score_simd;

        prepare_zero_src();
        prepare_zero_rec();
        run_8x8_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;
        run_4x4_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;

        prepare_extreme_src();
        prepare_extreme_rec();
        run_8x8_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;
        run_4x4_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;

        prepare_zero_src();
        prepare_extreme_rec();
        run_8x8_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;
        run_4x4_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;

        prepare_extreme_src();
        prepare_zero_rec();
        run_8x8_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;
        run_4x4_test(&score_ref, &score_simd);
        check_data(score_ref, score_simd, iter++);
        if (HasFatalFailure())
            return;
    }

  protected:
    int32_t bd_;
    Sample *src_8x8_ = nullptr;
    Sample *rec_8x8_ = nullptr;
    Sample *src_4x4_ = nullptr;
    Sample *rec_4x4_ = nullptr;
    SVTRandom *rnd_;
};

class SsimLbdTest : public SsimTest<uint8_t> {
  public:
    void run_8x8_test(double *score_ref, double *score_simd) override {
        // setup using c code
        reset_test_env();
        *score_ref = svt_ssim_8x8(src_8x8_, 8, rec_8x8_, 8);

        // setup using simd accelerating
        setup_test_env();
        *score_simd = svt_ssim_8x8(src_8x8_, 8, rec_8x8_, 8);
    }

    void run_4x4_test(double *score_ref, double *score_simd) override {
        // setup using c code
        reset_test_env();
        *score_ref = svt_ssim_4x4(src_4x4_, 4, rec_4x4_, 4);

        // setup using simd accelerating
        setup_test_env();
        *score_simd = svt_ssim_4x4(src_4x4_, 4, rec_4x4_, 4);
    }
};

class SsimHbdTest : public SsimTest<uint16_t> {
  public:
    void run_8x8_test(double *score_ref, double *score_simd) override {
        // setup using c code
        reset_test_env();
        *score_ref = svt_ssim_8x8_hbd(src_8x8_, 8, rec_8x8_, 8);

        // setup using simd accelerating
        setup_test_env();
        *score_simd = svt_ssim_8x8_hbd(src_8x8_, 8, rec_8x8_, 8);
    }

    void run_4x4_test(double *score_ref, double *score_simd) override {
        // setup using c code
        reset_test_env();
        *score_ref = svt_ssim_4x4_hbd(src_4x4_, 4, rec_4x4_, 4);

        // setup using simd accelerating
        setup_test_env();
        *score_simd = svt_ssim_4x4_hbd(src_4x4_, 4, rec_4x4_, 4);
    }
};

TEST_P(SsimLbdTest, MatchTestWithExtremeValue) {
    run_extreme_test();
}
TEST_P(SsimLbdTest, MatchTestWithRandomValue) {
    run_random_test(test_times);
}
TEST_P(SsimHbdTest, MatchTestWithExtremeValue) {
    run_extreme_test();
}
TEST_P(SsimHbdTest, MatchTestWithRandomValue) {
    run_random_test(test_times);
}

INSTANTIATE_TEST_SUITE_P(SSIM, SsimLbdTest, ::testing::Values(8));
INSTANTIATE_TEST_SUITE_P(SSIM, SsimHbdTest, ::testing::Values(10));

}  // namespace
