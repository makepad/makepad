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
 * @file PictureOperatorTest.cc
 *
 * @brief Unit test for PictureOperatorTest functions:
 * - picture_copy_kernel_sse2
 *
 * @author Cidana-Ivy
 *
 ******************************************************************************/
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <limits.h>
#include <new>

#include "mcp_sse2.h"
#include "enc_intra_prediction.h"
#include "definitions.h"
#include "random.h"
#include "util.h"
#include "common_dsp_rtcd.h"
#include "aom_dsp_rtcd.h"
#include "motion_estimation.h"
using svt_av1_test_tool::SVTRandom;  // to generate the random

namespace {

typedef std::tuple<uint32_t, uint32_t> PUSize;

typedef void (*downsample_2d_fn)(uint8_t *input_samples, uint32_t input_stride,
                                 uint32_t input_area_width,
                                 uint32_t input_area_height,
                                 uint8_t *decim_samples, uint32_t decim_stride,
                                 uint32_t decim_step);
uint32_t DECIM_STEPS[] = {2, 4, 8};
PUSize DOWNSAMPLE_SIZES[] = {
    PUSize(1920, 1080), PUSize(960, 540), PUSize(176, 144), PUSize(88, 72)};

typedef std::tuple<PUSize, uint32_t, downsample_2d_fn> Downsample2DParam;

class Downsample2DTest
    : public ::testing::Test,
      public ::testing::WithParamInterface<Downsample2DParam> {
  public:
    Downsample2DTest()
        : pu_width(std::get<0>(TEST_GET_PARAM(0))),
          pu_height(std::get<1>(TEST_GET_PARAM(0))),
          decim_step(TEST_GET_PARAM(1)),
          fn_ptr(TEST_GET_PARAM(2)) {
        max_size = sizeof(uint8_t) * (1920 + 3) * (1080 + 3);
        stride = pu_width + 3;
        decim_stride = (pu_width / decim_step) + 3;
        src_ptr = (uint8_t *)malloc(max_size);
        dst_ref_ptr = (uint8_t *)malloc(max_size);
        dst_tst_ptr = (uint8_t *)malloc(max_size);
    }

    void TearDown() override {
        if (src_ptr)
            free(src_ptr);
        if (dst_ref_ptr)
            free(dst_ref_ptr);
        if (dst_tst_ptr)
            free(dst_tst_ptr);
    }

  protected:
    void prepare_data() {
        const int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        for (int i = 0; i < max_size; i++) {
            src_ptr[i] = rnd.random();
        }
        uint8_t val = rnd.random();
        memset(dst_ref_ptr, val, max_size);
        memset(dst_tst_ptr, val, max_size);
    }

    void run_test() {
        prepare_data();
        svt_aom_downsample_2d_c(src_ptr,
                                stride,
                                pu_width,
                                pu_height,
                                dst_ref_ptr,
                                decim_stride,
                                decim_step);

        fn_ptr(src_ptr,
               stride,
               pu_width,
               pu_height,
               dst_tst_ptr,
               decim_stride,
               decim_step);

        EXPECT_EQ(memcmp(dst_ref_ptr, dst_tst_ptr, max_size), 0);
    }

    int max_size;
    uint32_t pu_width, pu_height;
    uint32_t decim_step;
    uint32_t decim_stride;
    uint32_t stride;
    uint8_t *src_ptr;
    uint8_t *dst_ref_ptr, *dst_tst_ptr;
    downsample_2d_fn fn_ptr;
};

TEST_P(Downsample2DTest, test) {
    for (int i = 0; i < 20; i++) {
        run_test();
    }
};

#if defined(ARCH_X86_64)

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, Downsample2DTest,
    ::testing::Combine(::testing::ValuesIn(DOWNSAMPLE_SIZES),
                       ::testing::ValuesIn(DECIM_STEPS),
                       ::testing::Values(svt_aom_downsample_2d_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, Downsample2DTest,
    ::testing::Combine(::testing::ValuesIn(DOWNSAMPLE_SIZES),
                       ::testing::ValuesIn(DECIM_STEPS),
                       ::testing::Values(svt_aom_downsample_2d_avx2)));

#endif

#if defined(ARCH_AARCH64)

INSTANTIATE_TEST_SUITE_P(
    NEON, Downsample2DTest,
    ::testing::Combine(::testing::ValuesIn(DOWNSAMPLE_SIZES),
                       ::testing::ValuesIn(DECIM_STEPS),
                       ::testing::Values(svt_aom_downsample_2d_neon)));

#endif

}  // namespace
