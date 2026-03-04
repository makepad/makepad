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
 * @brief Unit test for chroma from luma prediction:
 * - svt_cfl_predict_hbd_avx2
 * - svt_cfl_predict_lbd_avx2
 * - svt_cfl_luma_subsampling_420_lbd_avx2
 * - svt_cfl_luma_subsampling_420_hbd_avx2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "common_utils.h"
#include "random.h"
#include "util.h"
namespace {
using svt_av1_test_tool::SVTRandom;

using CFL_PRED_HBD = void (*)(const int16_t *pred_buf_q3, uint16_t *pred,
                              int32_t pred_stride, uint16_t *dst,
                              int32_t dst_stride, int32_t alpha_q3,
                              int32_t bit_depth, int32_t width, int32_t height);
using CFL_PRED_LBD = void (*)(const int16_t *pred_buf_q3, uint8_t *pred,
                              int32_t pred_stride, uint8_t *dst,
                              int32_t dst_stride, int32_t alpha_q3,
                              int32_t bit_depth, int32_t width, int32_t height);
/**
 * @brief Unit test for chroma from luma prediction:
 * - svt_cfl_predict_hbd_avx2
 * - svt_cfl_predict_lbd_avx2
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
 * pred buffer and dst buffer: Fill with random values
 * TxSize: all the TxSize.
 * alpha_q3: [-16, 16]
 * BitDepth: 8bit and 10bit
 */
template <typename Sample, typename FuncType>
class CflPredTest : public ::testing::TestWithParam<FuncType> {
  public:
    virtual ~CflPredTest() {
    }

    void RunAllTest() {
        // for pred_buf, after sampling and subtracted from average
        SVTRandom pred_rnd(bd_ + 3 + 1, true);
        SVTRandom dst_rnd(8, false);
        for (int tx = TX_4X4; tx < TX_SIZES_ALL; ++tx) {
            const int c_w = tx_size_wide[tx];
            const int c_h = tx_size_high[tx];
            if (c_w > 32 || c_h > 32) {
                continue;
            }
            const int c_stride = CFL_BUF_LINE;
            memset(pred_buf_q3, 0, sizeof(pred_buf_q3));
            memset(dst_buf_ref_data_, 0, sizeof(dst_buf_ref_data_));
            memset(dst_buf_tst_data_, 0, sizeof(dst_buf_tst_data_));

            for (int alpha_q3 = -16; alpha_q3 <= 16; ++alpha_q3) {
                // prepare data
                // Implicit assumption: The dst_buf is supposed to be populated
                // by dc prediction before cfl prediction.
                const Sample rnd_dc = dst_rnd.random();
                for (int y = 0; y < c_h; ++y) {
                    for (int x = 0; x < c_w; ++x) {
                        pred_buf_q3[y * c_stride + x] =
                            (Sample)pred_rnd.random();
                        dst_buf_ref_[y * c_stride + x] =
                            dst_buf_tst_[y * c_stride + x] = rnd_dc;
                    }
                }

                ref_func_(pred_buf_q3,
                          dst_buf_ref_,
                          CFL_BUF_LINE,
                          dst_buf_ref_,
                          CFL_BUF_LINE,
                          alpha_q3,
                          bd_,
                          c_w,
                          c_h);
                tst_func_(pred_buf_q3,
                          dst_buf_tst_,
                          c_stride,
                          dst_buf_tst_,
                          c_stride,
                          alpha_q3,
                          bd_,
                          c_w,
                          c_h);

                for (int y = 0; y < c_h; ++y) {
                    for (int x = 0; x < c_w; ++x) {
                        ASSERT_EQ(dst_buf_ref_[y * c_stride + x],
                                  dst_buf_tst_[y * c_stride + x])
                            << "tx_size: " << tx << " alpha_q3 " << alpha_q3
                            << " expect " << dst_buf_ref_[y * c_stride + x]
                            << " got " << dst_buf_tst_[y * c_stride + x]
                            << " at [ " << x << " x " << y << " ]";
                    }
                }
            }
        }
    }

  protected:
    void common_init() {
        dst_buf_ref_ = reinterpret_cast<Sample *>(
            ((intptr_t)(dst_buf_ref_data_) + alignment - 1) & ~(alignment - 1));
        dst_buf_tst_ = reinterpret_cast<Sample *>(
            ((intptr_t)(dst_buf_tst_data_) + alignment - 1) & ~(alignment - 1));
    }

