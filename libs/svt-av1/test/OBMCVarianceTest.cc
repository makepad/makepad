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
 * @file OBMCVarianceTest.cc
 *
 * @brief Unit test for obmc variance functions:
 * - svt_aom_obmc_variance{4-128}x{4-128}_{c, avx2}
 * - svt_aom_obmc_sub_pixel_variance{4-128}x{4-128}_{c, sse4_1}
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/
#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "random.h"
#include "util.h"
#include "utility.h"
#include "filter.h"

#include "enc_inter_prediction.h"

using std::tuple;
using svt_av1_test_tool::SVTRandom;  // to generate the random

namespace {
#if CONFIG_ENABLE_OBMC
static const int MaskMax = 64;

using ObmcVarFunc = unsigned int (*)(const uint8_t *pre, int pre_stride,
                                     const int32_t *wsrc, const int32_t *mask,
                                     unsigned int *sse);
using ObmcVarParam = tuple<ObmcVarFunc, ObmcVarFunc>;

class OBMCVarianceTest : public ::testing::TestWithParam<ObmcVarParam> {
  public:
    OBMCVarianceTest()
        : rnd_(8, false),
          rnd_msk_(0, MaskMax * MaskMax + 1),
          func_ref_(TEST_GET_PARAM(0)),
          func_tst_(TEST_GET_PARAM(1)) {
        pre_ = reinterpret_cast<uint8_t *>(svt_aom_memalign(32, MAX_SB_SQUARE));
        wsrc_buf_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int32_t)));
        mask_buf_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int32_t)));
    }

    ~OBMCVarianceTest() {
        if (pre_)
            svt_aom_free(pre_);
        if (wsrc_buf_)
            svt_aom_free(wsrc_buf_);
        if (mask_buf_)
            svt_aom_free(mask_buf_);
    }

  protected:
    void run_test(size_t test_num) {
        for (size_t i = 0; i < test_num; i++) {
            for (size_t j = 0; j < MAX_SB_SQUARE; j++) {
                pre_[j] = rnd_.random();
                wsrc_buf_[j] = rnd_.random() * rnd_msk_.random();
                mask_buf_[j] = rnd_msk_.random();
            }

            unsigned int sse_ref = 0, sse_tst = 0;
            uint32_t var_ref =
                func_ref_(pre_, MAX_SB_SIZE, wsrc_buf_, mask_buf_, &sse_ref);
            uint32_t var_tst =
                func_tst_(pre_, MAX_SB_SIZE, wsrc_buf_, mask_buf_, &sse_tst);

            ASSERT_EQ(var_tst, var_ref) << "compare var error";
            ASSERT_EQ(sse_tst, sse_ref) << "compare sse error";
        }
    }

  protected:
    SVTRandom rnd_;
    SVTRandom rnd_msk_;
    ObmcVarFunc func_ref_;
    ObmcVarFunc func_tst_;
    uint8_t *pre_;
    int32_t *wsrc_buf_;
    int32_t *mask_buf_;
};

TEST_P(OBMCVarianceTest, RunCheckOutput) {
    run_test(1000);
};

#define OBMC_VAR_FUNC(W, H, opt) svt_aom_obmc_variance##W##x##H##_##opt
#define GEN_OBMC_VAR_TEST_PARAM(W, H, opt) \
    ObmcVarParam(OBMC_VAR_FUNC(W, H, c), OBMC_VAR_FUNC(W, H, opt))
