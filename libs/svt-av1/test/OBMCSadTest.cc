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
 * @file OBMCsad_Test.cc
 *
 * @brief Unit test for obmc sad functions:
 * - obmc_sad_w4_avx2
 * - obmc_sad_w8n_avx2
 *
 * @author Cidana-Ivy
 *
 ******************************************************************************/
#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "random.h"
#include "util.h"
#include "utility.h"

using std::tuple;
using svt_av1_test_tool::SVTRandom;  // to generate the random

namespace {
#if CONFIG_ENABLE_OBMC
static const int MaskMax = 64;

using Obmcsad_Func = uint32_t (*)(const uint8_t* pre, int pre_stride,
                                  const int32_t* wsrc, const int32_t* mask);
using Obmcsad_Param = tuple<Obmcsad_Func, Obmcsad_Func>;

class OBMCsad_Test : public ::testing::TestWithParam<Obmcsad_Param> {
  public:
    OBMCsad_Test()
        : rnd_(8, false),
          rnd_msk_(0, MaskMax * MaskMax + 1),
          func_ref_(TEST_GET_PARAM(0)),
          func_tst_(TEST_GET_PARAM(1)) {
        pre_ = reinterpret_cast<uint8_t*>(svt_aom_memalign(32, MAX_SB_SQUARE));
        wsrc_buf_ = reinterpret_cast<int32_t*>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int32_t)));
        mask_buf_ = reinterpret_cast<int32_t*>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(int32_t)));
    }

    ~OBMCsad_Test() {
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

            uint32_t sad_ref =
                func_ref_(pre_, MAX_SB_SIZE, wsrc_buf_, mask_buf_);
            uint32_t sad_tst =
                func_tst_(pre_, MAX_SB_SIZE, wsrc_buf_, mask_buf_);

            ASSERT_EQ(sad_tst, sad_ref) << "compare SAD error";
        }
    }

  protected:
    SVTRandom rnd_;
    SVTRandom rnd_msk_;
    Obmcsad_Func func_ref_;
    Obmcsad_Func func_tst_;
    uint8_t* pre_;
    int32_t* wsrc_buf_;
    int32_t* mask_buf_;
};

TEST_P(OBMCsad_Test, RunCheckOutput) {
    run_test(1000);
};

#define OBMC_SAD_FUNC(W, H, opt) svt_aom_obmc_sad##W##x##H##_##opt
#define GEN_OBMC_SAD_TEST_PARAM(W, H, opt) \
    Obmcsad_Param(OBMC_SAD_FUNC(W, H, c), OBMC_SAD_FUNC(W, H, opt))
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
    AVX2, OBMCsad_Test,
    ::testing::ValuesIn(GEN_TEST_PARAMS(GEN_OBMC_SAD_TEST_PARAM, avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, OBMCsad_Test,
    ::testing::ValuesIn(GEN_TEST_PARAMS(GEN_OBMC_SAD_TEST_PARAM, neon)));
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_OBMC

}  // namespace
