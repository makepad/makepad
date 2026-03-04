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
 * @file WedgeUtilTest.cc
 *
 * @brief Unit test for util functions in wedge prediction:
 * - svt_av1_wedge_sign_from_residuals_avx2
 * - svt_av1_wedge_compute_delta_squares_avx2
 * - svt_av1_wedge_sse_from_residuals_avx2
 * - svt_aom_sum_squares_i16_sse2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include "gtest/gtest.h"
#include "common_utils.h"
#include "utility.h"
#include "aom_dsp_rtcd.h"
#include "random.h"
#include "util.h"

using svt_av1_test_tool::SVTRandom;
namespace {

// Choose the mask sign for a compound predictor.
class WedgeUtilTest : public ::testing::Test {
  public:
    void SetUp() override {
        r0 = (int16_t *)svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int16_t));
        r1 = (int16_t *)svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int16_t));
        m = (uint8_t *)svt_aom_memalign(32, MAX_SB_SQUARE);
    }

    void TearDown() override {
        svt_aom_free(r0);
        svt_aom_free(r1);
        svt_aom_free(m);
    }

  protected:
    uint8_t *m;       /* mask */
    int16_t *r0, *r1; /* two predicted residual */
};

// test svt_av1_wedge_sign_from_residuals
using WedgeSignFromResidualsFunc = int8_t (*)(const int16_t *ds,
                                              const uint8_t *m, int N,
                                              int64_t limit);

#define MAX_MASK_VALUE (1 << WEDGE_WEIGHT_BITS)

class WedgeSignFromResidualsTest
    : public WedgeUtilTest,
      public ::testing::WithParamInterface<WedgeSignFromResidualsFunc> {
  protected:
    WedgeSignFromResidualsTest() : test_func_(GetParam()) {
    }

    void wedge_sign_test(int N, int k) {
        DECLARE_ALIGNED(32, int16_t, ds[MAX_SB_SQUARE]);
        // pre-compute limit
        // MAX_MASK_VALUE/2 * (sum(r0**2) - sum(r1**2))
        int64_t limit;
        limit = (int64_t)svt_aom_sum_squares_i16_c(r0, N);
        limit -= (int64_t)svt_aom_sum_squares_i16_c(r1, N);
        limit *= (1 << WEDGE_WEIGHT_BITS) / 2;

        // calculate ds: r0**2 - r1**2
        for (int i = 0; i < N; ++i)
            ds[i] = clamp(r0[i] * r0[i] - r1[i] * r1[i], INT16_MIN, INT16_MAX);
        const int8_t ref_sign =
            svt_av1_wedge_sign_from_residuals_c(ds, m, N, limit);
        const int8_t tst_sign = test_func_(ds, m, N, limit);
        ASSERT_EQ(ref_sign, tst_sign)
            << "unit test for svt_av1_wedge_sign_from_residuals fail at "
               "iteration "
            << k;
    }

    void MaskSignRandomTest() {
        const int iterations = 10000;
        SVTRandom rnd(13, true);             // max residual is 13-bit
        SVTRandom m_rnd(0, MAX_MASK_VALUE);  // [0, MAX_MASK_VALUE]
        SVTRandom n_rnd(1, 8191 / 64);  // required by assembly implementation

        for (int k = 0; k < iterations; ++k) {
            // populate the residual buffer randomly
            for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                r0[i] = rnd.random();
                r1[i] = rnd.random();
                m[i] = m_rnd.random();
            }

            // N should be multiple of 64, required by
            // svt_av1_wedge_sign_from_residuals_avx2
            const int N = 64 * n_rnd.random();
            wedge_sign_test(N, k);
        }
    }

    void MaskSignExtremeTest() {
        const int int_13bit_max = (1 << 12) - 1;
        SVTRandom rnd(13, true);             // max residual is 13-bit
        SVTRandom m_rnd(0, MAX_MASK_VALUE);  // [0, MAX_MASK_VALUE]
        SVTRandom n_rnd(1, 8191 / 64);  // required by assembly implementation

        for (int k = 0; k < 4; ++k) {
            switch (k) {
            case 0:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = 0;
                    r1[i] = int_13bit_max;
                }
                break;
            case 1:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = int_13bit_max;
                    r1[i] = 0;
                }
                break;
            case 2:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = -int_13bit_max;
                    r1[i] = 0;
                }
                break;
            case 3:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = 0;
                    r1[i] = -int_13bit_max;
                }
                break;
            default: assert(0); break;
            }

            for (int i = 0; i < MAX_SB_SQUARE; ++i)
                m[i] = MAX_MASK_VALUE;

            // N should be multiple of 64, required by
            // svt_av1_wedge_sign_from_residuals_avx2
            const int N = 64 * n_rnd.random();

            wedge_sign_test(N, k);
        }
    }

    WedgeSignFromResidualsFunc test_func_;
};