#define GEN_TEST_PARAMS(GEN_PARAM, opt)                    \
    {                                                      \
        GEN_PARAM(128, 128, opt), GEN_PARAM(128, 64, opt), \
        GEN_PARAM(64, 128, opt),  GEN_PARAM(64, 64, opt),  \
        GEN_PARAM(64, 32, opt),   GEN_PARAM(32, 64, opt),  \
        GEN_PARAM(32, 32, opt),   GEN_PARAM(32, 16, opt),  \
        GEN_PARAM(16, 32, opt),   GEN_PARAM(16, 16, opt),  \
        GEN_PARAM(16, 8, opt),    GEN_PARAM(8, 16, opt),   \
        GEN_PARAM(8, 8, opt),     GEN_PARAM(8, 4, opt),    \
        GEN_PARAM(4, 8, opt),     GEN_PARAM(4, 4, opt),    \
        GEN_PARAM(4, 16, opt),    GEN_PARAM(16, 4, opt),   \
        GEN_PARAM(8, 32, opt),    GEN_PARAM(32, 8, opt),   \
        GEN_PARAM(16, 64, opt),   GEN_PARAM(64, 16, opt)}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, OBMCVarianceTest,
    ::testing::ValuesIn(GEN_TEST_PARAMS(GEN_OBMC_VAR_TEST_PARAM, sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, OBMCVarianceTest,
    ::testing::ValuesIn(GEN_TEST_PARAMS(GEN_OBMC_VAR_TEST_PARAM, avx2)));
#endif  // ARCH_x86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, OBMCVarianceTest,
    ::testing::ValuesIn(GEN_TEST_PARAMS(GEN_OBMC_VAR_TEST_PARAM, neon)));
#endif  // ARCH_AARCH64

using ObmcSubPixVarFunc = unsigned int (*)(const uint8_t *pre, int pre_stride,
                                           int xoffset, int yoffset,
                                           const int32_t *wsrc,
                                           const int32_t *mask,
                                           unsigned int *sse);
using ObmcSubPixVarParam = tuple<ObmcSubPixVarFunc, ObmcSubPixVarFunc>;

class OBMCSubPixelVarianceTest
    : public ::testing::TestWithParam<ObmcSubPixVarParam> {
  public:
    OBMCSubPixelVarianceTest()
        : rnd_(8, false),
          rnd_msk_(0, MaskMax * MaskMax + 1),
          rnd_offset_(0, BIL_SUBPEL_SHIFTS - 1),
          func_ref_(TEST_GET_PARAM(0)),
          func_tst_(TEST_GET_PARAM(1)) {
        pre_ = reinterpret_cast<uint8_t *>(svt_aom_memalign(32, MAX_SB_SQUARE));
        wsrc_buf_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int32_t)));
        mask_buf_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int32_t)));
    }

    ~OBMCSubPixelVarianceTest() {
        if (pre_)
            svt_aom_free(pre_);
        if (wsrc_buf_)
            svt_aom_free(wsrc_buf_);
        if (mask_buf_)
            svt_aom_free(mask_buf_);
    }

  protected:
    void run_test(size_t test_num) {
        for (size_t i = 0; i < test_num; i++) {
            for (size_t j = 0; j < MAX_SB_SQUARE; j++) {
                pre_[j] = rnd_.random();
                wsrc_buf_[j] = rnd_.random() * rnd_msk_.random();
                mask_buf_[j] = rnd_msk_.random();
            }

            int offset_x = rnd_offset_.random();
            int offset_y = rnd_offset_.random();
            unsigned int sse_ref = 0, sse_tst = 0;
            uint32_t var_ref = func_ref_(pre_,
                                         MAX_SB_SIZE,
                                         offset_x,
                                         offset_y,
                                         wsrc_buf_,
                                         mask_buf_,
                                         &sse_ref);
            uint32_t var_tst = func_tst_(pre_,
                                         MAX_SB_SIZE,
                                         offset_x,
                                         offset_y,
                                         wsrc_buf_,
                                         mask_buf_,
                                         &sse_tst);

            ASSERT_EQ(var_tst, var_ref)
                << "compare var error at offset x=" << offset_x
                << " y=" << offset_y;
            ASSERT_EQ(sse_tst, sse_ref)
                << "compare sse error at offset x=" << offset_x
                << " y=" << offset_y;
        }
    }

  protected:
    SVTRandom rnd_;
    SVTRandom rnd_msk_;
    SVTRandom rnd_offset_;
    ObmcSubPixVarFunc func_ref_;
    ObmcSubPixVarFunc func_tst_;
    uint8_t *pre_;
    int32_t *wsrc_buf_;
    int32_t *mask_buf_;
};