    static const int alignment = 32;
    int16_t pred_buf_q3[CFL_BUF_SQUARE];
    Sample dst_buf_ref_data_[CFL_BUF_SQUARE + alignment - 1];
    Sample dst_buf_tst_data_[CFL_BUF_SQUARE + alignment - 1];
    Sample *dst_buf_ref_;
    Sample *dst_buf_tst_;
    FuncType ref_func_;
    FuncType tst_func_;
    int bd_;
};

class LbdCflPredTest : public CflPredTest<uint8_t, CFL_PRED_LBD> {
  public:
    LbdCflPredTest() {
        bd_ = 8;
        ref_func_ = svt_cfl_predict_lbd_c;
        tst_func_ = GetParam();
        common_init();
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(LbdCflPredTest);

TEST_P(LbdCflPredTest, MatchTest) {
    RunAllTest();
}
#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(AVX2, LbdCflPredTest,
                         ::testing::Values(svt_cfl_predict_lbd_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, LbdCflPredTest,
                         ::testing::Values(svt_aom_cfl_predict_lbd_neon));
#endif  // ARCH_AARCH64

class HbdCflPredTest : public CflPredTest<uint16_t, CFL_PRED_HBD> {
  public:
    HbdCflPredTest() {
        bd_ = 10;
        ref_func_ = svt_cfl_predict_hbd_c;
        tst_func_ = GetParam();
        common_init();
    }
};

TEST_P(HbdCflPredTest, MatchTest) {
    RunAllTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(AVX2, HbdCflPredTest,
                         ::testing::Values(svt_cfl_predict_hbd_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, HbdCflPredTest,
                         ::testing::Values(svt_cfl_predict_hbd_neon));
#endif  // ARCH_AARCH64

typedef void (*AomUpsampledPredFunc)(MacroBlockD *,
                                     const struct AV1Common *const, int, int,
                                     const Mv *const, uint8_t *, int, int, int,
                                     int, const uint8_t *, int, int);

typedef ::testing::tuple<BlockSize, AomUpsampledPredFunc, int, int, int,
                         uint64_t>
    AomUpsampledPredParam;

class AomUpsampledPredTest
    : public ::testing::TestWithParam<AomUpsampledPredParam> {
  public:
    AomUpsampledPredTest() : rnd_(0, 255) {};
    virtual ~AomUpsampledPredTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        AomUpsampledPredFunc test_impl = TEST_GET_PARAM(1);
        int subpel_search = TEST_GET_PARAM(2);
        int subpel_x_q3 = TEST_GET_PARAM(3);
        int subpel_y_q3 = TEST_GET_PARAM(4);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, uint8_t, ref_[MAX_SB_SQUARE * 2]);
        DECLARE_ALIGNED(16, uint8_t, comp_pred_ref_[MAX_SB_SQUARE * 2]);
        DECLARE_ALIGNED(16, uint8_t, comp_pred_tst_[MAX_SB_SQUARE * 2]);

        memset(comp_pred_ref_, 1, sizeof(comp_pred_ref_));
        memset(comp_pred_tst_, 1, sizeof(comp_pred_tst_));

        // Function svt_aom_upsampled_pred call inside function pointer
        // which have to be set properly by
        // svt_aom_setup_common_rtcd_internal(), we want to test intrinsic
        // version of it, so feature flag is necessary
        uint64_t EbCpuFlags = TEST_GET_PARAM(5);
        svt_aom_setup_common_rtcd_internal(EbCpuFlags);

        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(ref_, 1, sizeof(ref_));
            for (int j = 0; j < width * height + 3 * width; j++) {
                ref_[j] = rnd_.random();
            }

            svt_aom_upsampled_pred_c(NULL,
                                     NULL,
                                     0,
                                     0,
                                     NULL,
                                     comp_pred_ref_,
                                     width,
                                     height,
                                     subpel_x_q3,
                                     subpel_y_q3,
                                     ref_ + 3 * width,
                                     width,
                                     subpel_search);
            test_impl(NULL,
                      NULL,
                      0,
                      0,
                      NULL,
                      comp_pred_tst_,
                      width,
                      height,
                      subpel_x_q3,
                      subpel_y_q3,
                      ref_ + 3 * width,
                      width,
                      subpel_search);

            ASSERT_EQ(
                0,
                memcmp(comp_pred_ref_, comp_pred_tst_, sizeof(comp_pred_ref_)));
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(AomUpsampledPredTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, AomUpsampledPredTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_aom_upsampled_pred_sse2),
        ::testing::Values((int)USE_2_TAPS, (int)USE_4_TAPS, (int)USE_8_TAPS),
        ::testing::Values(0, 1, 2), ::testing::Values(0, 1, 2),
        ::testing::Values(EB_CPU_FLAGS_SSSE3, EB_CPU_FLAGS_AVX2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AomUpsampledPredTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_upsampled_pred_neon),
                       ::testing::Values((int)USE_2_TAPS, (int)USE_4_TAPS,
                                         (int)USE_8_TAPS),
                       ::testing::Values(0, 1, 2), ::testing::Values(0, 1, 2),
                       ::testing::Values(EB_CPU_FLAGS_NEON)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, AomUpsampledPredTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_upsampled_pred_neon),
                       ::testing::Values((int)USE_2_TAPS, (int)USE_4_TAPS,
                                         (int)USE_8_TAPS),
                       ::testing::Values(0, 1, 2), ::testing::Values(0, 1, 2),
                       ::testing::Values(EB_CPU_FLAGS_NEON_DOTPROD)));
#endif  // HAVE_NEON_DOTPROD

#if HAVE_NEON_I8MM
INSTANTIATE_TEST_SUITE_P(
    NEON_I8MM, AomUpsampledPredTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_upsampled_pred_neon),
                       ::testing::Values((int)USE_2_TAPS, (int)USE_4_TAPS,
                                         (int)USE_8_TAPS),
                       ::testing::Values(0, 1, 2), ::testing::Values(0, 1, 2),
                       ::testing::Values(EB_CPU_FLAGS_NEON_I8MM)));
