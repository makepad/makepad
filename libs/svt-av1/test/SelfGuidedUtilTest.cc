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
 * @file pixel_proj_err_test.cc
 *
 * @brief Unit test of project-related test in selfguided filter:
 *
 * - av1_lowbd_pixel_proj_error_avx2
 * - av1_highbd_pixel_proj_error_avx2
 * - svt_get_proj_subspace_avx2
 *
 * @author Cidana-Edmond, Cidana-Wenyao
 *
 ******************************************************************************/

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "utility.h"
#include "restoration.h"
#include "unit_test_utility.h"
#include "random.h"
#include "util.h"

#define MAX_DATA_BLOCK 384

namespace {
using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

static const int min_test_times = 10;

typedef int64_t (*PixelProjFunc)(const uint8_t *src8, int32_t width,
                                 int32_t height, int32_t src_stride,
                                 const uint8_t *dat8, int32_t dat_stride,
                                 int32_t *flt0, int32_t flt0_stride,
                                 int32_t *flt1, int32_t flt1_stride,
                                 const int32_t xq[2],
                                 const SgrParamsType *params);

typedef std::tuple<const PixelProjFunc, const PixelProjFunc>
    PixelProjErrorTestParam;

/**
 * @brief Unit test for pixel projection error:
 * - av1_lowbd_pixel_proj_error_avx2
 * - av1_highbd_pixel_proj_error_avx2
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same random data and check test output and reference output.
 * Define a template class to handle the common process, and
 * declare sub class to handle different bitdepth.
 *
 * Expected result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * input value: Fill with random values
 * test mode: fixed block size, random block size and extreme data check
 *
 */
template <typename Sample>
class PixelProjErrorTest
    : public ::testing::TestWithParam<PixelProjErrorTestParam> {
  public:
    PixelProjErrorTest()
        : rnd8_(8, false),
          rnd16_(16, false),
          rnd15s_(15, true),
          rnd_blk_size_(1, MAX_DATA_BLOCK) {
        tst_func_ = TEST_GET_PARAM(0);
        ref_func_ = TEST_GET_PARAM(1);
        src_ = nullptr;
        dgd_ = nullptr;
        flt0_ = nullptr;
        flt1_ = nullptr;
    }

    virtual void SetUp() {
        src_ = (Sample *)(svt_aom_malloc(MAX_DATA_BLOCK * MAX_DATA_BLOCK *
                                         sizeof(*src_)));
        ASSERT_NE(src_, nullptr);
        dgd_ = (Sample *)(svt_aom_malloc(MAX_DATA_BLOCK * MAX_DATA_BLOCK *
                                         sizeof(*dgd_)));
        ASSERT_NE(dgd_, nullptr);
        flt0_ = (int32_t *)(svt_aom_malloc(MAX_DATA_BLOCK * MAX_DATA_BLOCK *
                                           sizeof(*flt0_)));
        ASSERT_NE(flt0_, nullptr);
        flt1_ = (int32_t *)(svt_aom_malloc(MAX_DATA_BLOCK * MAX_DATA_BLOCK *
                                           sizeof(*flt1_)));
        ASSERT_NE(flt1_, nullptr);
    }

    virtual void TearDown() {
        svt_aom_free(src_);
        svt_aom_free(dgd_);
        svt_aom_free(flt0_);
        svt_aom_free(flt1_);
    }

    virtual void prepare_random_data() = 0;
    virtual void prepare_extreme_data() = 0;
    void run_and_check_data(const int index, const int fixed_size) {
        const int dgd_stride = MAX_DATA_BLOCK;
        const int src_stride = MAX_DATA_BLOCK;
        const int flt0_stride = MAX_DATA_BLOCK;
        const int flt1_stride = MAX_DATA_BLOCK;
        int h_end = fixed_size;
        int v_end = fixed_size;
        bool is_fixed_size = true;
        if (fixed_size == 0) {
            h_end = rnd_blk_size_.random();
            v_end = rnd_blk_size_.random();
            is_fixed_size = false;
        }

        int xq[2];
        xq[0] = rnd8_.random() % (1 << SGRPROJ_PRJ_BITS);
        xq[1] = rnd8_.random() % (1 << SGRPROJ_PRJ_BITS);
        SgrParamsType params;
        params.r[0] =
            !is_fixed_size ? (rnd8_.random() % MAX_RADIUS) : (index % 2);
        params.r[1] =
            !is_fixed_size ? (rnd8_.random() % MAX_RADIUS) : (index / 2);
        params.s[0] =
            !is_fixed_size ? (rnd8_.random() % MAX_RADIUS) : (index % 2);
        params.s[1] =
            !is_fixed_size ? (rnd8_.random() % MAX_RADIUS) : (index / 2);
        uint8_t *dgd =
            (sizeof(*dgd_) == 2) ? (CONVERT_TO_BYTEPTR(dgd_)) : (uint8_t *)dgd_;
        uint8_t *src =
            (sizeof(*src_) == 2) ? (CONVERT_TO_BYTEPTR(src_)) : (uint8_t *)src_;

        int64_t err_ref = ref_func_(src,
                                    h_end,
                                    v_end,
                                    src_stride,
                                    dgd,
                                    dgd_stride,
                                    flt0_,
                                    flt0_stride,
                                    flt1_,
                                    flt1_stride,
                                    xq,
                                    &params);
        int64_t err_test = tst_func_(src,
                                     h_end,
                                     v_end,
                                     src_stride,
                                     dgd,
                                     dgd_stride,
                                     flt0_,
                                     flt0_stride,
                                     flt1_,
                                     flt1_stride,
                                     xq,
                                     &params);
        ASSERT_EQ(err_ref, err_test);
    }

    virtual void run_random_test(const int run_times,
                                 const bool is_fixed_size) {
        const int iters = AOMMAX(run_times, min_test_times);
        for (int iter = 0; iter < iters && !HasFatalFailure(); ++iter) {
            prepare_random_data();
            run_and_check_data(iter, is_fixed_size ? 128 : 0);
        }
    }

    virtual void run_speed_test(const int size, const int r0, const int r1) {
        prepare_random_data();

        const int dgd_stride = MAX_DATA_BLOCK;
        const int src_stride = MAX_DATA_BLOCK;
        const int flt0_stride = MAX_DATA_BLOCK;
        const int flt1_stride = MAX_DATA_BLOCK;
        int h_end = size;
        int v_end = size;
        int xq[2];
        int64_t err_ref, err_test;
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        xq[0] = rnd8_.random() % (1 << SGRPROJ_PRJ_BITS);
        xq[1] = rnd8_.random() % (1 << SGRPROJ_PRJ_BITS);
        SgrParamsType params;
        params.r[0] = r0;
        params.r[1] = r1;
        uint8_t *dgd =
            (sizeof(*dgd_) == 2) ? (CONVERT_TO_BYTEPTR(dgd_)) : (uint8_t *)dgd_;
        uint8_t *src =
            (sizeof(*src_) == 2) ? (CONVERT_TO_BYTEPTR(src_)) : (uint8_t *)src_;

        const uint64_t num_loop = 1000000;

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            err_ref = ref_func_(src,
                                h_end,
                                v_end,
                                src_stride,
                                dgd,
                                dgd_stride,
                                flt0_,
                                flt0_stride,
                                flt1_,
                                flt1_stride,
                                xq,
                                &params);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            err_test = tst_func_(src,
                                 h_end,
                                 v_end,
                                 src_stride,
                                 dgd,
                                 dgd_stride,
                                 flt0_,
                                 flt0_stride,
                                 flt1_,
                                 flt1_stride,
                                 xq,
                                 &params);
        }

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        ASSERT_EQ(err_ref, err_test);

        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("Average Nanoseconds per Function Call\n");
        printf(
            "   svt_av1_lowbd_pixel_proj_error_c(size: %d, r0: %d, r1: %d)   : "
            "%6.2f\n",
            size,
            r0,
            r1,
            1000000 * time_c / num_loop);
        printf(
            "   svt_av1_lowbd_pixel_proj_error_optsize: %d, r0: %d, r1: %d) : "
            "%6.2f   (Comparison: "
            "%5.2fx)\n",
            size,
            r0,
            r1,
            1000000 * time_o / num_loop,
            time_c / time_o);
    }

