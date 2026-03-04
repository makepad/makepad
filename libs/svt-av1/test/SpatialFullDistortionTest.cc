/*
 * Copyright(c) 2019 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#include "random.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "picture_operators_c.h"
#include "pic_operators.h"
#include "unit_test_utility.h"
#include "util.h"

namespace {
using svt_av1_test_tool::SVTRandom;

typedef uint64_t (*SpatialFullDistortionKernelFunc)(
    uint8_t *input, uint32_t input_offset, uint32_t input_stride,
    uint8_t *recon, int32_t recon_offset, uint32_t recon_stride,
    uint32_t area_width, uint32_t area_height);

typedef enum { VAL_MIN, VAL_MAX, VAL_RANDOM } TestPattern;
typedef std::tuple<uint32_t, uint32_t> AreaSize;

/**
 * @Brief Base class for SpatialFullDistortionFunc test.
 */
class SpatialFullDistortionFuncTestBase : public ::testing::Test {
    void SetUp() override {
        input_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        recon_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        input_test_size_ = MAX_SB_SIZE * input_stride_;
        recon_test_size_ = MAX_SB_SIZE * recon_stride_;
        input_ = reinterpret_cast<uint8_t *>(
            malloc(sizeof(*input_) * input_test_size_));
        recon_ = reinterpret_cast<uint8_t *>(
            malloc(sizeof(*recon_) * recon_test_size_));
    }
    void TearDown() override {
        free(recon_);
        free(input_);
    }

  protected:
    virtual void RunCheckOutput(TestPattern pattern) = 0;
    virtual void RunSpeedTest() = 0;

    virtual void init_data(TestPattern pattern) {
        const uint8_t mask = (1 << 8) - 1;
        switch (pattern) {
        case VAL_MIN: {
            memset(input_, 0, input_test_size_ * sizeof(input_[0]));
            memset(recon_, 0, recon_test_size_ * sizeof(recon_[0]));
            break;
        }
        case VAL_MAX: {
            memset(input_, mask, input_test_size_ * sizeof(input_[0]));
            memset(recon_, mask, recon_test_size_ * sizeof(recon_[0]));
            break;
        }
        case VAL_RANDOM: {
            svt_buf_random_u8(input_, input_test_size_);
            svt_buf_random_u8(recon_, recon_test_size_);
            break;
        }
        default: break;
        }
    }

    int input_test_size_, recon_test_size_;
    uint8_t *input_;
    uint8_t *recon_;
    uint32_t input_stride_;
    uint32_t recon_stride_;
    uint32_t area_width_, area_height_;
};

AreaSize TEST_AREA_SIZES[] = {
    AreaSize(4, 4),    AreaSize(4, 8),    AreaSize(8, 4),   AreaSize(8, 8),
    AreaSize(16, 16),  AreaSize(12, 16),  AreaSize(4, 16),  AreaSize(16, 4),
    AreaSize(16, 8),   AreaSize(20, 16),  AreaSize(24, 16), AreaSize(28, 16),
    AreaSize(8, 16),   AreaSize(32, 32),  AreaSize(32, 8),  AreaSize(16, 32),
    AreaSize(8, 32),   AreaSize(32, 16),  AreaSize(16, 64), AreaSize(64, 16),
    AreaSize(64, 64),  AreaSize(64, 32),  AreaSize(32, 64), AreaSize(128, 128),
    AreaSize(96, 128), AreaSize(64, 128), AreaSize(128, 64)};

typedef std::tuple<AreaSize, SpatialFullDistortionKernelFunc>
    SpatialKernelTestParam;