TEST_P(OBMCSubPixelVarianceTest, RunCheckOutput) {
    run_test(1000);
};

#define OBMC_SUB_PIX_VAR_FUNC(W, H, opt) \
    svt_aom_obmc_sub_pixel_variance##W##x##H##_##opt
#define GEN_OBMC_SUB_PIX_VAR_TEST_PARAM(W, H, opt)     \
    ObmcSubPixVarParam(OBMC_SUB_PIX_VAR_FUNC(W, H, c), \
                       OBMC_SUB_PIX_VAR_FUNC(W, H, opt))

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, OBMCSubPixelVarianceTest,
                         ::testing::ValuesIn(GEN_TEST_PARAMS(
                             GEN_OBMC_SUB_PIX_VAR_TEST_PARAM, sse4_1)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, OBMCSubPixelVarianceTest,
                         ::testing::ValuesIn(GEN_TEST_PARAMS(
                             GEN_OBMC_SUB_PIX_VAR_TEST_PARAM, neon)));
#endif  // ARCH_AARCH64
using CalcTargetWeightedPredFn = void (*)(uint8_t, MacroBlockD *, int, uint8_t,
                                          MbModeInfo *, void *);
using CalcTargetWeightedPredParam =
    tuple<int, CalcTargetWeightedPredFn, CalcTargetWeightedPredFn>;