    virtual void run_extreme_test() {
        const int iters = min_test_times;
        for (int iter = 0; iter < iters && !HasFatalFailure(); ++iter) {
            prepare_extreme_data();
            run_and_check_data(iter, 192);
        }
    }

  protected:
    PixelProjFunc tst_func_;
    PixelProjFunc ref_func_;
    Sample *src_;
    Sample *dgd_;
    int32_t *flt0_;
    int32_t *flt1_;

    SVTRandom rnd8_;
    SVTRandom rnd16_;
    SVTRandom rnd15s_;
    SVTRandom rnd_blk_size_;
};

class PixelProjErrorLbdTest : public PixelProjErrorTest<uint8_t> {
    void prepare_random_data() override {
        for (int i = 0; i < MAX_DATA_BLOCK * MAX_DATA_BLOCK; ++i) {
            dgd_[i] = rnd8_.random();
            src_[i] = rnd8_.random();
            flt0_[i] = rnd15s_.random();
            flt1_[i] = rnd15s_.random();
        }
    }

    void prepare_extreme_data() override {
        for (int i = 0; i < MAX_DATA_BLOCK * MAX_DATA_BLOCK; ++i) {
            dgd_[i] = 0;
            src_[i] = 255;
            flt0_[i] = rnd15s_.random();
            flt1_[i] = rnd15s_.random();
        }
    }
};