/**
 * @brief Unit test for spatial distortion calculation functions include:
 *  - svt_spatial_full_distortion_kernel_{avx2,avx512}
 *
 *
 * Test strategy:
 *  This test case combine different area width{4-128} x area
 * height{4-128} and different test pattern(VAL_MIN, VAL_MAX, VAL_RANDOM). Check
 * the result by compare result from reference function and avx2/sse2 function.
 *
 *
 * Expect result:
 *  Results from reference function and avx2/avx512 function are
 * equal.
 *
 *
 * Test cases:
 *  Width {4, 8, 12, 16, 20, 24, 28, 32, 64, 96, 128} x height{ 4, 8, 16, 32,
 * 64, 128} Test vector pattern {VAL_MIN, VAL_MIN, VAL_RANDOM}
 *
 */
class SpatialFullDistortionKernelFuncTest
    : public SpatialFullDistortionFuncTestBase,
      public ::testing::WithParamInterface<SpatialKernelTestParam> {
  public:
    SpatialFullDistortionKernelFuncTest() {
        area_width_ = std::get<0>(TEST_GET_PARAM(0));
        area_height_ = std::get<1>(TEST_GET_PARAM(0));
        test_func_ = TEST_GET_PARAM(1);
    }

    ~SpatialFullDistortionKernelFuncTest() {
    }

  protected:
    void RunCheckOutput(TestPattern pattern);
    void RunSpeedTest();
    SpatialFullDistortionKernelFunc test_func_;
};

void SpatialFullDistortionKernelFuncTest::RunCheckOutput(TestPattern pattern) {
    for (int i = 0; i < 10; i++) {
        init_data(pattern);
        const uint64_t dist_test = test_func_(input_,
                                              0,
                                              input_stride_,
                                              recon_,
                                              0,
                                              recon_stride_,
                                              area_width_,
                                              area_height_);
        const uint64_t dist_c =
            svt_spatial_full_distortion_kernel_c(input_,
                                                 0,
                                                 input_stride_,
                                                 recon_,
                                                 0,
                                                 recon_stride_,
                                                 area_width_,
                                                 area_height_);

        EXPECT_EQ(dist_test, dist_c)
            << "Compare Spatial distortion result error";
    }
}

void SpatialFullDistortionKernelFuncTest::RunSpeedTest() {
    uint64_t dist_org, dist_opt;
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t middle_time_seconds, middle_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;

    init_data(VAL_RANDOM);

    for (uint32_t area_width = 4; area_width <= 128; area_width += 4) {
        const uint32_t area_height = area_width;
        const int num_loops = 1000000000 / (area_width * area_height);
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (int i = 0; i < num_loops; ++i) {
            dist_org = svt_spatial_full_distortion_kernel_c(input_,
                                                            0,
                                                            input_stride_,
                                                            recon_,
                                                            0,
                                                            recon_stride_,
                                                            area_width,
                                                            area_height);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (int i = 0; i < num_loops; ++i) {
            dist_opt = test_func_(input_,
                                  0,
                                  input_stride_,
                                  recon_,
                                  0,
                                  recon_stride_,
                                  area_width,
                                  area_height);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        EXPECT_EQ(dist_org, dist_opt) << area_width << "x" << area_height;

        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);
        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_spatial_full_distortion_kernel_c  (%dx%d) : %6.2f\n",
               area_width,
               area_height,
               1000000 * time_c / num_loops);
        printf(
            "    svt_spatial_full_distortion_kernel_opt(%dx%d) : %6.2f   "
            "(Comparison: %5.2fx)\n",
            area_width,
            area_height,
            1000000 * time_o / num_loops,
            time_c / time_o);
    }
}
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(
    SpatialFullDistortionKernelFuncTest);

TEST_P(SpatialFullDistortionKernelFuncTest, Random) {
    RunCheckOutput(VAL_RANDOM);
}

TEST_P(SpatialFullDistortionKernelFuncTest, ExtremeMin) {
    RunCheckOutput(VAL_MIN);
}

TEST_P(SpatialFullDistortionKernelFuncTest, ExtremeMax) {
    RunCheckOutput(VAL_MAX);
}