TEST_P(WedgeSignFromResidualsTest, RandomTest) {
    MaskSignRandomTest();
}

TEST_P(WedgeSignFromResidualsTest, ExtremeTest) {
    MaskSignExtremeTest();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, WedgeSignFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sign_from_residuals_sse2));

INSTANTIATE_TEST_SUITE_P(
    AVX2, WedgeSignFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sign_from_residuals_avx2));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, WedgeSignFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sign_from_residuals_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, WedgeSignFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sign_from_residuals_sve));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

// test svt_av1_wedge_compute_delta_squares
using WedgeComputeDeltaSquaresFunc = void (*)(int16_t *d, const int16_t *a,
                                              const int16_t *b, int N);

class WedgeComputeDeltaSquaresTest
    : public WedgeUtilTest,
      public ::testing::WithParamInterface<WedgeComputeDeltaSquaresFunc> {
  protected:
    WedgeComputeDeltaSquaresTest() : test_func_(GetParam()) {
    }

    void ComputeDeltaSquareTest() {
        const int iterations = 10000;
        SVTRandom rnd(13, true);  // max residual is 13-bit
        SVTRandom n_rnd(1, MAX_SB_SQUARE / 64);
        DECLARE_ALIGNED(32, int16_t, ref_diff[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(32, int16_t, tst_diff[MAX_SB_SQUARE]);

        for (int k = 0; k < iterations; ++k) {
            // populate the residual buffer randomly
            for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                r0[i] = rnd.random();
                r1[i] = rnd.random();
            }
            memset(ref_diff, 0, sizeof(ref_diff));
            memset(tst_diff, 0, sizeof(ref_diff));

            // N should be multiple of 64, required by
            // svt_av1_wedge_compute_delta_squares
            const int N = 64 * n_rnd.random();

            svt_av1_wedge_compute_delta_squares_c(ref_diff, r0, r1, N);
            test_func_(tst_diff, r0, r1, N);

            // check the output
            for (int i = 0; i < N; ++i) {
                ASSERT_EQ(ref_diff[i], tst_diff[i])
                    << "unit test for svt_av1_wedge_compute_delta_squares "
                       "fail at "
                       "iteration "
                    << k;
            }
        }
    }

    WedgeComputeDeltaSquaresFunc test_func_;
};

// element-by-element calculate the difference of square
TEST_P(WedgeComputeDeltaSquaresTest, ComputeDeltaSquareTest) {
    ComputeDeltaSquareTest();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, WedgeComputeDeltaSquaresTest,
    ::testing::Values(svt_av1_wedge_compute_delta_squares_avx2));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, WedgeComputeDeltaSquaresTest,
    ::testing::Values(svt_av1_wedge_compute_delta_squares_neon));
#endif  // ARCH_AARCH64

// test svt_av1_wedge_sse_from_residuals
using WedgeSseFromResidualsFunc = uint64_t (*)(const int16_t *r1,
                                               const int16_t *d,
                                               const uint8_t *m, int N);