TEST_P(PixelProjErrorLbdTest, MatchTestWithRandomValue) {
    run_random_test(50, true);
}
TEST_P(PixelProjErrorLbdTest, MatchTestWithRandomSizeAndValue) {
    run_random_test(50, false);
}
TEST_P(PixelProjErrorLbdTest, MatchTestWithExtremeValue) {
    run_extreme_test();
}
TEST_P(PixelProjErrorLbdTest, DISABLED_SpeedTest) {
    run_speed_test(256, 1, 1);
    run_speed_test(256, 1, 0);
    run_speed_test(256, 0, 0);
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, PixelProjErrorLbdTest,
    ::testing::Values(make_tuple(svt_av1_lowbd_pixel_proj_error_sse4_1,
                                 svt_av1_lowbd_pixel_proj_error_c)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, PixelProjErrorLbdTest,
    ::testing::Values(make_tuple(svt_av1_lowbd_pixel_proj_error_avx2,
                                 svt_av1_lowbd_pixel_proj_error_c)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, PixelProjErrorLbdTest,
    ::testing::Values(make_tuple(svt_av1_lowbd_pixel_proj_error_avx512,
                                 svt_av1_lowbd_pixel_proj_error_c)));
#endif
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, PixelProjErrorLbdTest,
    ::testing::Values(make_tuple(svt_av1_lowbd_pixel_proj_error_neon,
                                 svt_av1_lowbd_pixel_proj_error_c)));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, PixelProjErrorLbdTest,
    ::testing::Values(make_tuple(svt_av1_lowbd_pixel_proj_error_sve,
                                 svt_av1_lowbd_pixel_proj_error_c)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH

class PixelProjErrorHbdTest : public PixelProjErrorTest<uint16_t> {
  protected:
    PixelProjErrorHbdTest() : rnd12_(12, false) {
    }

    void prepare_random_data() override {
        for (int i = 0; i < MAX_DATA_BLOCK * MAX_DATA_BLOCK; ++i) {
            dgd_[i] = rnd12_.random();
            src_[i] = rnd12_.random();
            flt0_[i] = rnd15s_.random();
            flt1_[i] = rnd15s_.random();
        }
    }

    void prepare_extreme_data() override {
        for (int i = 0; i < MAX_DATA_BLOCK * MAX_DATA_BLOCK; ++i) {
            dgd_[i] = 0;
            src_[i] = (1 << 12) - 1;
            flt0_[i] = rnd15s_.random();
            flt1_[i] = rnd15s_.random();
        }
    }

  private:
    SVTRandom rnd12_;
};

TEST_P(PixelProjErrorHbdTest, MatchTestWithRandomValue) {
    run_random_test(50, true);
}
TEST_P(PixelProjErrorHbdTest, MatchTestWithRandomSizeAndValue) {
    run_random_test(50, false);
}
TEST_P(PixelProjErrorHbdTest, MatchTestWithExtremeValue) {
    run_extreme_test();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, PixelProjErrorHbdTest,
    ::testing::Values(make_tuple(svt_av1_highbd_pixel_proj_error_sse4_1,
                                 svt_av1_highbd_pixel_proj_error_c)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, PixelProjErrorHbdTest,
    ::testing::Values(make_tuple(svt_av1_highbd_pixel_proj_error_avx2,
                                 svt_av1_highbd_pixel_proj_error_c)));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, PixelProjErrorHbdTest,
    ::testing::Values(make_tuple(svt_av1_highbd_pixel_proj_error_neon,
                                 svt_av1_highbd_pixel_proj_error_c)));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, PixelProjErrorHbdTest,
    ::testing::Values(make_tuple(svt_av1_highbd_pixel_proj_error_sve,
                                 svt_av1_highbd_pixel_proj_error_c)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

typedef void (*GetProjSubspaceFunc)(const uint8_t *src8, int32_t width,
                                    int32_t height, int32_t src_stride,
                                    const uint8_t *dat8, int32_t dat_stride,
                                    int32_t use_highbitdepth, int32_t *flt0,
                                    int32_t flt0_stride, int32_t *flt1,
                                    int32_t flt1_stride, int32_t *xq,
                                    const SgrParamsType *params);

template <typename Sample>
class GetProjSubspaceTest
    : public ::testing::TestWithParam<GetProjSubspaceFunc> {
  public:
    GetProjSubspaceTest() = default;
    void SetUp() override {
        test_impl_ = GetParam();
        input_ = (Sample *)svt_aom_memalign(
            32, stride * (height + 32) * sizeof(Sample));
        output_ = (Sample *)svt_aom_memalign(
            32, out_stride * (height + 32) * sizeof(Sample));
        tmpbuf_ = (int32_t *)svt_aom_memalign(32, RESTORATION_TMPBUF_SIZE);
    }

    void TearDown() override {
        svt_aom_free(input_);
        svt_aom_free(output_);
        svt_aom_free(tmpbuf_);
    }

    void run_test() {
        const int32_t pu_width = RESTORATION_PROC_UNIT_SIZE;
        const int32_t pu_height = RESTORATION_PROC_UNIT_SIZE;
        const int NUM_ITERS = 2000;
        int i, j, k;
        Sample *input = input_ + stride * 16 + 16;
        Sample *output = output_ + out_stride * 16 + 16;
        int32_t width_sample[] = {128, 192, 256, 270};
        for (int w_index = 0;
             w_index < (int)(sizeof(width_sample) / sizeof(width_sample[0]));
             w_index++) {
            int32_t width = width_sample[w_index];
            int32_t *flt0 = tmpbuf_;
            int32_t *flt1 = flt0 + RESTORATION_UNITPELS_MAX;
            int32_t flt_stride = ((width + 7) & ~7) + 8;

            // check all the sg params
            SVTRandom rnd(8, false);
            for (int iter = 0; iter < NUM_ITERS; ++iter) {
                // prepare src data and recon data
                for (i = -16; i < height + 16; ++i) {
                    for (j = -16; j < width + 16; ++j) {
                        if (iter == 0)
                            output[i * stride + j] = input[i * stride + j] =
                                rnd.random();
                        else if (iter == 1)
                            output[i * stride + j] = input[i * stride + j] = 0;
                        else {
                            input[i * stride + j] = rnd.random();
                            output[i * stride + j] = rnd.random();
                        }
                    }
                }

                for (int32_t ep = 0; ep < SGRPROJ_PARAMS; ++ep) {
                    // apply selfguided filter to get A and b
                    for (k = 0; k < height; k += pu_height) {
                        for (j = 0; j < width; j += pu_width) {
                            int32_t w = AOMMIN(pu_width, width - j);
                            int32_t h = AOMMIN(pu_height, height - k);
                            Sample *output_p = output + k * out_stride + j;
                            int32_t *flt0_p = flt0 + k * flt_stride + j;
                            int32_t *flt1_p = flt1 + k * flt_stride + j;
                            assert(w * h <= RESTORATION_UNITPELS_MAX);

                            svt_av1_selfguided_restoration_c(
                                (uint8_t *)output_p,
                                w,
                                h,
                                out_stride,
                                flt0_p,
                                flt1_p,
                                flt_stride,
                                ep,
                                8,
                                0);
                        }
                    }

                    int32_t xqd_c[2] = {0};
                    int32_t xqd_asm[2] = {0};
                    const SgrParamsType *const params =
                        &svt_aom_eb_sgr_params[ep];
                    uint8_t *input_p = sizeof(*input) == sizeof(uint16_t)
                                           ? (CONVERT_TO_BYTEPTR(input))
                                           : (uint8_t *)input;
                    uint8_t *output_p = sizeof(*output) == sizeof(uint16_t)
                                            ? (CONVERT_TO_BYTEPTR(output))
                                            : (uint8_t *)output;
                    int32_t use_highbitdepth =
                        sizeof(*input) == sizeof(uint16_t) ? 1 : 0;

                    svt_get_proj_subspace_c(input_p,
                                            width,
                                            height,
                                            stride,
                                            output_p,
                                            out_stride,
                                            use_highbitdepth,
                                            flt0,
                                            flt_stride,
                                            flt1,
                                            flt_stride,
                                            xqd_c,
                                            params);
                    test_impl_(input_p,
                               width,
                               height,
                               stride,
                               output_p,
                               out_stride,
                               use_highbitdepth,
                               flt0,
                               flt_stride,
                               flt1,
                               flt_stride,
                               xqd_asm,
                               params);
                    ASSERT_EQ(xqd_c[0], xqd_asm[0])
                        << "xqd_asm[0] does not match with xqd_asm[0] with "
                           "iter "
                        << iter << " ep " << ep;
                    ASSERT_EQ(xqd_c[1], xqd_asm[1])
                        << "xqd_asm[1] does not match with xqd_asm[1] with "
                           "iter "
                        << iter << " ep " << ep;
                }
            }
        }
    }

  private:
    static const int32_t height = 256, stride = 300, out_stride = 300;
    GetProjSubspaceFunc test_impl_;
    Sample *input_;
    Sample *output_;
    int32_t *tmpbuf_;
};

using GetProjSubspaceTestLbd = GetProjSubspaceTest<uint8_t>;
using GetProjSubspaceTestHbd = GetProjSubspaceTest<uint16_t>;

TEST_P(GetProjSubspaceTestLbd, MatchTest) {
    run_test();
}
TEST_P(GetProjSubspaceTestHbd, MatchTest) {
    run_test();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(AVX2, GetProjSubspaceTestLbd,
                         ::testing::Values(svt_get_proj_subspace_avx2));

INSTANTIATE_TEST_SUITE_P(AVX2, GetProjSubspaceTestHbd,
                         ::testing::Values(svt_get_proj_subspace_avx2));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, GetProjSubspaceTestLbd,
                         ::testing::Values(svt_get_proj_subspace_neon));
INSTANTIATE_TEST_SUITE_P(NEON, GetProjSubspaceTestHbd,
                         ::testing::Values(svt_get_proj_subspace_neon));
#endif  // ARCH_AARCH64

}  // namespace