TEST_P(SpatialFullDistortionKernelFuncTest, DISABLED_Speed) {
    RunSpeedTest();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, SpatialFullDistortionKernelFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_spatial_full_distortion_kernel_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, SpatialFullDistortionKernelFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_spatial_full_distortion_kernel_avx2)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, SpatialFullDistortionKernelFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_spatial_full_distortion_kernel_avx512)));
#endif
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, SpatialFullDistortionKernelFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_spatial_full_distortion_kernel_neon)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, SpatialFullDistortionKernelFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_spatial_full_distortion_kernel_neon_dotprod)));
#endif  // HAVE_NEON_DOTPROD
#endif  // ARCH_AARCH64

class FullDistortionKernel16BitsFuncTest
    : public SpatialFullDistortionFuncTestBase,
      public ::testing::WithParamInterface<SpatialKernelTestParam> {
    void SetUp() override {
        input_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        recon_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        input_test_size_ = MAX_SB_SIZE * input_stride_ * 2;
        recon_test_size_ = MAX_SB_SIZE * recon_stride_ * 2;
        input_ = reinterpret_cast<uint8_t *>(
            malloc(sizeof(*input_) * input_test_size_));
        recon_ = reinterpret_cast<uint8_t *>(
            malloc(sizeof(*recon_) * recon_test_size_));
    }

    virtual void init_data(TestPattern pattern) override {
        /// Support up to 10 bit depth
        const uint16_t mask = (1 << 10) - 1;
        uint16_t *input_16bit = (uint16_t *)input_;
        uint16_t *recon_16bit = (uint16_t *)recon_;
        SVTRandom rnd = SVTRandom(0, mask);

        switch (pattern) {
        case VAL_MIN: {
            for (int i = 0; i < (input_test_size_ / 2); i++)
                input_16bit[i] = 0;
            for (int i = 0; i < (recon_test_size_ / 2); i++)
                recon_16bit[i] = mask;
            break;
        }
        case VAL_MAX: {
            for (int i = 0; i < (input_test_size_ / 2); i++)
                input_16bit[i] = mask;
            for (int i = 0; i < (recon_test_size_ / 2); i++)
                recon_16bit[i] = 0;
            break;
        }
        case VAL_RANDOM: {
            for (int i = 0; i < (input_test_size_ / 2); i++)
                input_16bit[i] = rnd.random();
            for (int i = 0; i < (recon_test_size_ / 2); i++)
                recon_16bit[i] = rnd.random();
            break;
        }
        default: break;
        }
    }

  public:
    FullDistortionKernel16BitsFuncTest() {
        area_width_ = std::get<0>(TEST_GET_PARAM(0));
        area_height_ = std::get<1>(TEST_GET_PARAM(0));
        test_func_ = TEST_GET_PARAM(1);
    }

    ~FullDistortionKernel16BitsFuncTest() {
    }

  protected:
    void RunCheckOutput(TestPattern pattern) override;
    void RunSpeedTest() override;
    SpatialFullDistortionKernelFunc test_func_;
};

void FullDistortionKernel16BitsFuncTest::RunCheckOutput(TestPattern pattern) {
    for (int i = 0; i < 10; i++) {
        init_data(pattern);
        const uint64_t dist_test = test_func_(input_,
                                              0,
                                              input_stride_,
                                              recon_,
                                              0,
                                              recon_stride_,
                                              area_width_,
                                              area_height_);
        const uint64_t dist_c =
            svt_full_distortion_kernel16_bits_c(input_,
                                                0,
                                                input_stride_,
                                                recon_,
                                                0,
                                                recon_stride_,
                                                area_width_,
                                                area_height_);

        EXPECT_EQ(dist_test, dist_c)
            << "Compare Full distortion kernel 16 bits result error";
    }
}