#endif  // HAVE_NEON_I8MM
#endif  // ARCH_AARCH64

typedef void (*CflLumaSubsamplingLbdFunc)(const uint8_t *, int32_t, int16_t *,
                                          int32_t, int32_t);
typedef ::testing::tuple<BlockSize, CflLumaSubsamplingLbdFunc>
    CflLumaSubsamplingLbdParam;

class CflLumaSubsamplingLbdTest
    : public ::testing::TestWithParam<CflLumaSubsamplingLbdParam> {
  public:
    CflLumaSubsamplingLbdTest() : rnd_(0, 255) {};
    virtual ~CflLumaSubsamplingLbdTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        CflLumaSubsamplingLbdFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        // CFL prediction only operates on blocks where
        // max(width, height) <= 32.
        if (width > 32 || height > 32)
            return;
        DECLARE_ALIGNED(16, uint8_t, input[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, int16_t, output_q3_ref_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, int16_t, output_q3_tst_[MAX_SB_SQUARE]);

        memset(output_q3_ref_, 1, sizeof(output_q3_ref_));
        memset(output_q3_tst_, 1, sizeof(output_q3_tst_));

        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(input, 1, sizeof(input));
            for (int j = 0; j < MAX_SB_SQUARE; j++) {
                input[j] = rnd_.random();
            }

            svt_cfl_luma_subsampling_420_lbd_c(
                input, width, output_q3_ref_, width, height);

            test_impl(input, width, output_q3_tst_, width, height);

            ASSERT_EQ(
                0,
                memcmp(output_q3_ref_, output_q3_tst_, sizeof(output_q3_ref_)));
        }
    }

  private:
    SVTRandom rnd_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(CflLumaSubsamplingLbdTest);

TEST_P(CflLumaSubsamplingLbdTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, CflLumaSubsamplingLbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_cfl_luma_subsampling_420_lbd_avx2)));
#endif

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CflLumaSubsamplingLbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_cfl_luma_subsampling_420_lbd_neon)));
#endif

typedef void (*CflLumaSubsamplingHbdFunc)(const uint16_t *, int32_t, int16_t *,
                                          int32_t, int32_t);
typedef ::testing::tuple<BlockSize, CflLumaSubsamplingHbdFunc>
    CflLumaSubsamplingHbdParam;

class CflLumaSubsamplingHbdTest
    : public ::testing::TestWithParam<CflLumaSubsamplingHbdParam> {
  public:
    CflLumaSubsamplingHbdTest() : rnd_(0, 1023) {};
    virtual ~CflLumaSubsamplingHbdTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        CflLumaSubsamplingHbdFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        // CFL prediction only operates on blocks where
        // max(width, height) <= 32.
        if (width > 32 || height > 32)
            return;
        DECLARE_ALIGNED(16, uint16_t, input[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, int16_t, output_q3_ref_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, int16_t, output_q3_tst_[MAX_SB_SQUARE]);

        memset(output_q3_ref_, 1, sizeof(output_q3_ref_));
        memset(output_q3_tst_, 1, sizeof(output_q3_tst_));

        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(input, 1, sizeof(input));
            for (int j = 0; j < MAX_SB_SQUARE; j++) {
                input[j] = rnd_.random();
            }

            svt_cfl_luma_subsampling_420_hbd_c(
                input, width, output_q3_ref_, width, height);

            test_impl(input, width, output_q3_tst_, width, height);

            ASSERT_EQ(
                0,
                memcmp(output_q3_ref_, output_q3_tst_, sizeof(output_q3_ref_)));
        }
    }

  private:
    SVTRandom rnd_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(CflLumaSubsamplingHbdTest);

TEST_P(CflLumaSubsamplingHbdTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, CflLumaSubsamplingHbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_cfl_luma_subsampling_420_hbd_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CflLumaSubsamplingHbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_cfl_luma_subsampling_420_hbd_neon)));
#endif  // ARCH_AARCH64

}  // namespace