class CalcTargetWeightedPredTest
    : public ::testing::TestWithParam<CalcTargetWeightedPredParam> {
  public:
    CalcTargetWeightedPredTest()
        : rnd_(10, false),
          width_(TEST_GET_PARAM(0)),
          func_ref_(TEST_GET_PARAM(1)),
          func_tst_(TEST_GET_PARAM(2)) {
        mask_buf_ref = reinterpret_cast<int32_t *>(
            malloc(2 * MAX_SB_SQUARE * sizeof(int32_t)));
        mask_buf_tst = reinterpret_cast<int32_t *>(
            malloc(2 * MAX_SB_SQUARE * sizeof(int32_t)));
        wsrc_buf_ref = reinterpret_cast<int32_t *>(
            malloc(2 * MAX_SB_SQUARE * sizeof(int32_t)));
        wsrc_buf_tst = reinterpret_cast<int32_t *>(
            malloc(2 * MAX_SB_SQUARE * sizeof(int32_t)));
        tmp_ref = reinterpret_cast<uint8_t *>(
            malloc(2 * MAX_SB_SQUARE * sizeof(uint8_t)));
        tmp_tst = reinterpret_cast<uint8_t *>(
            malloc(2 * MAX_SB_SQUARE * sizeof(uint8_t)));

        stride = MAX_SB_SIZE;
    }

    ~CalcTargetWeightedPredTest() {
        if (mask_buf_ref)
            free(mask_buf_ref);
        if (mask_buf_tst)
            free(mask_buf_tst);
        if (wsrc_buf_ref)
            free(wsrc_buf_ref);
        if (wsrc_buf_tst)
            free(wsrc_buf_tst);
        if (tmp_ref)
            free(tmp_ref);
        if (tmp_tst)
            free(tmp_tst);
    }

  protected:
    void run_test() {
        uint32_t test_num = 10;

        xd.n4_w = 5 + (width_ >> MI_SIZE_LOG2);
        calc_target_weighted_pred_ctxt ctxt_ref = {
            mask_buf_ref, wsrc_buf_ref, tmp_ref, stride, width_};
        calc_target_weighted_pred_ctxt ctxt_tst = {
            mask_buf_tst, wsrc_buf_tst, tmp_tst, stride, width_};
        for (uint32_t i = 0; i < test_num; i++) {
            for (uint32_t j = 0; j < 2 * MAX_SB_SQUARE; j++) {
                mask_buf_ref[j] = mask_buf_tst[j] = rnd_.random();
                wsrc_buf_ref[j] = wsrc_buf_tst[j] = rnd_.random();
                tmp_ref[j] = tmp_tst[j] = rnd_.random() % 255;
            }
            uint8_t size = (width_ >> 1) < 1 ? 1 : (width_ >> 1);
            func_ref_(0, &xd, 0, size, NULL, &ctxt_ref);
            func_tst_(0, &xd, 0, size, NULL, &ctxt_tst);

            if (memcmp(mask_buf_ref,
                       mask_buf_tst,
                       sizeof(int32_t) * 2 * MAX_SB_SQUARE) != 0) {
                for (uint32_t j = 0; j < 2 * MAX_SB_SQUARE; j++)
                    ASSERT_EQ(mask_buf_ref[j], mask_buf_tst[j])
                        << "Mismatch for mask_buf at idx " << j;
            }
            if (memcmp(wsrc_buf_ref,
                       wsrc_buf_tst,
                       sizeof(int32_t) * 2 * MAX_SB_SQUARE) != 0) {
                for (uint32_t j = 0; j < 2 * MAX_SB_SQUARE; j++)
                    ASSERT_EQ(wsrc_buf_ref[j], wsrc_buf_tst[j])
                        << "Mismatch for wsrc_buf at idx " << j;
            }
            if (memcmp(tmp_ref, tmp_tst, sizeof(uint8_t) * 2 * MAX_SB_SQUARE) !=
                0) {
                for (uint32_t j = 0; j < 2 * MAX_SB_SQUARE; j++)
                    ASSERT_EQ(tmp_ref[j], tmp_tst[j])
                        << "Mismatch for tmp at idx " << j;
            }
        }
    }

  protected:
    SVTRandom rnd_;
    int width_;
    CalcTargetWeightedPredFn func_ref_;
    CalcTargetWeightedPredFn func_tst_;
    MacroBlockD xd;
    int32_t *mask_buf_ref;
    int32_t *mask_buf_tst;
    int32_t *wsrc_buf_ref;
    int32_t *wsrc_buf_tst;
    uint8_t *tmp_ref;
    uint8_t *tmp_tst;
    int stride;
};

using CalcTargetWeightedPredTestAbove = CalcTargetWeightedPredTest;

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(CalcTargetWeightedPredTestAbove);

TEST_P(CalcTargetWeightedPredTestAbove, RunCheckOutput) {
    run_test();
};

using CalcTargetWeightedPredTestLeft = CalcTargetWeightedPredTest;

TEST_P(CalcTargetWeightedPredTestLeft, RunCheckOutput) {
    run_test();
};

static const int overlap_tab[] = {2, 4, 8, 16, 32};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, CalcTargetWeightedPredTestAbove,
    ::testing::Combine(
        ::testing::ValuesIn(overlap_tab),
        ::testing::Values(svt_av1_calc_target_weighted_pred_above_c),
        ::testing::Values(svt_av1_calc_target_weighted_pred_above_avx2)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, CalcTargetWeightedPredTestLeft,
    ::testing::Combine(
        ::testing::ValuesIn(overlap_tab),
        ::testing::Values(svt_av1_calc_target_weighted_pred_left_c),
        ::testing::Values(svt_av1_calc_target_weighted_pred_left_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CalcTargetWeightedPredTestLeft,
    ::testing::Combine(
        ::testing::ValuesIn(overlap_tab),
        ::testing::Values(svt_av1_calc_target_weighted_pred_left_c),
        ::testing::Values(svt_av1_calc_target_weighted_pred_left_neon)));
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_OBMC

}  // namespace