void FullDistortionKernel16BitsFuncTest::RunSpeedTest() {
    uint64_t dist_org, dist_opt;
    double time_c, time_o;
    uint64_t start_time_seconds, start_time_useconds;
    uint64_t middle_time_seconds, middle_time_useconds;
    uint64_t finish_time_seconds, finish_time_useconds;
    init_data(VAL_RANDOM);

    for (uint32_t area_width = 4; area_width <= 128; area_width += 4) {
        const uint32_t area_height = area_width;
        const int num_loops = 1000000000 / (area_width * area_height);
        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (int i = 0; i < num_loops; ++i) {
            dist_org = svt_full_distortion_kernel16_bits_c(input_,
                                                           0,
                                                           input_stride_,
                                                           recon_,
                                                           0,
                                                           recon_stride_,
                                                           area_width,
                                                           area_height);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (int i = 0; i < num_loops; ++i) {
            dist_opt = test_func_(input_,
                                  0,
                                  input_stride_,
                                  recon_,
                                  0,
                                  recon_stride_,
                                  area_width,
                                  area_height);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        EXPECT_EQ(dist_org, dist_opt) << area_width << "x" << area_height;

        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);
        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_full_distortion_kernel16_bits_c  (%dx%d) : %6.2f\n",
               area_width,
               area_height,
               1000000 * time_c / num_loops);
        printf(
            "    svt_full_distortion_kernel16_bits_opt(%dx%d) : %6.2f   "
            "(Comparison: %5.2fx)\n",
            area_width,
            area_height,
            1000000 * time_o / num_loops,
            time_c / time_o);
    }
}
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(
    FullDistortionKernel16BitsFuncTest);

TEST_P(FullDistortionKernel16BitsFuncTest, Random) {
    RunCheckOutput(VAL_RANDOM);
}

TEST_P(FullDistortionKernel16BitsFuncTest, ExtremeMin) {
    RunCheckOutput(VAL_MIN);
}

TEST_P(FullDistortionKernel16BitsFuncTest, ExtremeMax) {
    RunCheckOutput(VAL_MAX);
}

TEST_P(FullDistortionKernel16BitsFuncTest, DISABLED_Speed) {
    RunSpeedTest();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, FullDistortionKernel16BitsFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_full_distortion_kernel16_bits_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, FullDistortionKernel16BitsFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_full_distortion_kernel16_bits_avx2)));

#endif

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, FullDistortionKernel16BitsFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_full_distortion_kernel16_bits_neon)));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, FullDistortionKernel16BitsFuncTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_AREA_SIZES),
        ::testing::Values(svt_full_distortion_kernel16_bits_sve)));
#endif  // HAVE_SVE
#endif

typedef void (*FullDistortionKernel32BitsFunc)(
    int32_t *coeff, uint32_t coeff_stride, int32_t *recon_coeff,
    uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL],
    uint32_t area_width, uint32_t area_height);

class FullDistortionKernel32Bits
    : public ::testing::TestWithParam<FullDistortionKernel32BitsFunc> {
  public:
    FullDistortionKernel32Bits() : func_(GetParam()) {
    }

    ~FullDistortionKernel32Bits() {};

    void SetUp() {
        coeff_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        recon_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        coeff = reinterpret_cast<int32_t *>(
            malloc(sizeof(*coeff) * MAX_SB_SIZE * coeff_stride_));
        recon = reinterpret_cast<int32_t *>(
            malloc(sizeof(*recon) * MAX_SB_SIZE * recon_stride_));
    }
    void TearDown() {
        free(recon);
        free(coeff);
    }

  protected:
    void RunCheckOutput();

    void init_data() {
        svt_buf_random_s32_with_max(
            coeff, MAX_SB_SIZE * coeff_stride_, (1 << 17));
        svt_buf_random_s32_with_max(
            recon, MAX_SB_SIZE * recon_stride_, (1 << 17));
    }

    uint64_t result_ref[DIST_CALC_TOTAL];
    uint64_t result_mod[DIST_CALC_TOTAL];
    FullDistortionKernel32BitsFunc func_;
    int32_t *coeff;
    int32_t *recon;
    uint32_t coeff_stride_;
    uint32_t recon_stride_;
};