class WedgeSseFromResidualsTest
    : public WedgeUtilTest,
      public ::testing::WithParamInterface<WedgeSseFromResidualsFunc> {
  protected:
    WedgeSseFromResidualsTest() : test_func_(GetParam()) {
    }

    void SseFromResidualRandomTest() {
        const int iterations = 10000;
        SVTRandom rnd(13, true);             // max residual is 13-bit
        SVTRandom m_rnd(0, MAX_MASK_VALUE);  // [0, MAX_MASK_VALUE]
        SVTRandom n_rnd(1, MAX_SB_SQUARE / 64);

        for (int k = 0; k < iterations; ++k) {
            // populate the residual buffer randomly
            for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                r0[i] = rnd.random();
                r1[i] = rnd.random();
                m[i] = m_rnd.random();
            }

            // N should be multiple of 64, required by
            // svt_av1_wedge_sse_from_residuals
            const int N = 64 * n_rnd.random();

            uint64_t ref_sse = svt_av1_wedge_sse_from_residuals_c(r0, r1, m, N);
            uint64_t tst_sse = test_func_(r0, r1, m, N);

            // check output
            ASSERT_EQ(ref_sse, tst_sse)
                << "unit test for svt_av1_wedge_sse_from_residuals fail at "
                   "iteration "
                << k;
        }
    }

    void SseFromResidualExtremeTest() {
        const int int_13bit_max = (1 << 12) - 1;
        SVTRandom rnd(13, true);             // max residual is 13-bit
        SVTRandom m_rnd(0, MAX_MASK_VALUE);  // [0, MAX_MASK_VALUE]
        SVTRandom n_rnd(1, MAX_SB_SQUARE / 64);

        for (int k = 0; k < 4; ++k) {
            switch (k) {
            case 0:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = 0;
                    r1[i] = int_13bit_max;
                }
                break;
            case 1:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = int_13bit_max;
                    r1[i] = 0;
                }
                break;
            case 2:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = -int_13bit_max;
                    r1[i] = 0;
                }
                break;
            case 3:
                for (int i = 0; i < MAX_SB_SQUARE; ++i) {
                    r0[i] = 0;
                    r1[i] = -int_13bit_max;
                }
                break;
            default: assert(0); break;
            }

            for (int i = 0; i < MAX_SB_SQUARE; ++i)
                m[i] = MAX_MASK_VALUE;

            // N should be multiple of 64, required by
            // svt_av1_wedge_sse_from_residuals
            const int N = 64 * n_rnd.random();

            uint64_t ref_sse = svt_av1_wedge_sse_from_residuals_c(r0, r1, m, N);
            uint64_t tst_sse = test_func_(r0, r1, m, N);

            // check output
            ASSERT_EQ(ref_sse, tst_sse)
                << "unit test for svt_av1_wedge_sse_from_residuals fail at "
                   "iteration "
                << k;
        }
    }

    WedgeSseFromResidualsFunc test_func_;
};

// calculate the sse of two prediction combined with mask m
TEST_P(WedgeSseFromResidualsTest, RandomTest) {
    SseFromResidualRandomTest();
}

TEST_P(WedgeSseFromResidualsTest, ExtremeTest) {
    SseFromResidualExtremeTest();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, WedgeSseFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sse_from_residuals_sse2));

INSTANTIATE_TEST_SUITE_P(
    AVX2, WedgeSseFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sse_from_residuals_avx2));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, WedgeSseFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sse_from_residuals_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, WedgeSseFromResidualsTest,
    ::testing::Values(svt_av1_wedge_sse_from_residuals_sve));
#endif
#endif  // ARCH_AARCH64

typedef uint64_t (*AomSumSquaresI16Func)(const int16_t *, uint32_t);
typedef ::testing::tuple<BlockSize, AomSumSquaresI16Func> AomHSumSquaresParam;

class AomSumSquaresTest : public ::testing::TestWithParam<AomHSumSquaresParam> {
  public:
    AomSumSquaresTest() : rnd_(0, 255) {};
    virtual ~AomSumSquaresTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        AomSumSquaresI16Func test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, uint16_t, src_[MAX_SB_SQUARE]);
        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(src_, 0, sizeof(src_));
            for (int j = 0; j < width * height; j++) {
                src_[j] = rnd_.random();
            }

            uint64_t res_ref_ = svt_aom_sum_squares_i16_c((const int16_t *)src_,
                                                          width * height);

            uint64_t res_tst_ =
                test_impl((const int16_t *)src_, width * height);

            ASSERT_EQ(res_ref_, res_tst_);
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(AomSumSquaresTest, MatchTest) {
    run_test();
}

#if ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, AomSumSquaresTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_sum_squares_i16_sse2)));
#endif  // ARCH_X86_64

#if ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AomSumSquaresTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_sum_squares_i16_neon)));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, AomSumSquaresTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_sum_squares_i16_sve)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

}  // namespace