void FullDistortionKernel32Bits::RunCheckOutput() {
    for (int i = 0; i < 10; i++) {
        init_data();
        for (uint32_t area_width = 4; area_width <= 128; area_width += 4) {
            for (uint32_t area_height = 4; area_height <= 128;
                 area_height += 4) {
                svt_full_distortion_kernel32_bits_c(coeff,
                                                    coeff_stride_,
                                                    recon,
                                                    recon_stride_,
                                                    result_ref,
                                                    area_width,
                                                    area_height);
                func_(coeff,
                      coeff_stride_,
                      recon,
                      recon_stride_,
                      result_mod,
                      area_width,
                      area_height);

                EXPECT_EQ(memcmp(result_ref, result_mod, sizeof(result_ref)),
                          0);
            }
        }
    }
}

TEST_P(FullDistortionKernel32Bits, CheckOutput) {
    RunCheckOutput();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, FullDistortionKernel32Bits,
    ::testing::Values(svt_full_distortion_kernel32_bits_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, FullDistortionKernel32Bits,
    ::testing::Values(svt_full_distortion_kernel32_bits_avx2));

#endif

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, FullDistortionKernel32Bits,
    ::testing::Values(svt_full_distortion_kernel32_bits_neon));

#endif

typedef void (*FullDistortionKernelCbfZero32BitsFunc)(
    int32_t *coeff, uint32_t coeff_stride,
    uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
    uint32_t area_height);

class FullDistortionKernelCbfZero32Bits
    : public ::testing::TestWithParam<FullDistortionKernelCbfZero32BitsFunc> {
  public:
    FullDistortionKernelCbfZero32Bits() : func_(GetParam()) {
    }

    ~FullDistortionKernelCbfZero32Bits() {};

    void SetUp() {
        coeff_stride_ = svt_create_random_aligned_stride(MAX_SB_SIZE, 64);
        coeff = reinterpret_cast<int32_t *>(
            malloc(sizeof(*coeff) * MAX_SB_SIZE * coeff_stride_));
    }
    void TearDown() {
        free(coeff);
    }

  protected:
    void RunCheckOutput();

    void init_data() {
        svt_buf_random_u32_with_max(
            (uint32_t *)coeff, MAX_SB_SIZE * coeff_stride_, (1 << 15));
    }

    uint64_t result_ref[DIST_CALC_TOTAL];
    uint64_t result_mod[DIST_CALC_TOTAL];
    FullDistortionKernelCbfZero32BitsFunc func_;
    int32_t *coeff;
    uint32_t coeff_stride_;
};

void FullDistortionKernelCbfZero32Bits::RunCheckOutput() {
    for (int i = 0; i < 10; i++) {
        init_data();
        for (uint32_t area_width = 4; area_width <= 128; area_width += 4) {
            for (uint32_t area_height = 4; area_height <= 128;
                 area_height += 4) {
                svt_full_distortion_kernel_cbf_zero32_bits_c(
                    coeff, coeff_stride_, result_ref, area_width, area_height);
                func_(
                    coeff, coeff_stride_, result_mod, area_width, area_height);

                EXPECT_EQ(memcmp(result_ref, result_mod, sizeof(result_ref)),
                          0);
            }
        }
    }
}
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(
    FullDistortionKernelCbfZero32Bits);

TEST_P(FullDistortionKernelCbfZero32Bits, CheckOutput) {
    RunCheckOutput();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, FullDistortionKernelCbfZero32Bits,
    ::testing::Values(svt_full_distortion_kernel_cbf_zero32_bits_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, FullDistortionKernelCbfZero32Bits,
    ::testing::Values(svt_full_distortion_kernel_cbf_zero32_bits_avx2));

#endif

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, FullDistortionKernelCbfZero32Bits,
    ::testing::Values(svt_full_distortion_kernel_cbf_zero32_bits_neon));

#endif  //  ARCH_AARCH64

}  // namespace
