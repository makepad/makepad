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
 * @file FwdTxfm2dAsmTest.c
 *
 * @brief Unit test for forward 2d transform functions written in assembly code:
 * - svt_av1_fwd_txfm2d_{4, 8, 16, 32, 64}x{4, 8, 16, 32, 64}_avx2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include "gtest/gtest.h"

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <new>
#include <algorithm>

#include "svt_time.h"
#include "definitions.h"
#include "transforms.h"
#include "random.h"
#include "util.h"
#include "aom_dsp_rtcd.h"
#include "transforms.h"
#include "TxfmCommon.h"
#include "svt_time.h"

using svt_av1_test_tool::SVTRandom;
#define TEST_OFFSET 10

namespace {
using FwdTxfm2dAsmParam =
    std::tuple<int, int, EB_TRANS_COEFF_SHAPE, const FwdTxfm2dFunc *,
               const FwdTxfm2dFunc *>;

#ifdef ARCH_X86_64

static const FwdTxfm2dFunc fwd_txfm_2d_avx2_func[TX_SIZES_ALL] = {
    NULL,
    svt_av1_fwd_txfm2d_8x8_avx2,
    svt_av1_fwd_txfm2d_16x16_avx2,
    svt_av1_fwd_txfm2d_32x32_avx2,
    svt_av1_fwd_txfm2d_64x64_avx2,
    svt_av1_fwd_txfm2d_4x8_avx2,
    svt_av1_fwd_txfm2d_8x4_avx2,
    svt_av1_fwd_txfm2d_8x16_avx2,
    svt_av1_fwd_txfm2d_16x8_avx2,
    svt_av1_fwd_txfm2d_16x32_avx2,
    svt_av1_fwd_txfm2d_32x16_avx2,
    svt_av1_fwd_txfm2d_32x64_avx2,
    svt_av1_fwd_txfm2d_64x32_avx2,
    svt_av1_fwd_txfm2d_4x16_avx2,
    svt_av1_fwd_txfm2d_16x4_avx2,
    svt_av1_fwd_txfm2d_8x32_avx2,
    svt_av1_fwd_txfm2d_32x8_avx2,
    svt_av1_fwd_txfm2d_16x64_avx2,
    svt_av1_fwd_txfm2d_64x16_avx2,
};

static const FwdTxfm2dFunc fwd_txfm_2d_sse4_1_func[TX_SIZES_ALL] = {
    svt_av1_fwd_txfm2d_4x4_sse4_1,   svt_av1_fwd_txfm2d_8x8_sse4_1,
    svt_av1_fwd_txfm2d_16x16_sse4_1, svt_av1_fwd_txfm2d_32x32_sse4_1,
    svt_av1_fwd_txfm2d_64x64_sse4_1, svt_av1_fwd_txfm2d_4x8_sse4_1,
    svt_av1_fwd_txfm2d_8x4_sse4_1,   svt_av1_fwd_txfm2d_8x16_sse4_1,
    svt_av1_fwd_txfm2d_16x8_sse4_1,  svt_av1_fwd_txfm2d_16x32_sse4_1,
    svt_av1_fwd_txfm2d_32x16_sse4_1, svt_av1_fwd_txfm2d_32x64_sse4_1,
    svt_av1_fwd_txfm2d_64x32_sse4_1, svt_av1_fwd_txfm2d_4x16_sse4_1,
    svt_av1_fwd_txfm2d_16x4_sse4_1,  svt_av1_fwd_txfm2d_8x32_sse4_1,
    svt_av1_fwd_txfm2d_32x8_sse4_1,  svt_av1_fwd_txfm2d_16x64_sse4_1,
    svt_av1_fwd_txfm2d_64x16_sse4_1,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N2_avx2_func[TX_SIZES_ALL] = {
    NULL,
    svt_av1_fwd_txfm2d_8x8_N2_avx2,
    svt_av1_fwd_txfm2d_16x16_N2_avx2,
    svt_av1_fwd_txfm2d_32x32_N2_avx2,
    svt_av1_fwd_txfm2d_64x64_N2_avx2,
    svt_av1_fwd_txfm2d_4x8_N2_avx2,
    svt_av1_fwd_txfm2d_8x4_N2_avx2,
    svt_av1_fwd_txfm2d_8x16_N2_avx2,
    svt_av1_fwd_txfm2d_16x8_N2_avx2,
    svt_av1_fwd_txfm2d_16x32_N2_avx2,
    svt_av1_fwd_txfm2d_32x16_N2_avx2,
    svt_av1_fwd_txfm2d_32x64_N2_avx2,
    svt_av1_fwd_txfm2d_64x32_N2_avx2,
    svt_av1_fwd_txfm2d_4x16_N2_avx2,
    svt_av1_fwd_txfm2d_16x4_N2_avx2,
    svt_av1_fwd_txfm2d_8x32_N2_avx2,
    svt_av1_fwd_txfm2d_32x8_N2_avx2,
    svt_av1_fwd_txfm2d_16x64_N2_avx2,
    svt_av1_fwd_txfm2d_64x16_N2_avx2,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N4_avx2_func[TX_SIZES_ALL] = {
    NULL,
    svt_av1_fwd_txfm2d_8x8_N4_avx2,
    svt_av1_fwd_txfm2d_16x16_N4_avx2,
    svt_av1_fwd_txfm2d_32x32_N4_avx2,
    svt_av1_fwd_txfm2d_64x64_N4_avx2,
    svt_av1_fwd_txfm2d_4x8_N4_avx2,
    svt_av1_fwd_txfm2d_8x4_N4_avx2,
    svt_av1_fwd_txfm2d_8x16_N4_avx2,
    svt_av1_fwd_txfm2d_16x8_N4_avx2,
    svt_av1_fwd_txfm2d_16x32_N4_avx2,
    svt_av1_fwd_txfm2d_32x16_N4_avx2,
    svt_av1_fwd_txfm2d_32x64_N4_avx2,
    svt_av1_fwd_txfm2d_64x32_N4_avx2,
    svt_av1_fwd_txfm2d_4x16_N4_avx2,
    svt_av1_fwd_txfm2d_16x4_N4_avx2,
    svt_av1_fwd_txfm2d_8x32_N4_avx2,
    svt_av1_fwd_txfm2d_32x8_N4_avx2,
    svt_av1_fwd_txfm2d_16x64_N4_avx2,
    svt_av1_fwd_txfm2d_64x16_N4_avx2,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N2_sse4_1_func[TX_SIZES_ALL] = {
    svt_av1_fwd_txfm2d_4x4_N2_sse4_1,   svt_av1_fwd_txfm2d_8x8_N2_sse4_1,
    svt_av1_fwd_txfm2d_16x16_N2_sse4_1, svt_av1_fwd_txfm2d_32x32_N2_sse4_1,
    svt_av1_fwd_txfm2d_64x64_N2_sse4_1, svt_av1_fwd_txfm2d_4x8_N2_sse4_1,
    svt_av1_fwd_txfm2d_8x4_N2_sse4_1,   svt_av1_fwd_txfm2d_8x16_N2_sse4_1,
    svt_av1_fwd_txfm2d_16x8_N2_sse4_1,  svt_av1_fwd_txfm2d_16x32_N2_sse4_1,
    svt_av1_fwd_txfm2d_32x16_N2_sse4_1, svt_av1_fwd_txfm2d_32x64_N2_sse4_1,
    svt_av1_fwd_txfm2d_64x32_N2_sse4_1, svt_av1_fwd_txfm2d_4x16_N2_sse4_1,
    svt_av1_fwd_txfm2d_16x4_N2_sse4_1,  svt_av1_fwd_txfm2d_8x32_N2_sse4_1,
    svt_av1_fwd_txfm2d_32x8_N2_sse4_1,  svt_av1_fwd_txfm2d_16x64_N2_sse4_1,
    svt_av1_fwd_txfm2d_64x16_N2_sse4_1,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N4_sse4_1_func[TX_SIZES_ALL] = {
    svt_av1_fwd_txfm2d_4x4_N4_sse4_1,   svt_av1_fwd_txfm2d_8x8_N4_sse4_1,
    svt_av1_fwd_txfm2d_16x16_N4_sse4_1, svt_av1_fwd_txfm2d_32x32_N4_sse4_1,
    svt_av1_fwd_txfm2d_64x64_N4_sse4_1, svt_av1_fwd_txfm2d_4x8_N4_sse4_1,
    svt_av1_fwd_txfm2d_8x4_N4_sse4_1,   svt_av1_fwd_txfm2d_8x16_N4_sse4_1,
    svt_av1_fwd_txfm2d_16x8_N4_sse4_1,  svt_av1_fwd_txfm2d_16x32_N4_sse4_1,
    svt_av1_fwd_txfm2d_32x16_N4_sse4_1, svt_av1_fwd_txfm2d_32x64_N4_sse4_1,
    svt_av1_fwd_txfm2d_64x32_N4_sse4_1, svt_av1_fwd_txfm2d_4x16_N4_sse4_1,
    svt_av1_fwd_txfm2d_16x4_N4_sse4_1,  svt_av1_fwd_txfm2d_8x32_N4_sse4_1,
    svt_av1_fwd_txfm2d_32x8_N4_sse4_1,  svt_av1_fwd_txfm2d_16x64_N4_sse4_1,
    svt_av1_fwd_txfm2d_64x16_N4_sse4_1,
};

#if EN_AVX512_SUPPORT
static const FwdTxfm2dFunc fwd_txfm_2d_avx512_func[TX_SIZES_ALL] = {
    NULL,
    NULL,
    svt_av1_fwd_txfm2d_16x16_avx512,
    svt_av1_fwd_txfm2d_32x32_avx512,
    svt_av1_fwd_txfm2d_64x64_avx512,
    NULL,
    NULL,
    NULL,
    NULL,
    svt_av1_fwd_txfm2d_16x32_avx512,
    svt_av1_fwd_txfm2d_32x16_avx512,
    svt_av1_fwd_txfm2d_32x64_avx512,
    svt_av1_fwd_txfm2d_64x32_avx512,
    NULL,
    NULL,
    NULL,
    NULL,
    svt_av1_fwd_txfm2d_16x64_avx512,
    svt_av1_fwd_txfm2d_64x16_avx512,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N2_avx512_func[TX_SIZES_ALL] = {
    NULL,
    NULL,
    NULL,
    av1_fwd_txfm2d_32x32_N2_avx512,
    av1_fwd_txfm2d_64x64_N2_avx512,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    av1_fwd_txfm2d_32x64_N2_avx512,
    av1_fwd_txfm2d_64x32_N2_avx512,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N4_avx512_func[TX_SIZES_ALL] = {
    NULL, NULL, NULL, NULL, av1_fwd_txfm2d_64x64_N4_avx512,
    NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL,
};
#endif /*EN_AVX512_SUPPORT*/

#endif

#ifdef ARCH_AARCH64

static const FwdTxfm2dFunc fwd_txfm_2d_neon_func[TX_SIZES_ALL] = {
    svt_av1_fwd_txfm2d_4x4_neon,   svt_av1_fwd_txfm2d_8x8_neon,
    svt_av1_fwd_txfm2d_16x16_neon, svt_av1_fwd_txfm2d_32x32_neon,
    svt_av1_fwd_txfm2d_64x64_neon, svt_av1_fwd_txfm2d_4x8_neon,
    svt_av1_fwd_txfm2d_8x4_neon,   svt_av1_fwd_txfm2d_8x16_neon,
    svt_av1_fwd_txfm2d_16x8_neon,  svt_av1_fwd_txfm2d_16x32_neon,
    svt_av1_fwd_txfm2d_32x16_neon, svt_av1_fwd_txfm2d_32x64_neon,
    svt_av1_fwd_txfm2d_64x32_neon, svt_av1_fwd_txfm2d_4x16_neon,
    svt_av1_fwd_txfm2d_16x4_neon,  svt_av1_fwd_txfm2d_8x32_neon,
    svt_av1_fwd_txfm2d_32x8_neon,  svt_av1_fwd_txfm2d_16x64_neon,
    svt_av1_fwd_txfm2d_64x16_neon,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N4_neon_func[TX_SIZES_ALL] = {
    svt_av1_fwd_txfm2d_4x4_N4_neon,   svt_av1_fwd_txfm2d_8x8_N4_neon,
    svt_av1_fwd_txfm2d_16x16_N4_neon, svt_av1_fwd_txfm2d_32x32_N4_neon,
    svt_av1_fwd_txfm2d_64x64_N4_neon, svt_av1_fwd_txfm2d_4x8_N4_neon,
    svt_av1_fwd_txfm2d_8x4_N4_neon,   svt_av1_fwd_txfm2d_8x16_N4_neon,
    svt_av1_fwd_txfm2d_16x8_N4_neon,  svt_av1_fwd_txfm2d_16x32_N4_neon,
    svt_av1_fwd_txfm2d_32x16_N4_neon, svt_av1_fwd_txfm2d_32x64_N4_neon,
    svt_av1_fwd_txfm2d_64x32_N4_neon, svt_av1_fwd_txfm2d_4x16_N4_neon,
    svt_av1_fwd_txfm2d_16x4_N4_neon,  svt_av1_fwd_txfm2d_8x32_N4_neon,
    svt_av1_fwd_txfm2d_32x8_N4_neon,  svt_av1_fwd_txfm2d_16x64_N4_neon,
    svt_av1_fwd_txfm2d_64x16_N4_neon,
};

static const FwdTxfm2dFunc fwd_txfm_2d_N2_neon_func[TX_SIZES_ALL] = {
    svt_av1_fwd_txfm2d_4x4_N2_neon,   svt_av1_fwd_txfm2d_8x8_N2_neon,
    svt_av1_fwd_txfm2d_16x16_N2_neon, svt_av1_fwd_txfm2d_32x32_N2_neon,
    svt_av1_fwd_txfm2d_64x64_N2_neon, svt_av1_fwd_txfm2d_4x8_N2_neon,
    svt_av1_fwd_txfm2d_8x4_N2_neon,   svt_av1_fwd_txfm2d_8x16_N2_neon,
    svt_av1_fwd_txfm2d_16x8_N2_neon,  svt_av1_fwd_txfm2d_16x32_N2_neon,
    svt_av1_fwd_txfm2d_32x16_N2_neon, svt_av1_fwd_txfm2d_32x64_N2_neon,
    svt_av1_fwd_txfm2d_64x32_N2_neon, svt_av1_fwd_txfm2d_4x16_N2_neon,
    svt_av1_fwd_txfm2d_16x4_N2_neon,  svt_av1_fwd_txfm2d_8x32_N2_neon,
    svt_av1_fwd_txfm2d_32x8_N2_neon,  svt_av1_fwd_txfm2d_16x64_N2_neon,
    svt_av1_fwd_txfm2d_64x16_N2_neon,
};

#endif /* ARCH_AARCH64*/

/**
 * @brief Unit test for fwd tx 2d avx2 functions:
 * - svt_av1_fwd_txfm2d_{4, 8, 16, 32, 64}x{4, 8, 16, 32, 64}_avx2
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check test output and reference output.
 * The test output and reference output are different at the beginning.
 *
 * Expect result:
 * Output from assemble function should be exactly same as output from c.
 *
 * Test coverage:
 * Test cases:
 * Input buffer: Fill with random values
 * TxSize: all the valid TxSize and TxType allowed.
 * BitDepth: 8bit and 10bit.
 *
 */
class FwdTxfm2dAsmTest : public ::testing::TestWithParam<FwdTxfm2dAsmParam> {
  public:
    FwdTxfm2dAsmTest()
        : tx_size_(static_cast<TxSize>(TEST_GET_PARAM(0))),
          bd_(TEST_GET_PARAM(1)),
          shape_(TEST_GET_PARAM(2)),
          ref_func_tbl_(TEST_GET_PARAM(3)),
          test_func_tbl_(TEST_GET_PARAM(4)) {
        // input are signed value with bitdepth + 1 bits
        rnd_ = new SVTRandom(-(1 << bd_) + 1, (1 << bd_) - 1);

        width_ = tx_size_wide[tx_size_];
        height_ = tx_size_high[tx_size_];
        // set default value to 0
        memset(output_test_buf_, 0, sizeof(output_test_buf_));
        // set default value to -1
        memset(output_ref_buf_, 255, sizeof(output_ref_buf_));
        input_ = ALIGNED_ADDR(int16_t, ALIGNMENT, input_buf_);
        output_test_ = ALIGNED_ADDR(int32_t, ALIGNMENT, output_test_buf_);
        output_ref_ = ALIGNED_ADDR(int32_t, ALIGNMENT, output_ref_buf_);
        int over_buffer = sizeof(output_test_buf_) - width_ * height_;
        memset(output_test_buf_ + width_ * height_, 0xcd, over_buffer);
        memset(output_ref_buf_ + width_ * height_, 0xcd, over_buffer);
    }

    ~FwdTxfm2dAsmTest() {
        delete rnd_;
    }

    void run_match_test() {
        FwdTxfm2dFunc test_func = test_func_tbl_[tx_size_];
        FwdTxfm2dFunc ref_func = ref_func_tbl_[tx_size_];
        execute_test(test_func, ref_func, shape_);
    }

    void speed_test() {
        FwdTxfm2dFunc test_func = test_func_tbl_[tx_size_];
        FwdTxfm2dFunc ref_func = ref_func_tbl_[tx_size_];
        run_speed_test(test_func, ref_func);
    }

  private:
    void execute_test(FwdTxfm2dFunc test_func, FwdTxfm2dFunc ref_func,
                      EB_TRANS_COEFF_SHAPE shape) {
        if (ref_func == nullptr || test_func == nullptr)
            return;

        ASSERT_NE(rnd_, nullptr) << "Failed to create random generator";
        for (int tx_type = 0; tx_type < TX_TYPES; ++tx_type) {
            TxType type = static_cast<TxType>(tx_type);
            // tx_type and tx_size are not compatible in the av1-spec.
            // like the max size of adst transform is 16, and max size of
            // identity transform is 32.
            if (is_txfm_allowed(type, tx_size_) == false)
                continue;

            const int loops = 100;
            for (int k = 0; k < loops; k++) {
                populate_with_random();

                ref_func(input_, output_ref_, stride_, type, (uint8_t)bd_);
                if (shape == N2_SHAPE) {
                    for (int i = 0;
                         i < (tx_size_wide[tx_size_] * tx_size_high[tx_size_]);
                         i++) {
                        if (i % tx_size_wide[tx_size_] >=
                                (tx_size_wide[tx_size_] >> 1) ||
                            i / tx_size_wide[tx_size_] >=
                                (tx_size_high[tx_size_] >> 1)) {
                            output_ref_[i] = 0;
                        }
                    }
                } else if (shape == N4_SHAPE) {
                    for (int i = 0;
                         i < (tx_size_wide[tx_size_] * tx_size_high[tx_size_]);
                         i++) {
                        if (i % tx_size_wide[tx_size_] >=
                                (tx_size_wide[tx_size_] >> 2) ||
                            i / tx_size_wide[tx_size_] >=
                                (tx_size_high[tx_size_] >> 2)) {
                            output_ref_[i] = 0;
                        }
                    }
                }
                test_func(input_, output_test_, stride_, type, (uint8_t)bd_);

                if (0 !=
                    memcmp(output_test_,
                           output_ref_,
                           MAX_TX_SQUARE * sizeof(int32_t) + TEST_OFFSET)) {
                    for (int i = 0; i < height_; i++)
                        for (int j = 0; j < width_; j++) {
                            if (output_ref_[i * width_ + j] !=
                                output_test_[i * width_ + j]) {
                                printf("error in important part\n");
                            }

                            ASSERT_EQ(output_ref_[i * width_ + j],
                                      output_test_[i * width_ + j])
                                << "loop: " << k << " tx_type: " << tx_type
                                << " tx_size: " << tx_size_ << " Mismatch at ("
                                << j << " x " << i << ")";
                        }

                    ASSERT_EQ(1, 0);
                }
            }
        }
    }

    void run_speed_test(FwdTxfm2dFunc test_func, FwdTxfm2dFunc ref_func) {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;
        const char *tx_type_name[] = {"DCT_DCT",
                                      "ADST_DCT",
                                      "DCT_ADST",
                                      "ADST_ADST",
                                      "FLIPADST_DCT",
                                      "DCT_FLIPADST",
                                      "FLIPADST_FLIPADST",
                                      "ADST_FLIPADST",
                                      "FLIPADST_ADST",
                                      "IDTX",
                                      "V_DCT",
                                      "H_DCT",
                                      "V_ADST",
                                      "H_ADST",
                                      "V_FLIPADST",
                                      "H_FLIPADST",
                                      "TX_TYPES"};

        if (ref_func == nullptr || test_func == nullptr)
            return;

        ASSERT_NE(rnd_, nullptr) << "Failed to create random generator";
        for (int tx_type = 0; tx_type < TX_TYPES; ++tx_type) {
            TxType type = static_cast<TxType>(tx_type);
            populate_with_random();
            // tx_type and tx_size are not compatible in the av1-spec.
            // like the max size of adst transform is 16, and max size of
            // identity transform is 32.
            if (is_txfm_allowed(type, tx_size_) == false)
                continue;

            const int loops = 500000;
            svt_av1_get_time(&start_time_seconds, &start_time_useconds);
            for (int k = 0; k < loops; k++) {
                ref_func(input_, output_ref_, stride_, type, (uint8_t)bd_);
            }
            svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);
            for (int k = 0; k < loops; k++) {
                test_func(input_, output_test_, stride_, type, (uint8_t)bd_);
            }
            svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

            time_c =
                svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                        start_time_useconds,
                                                        middle_time_seconds,
                                                        middle_time_useconds);

            time_o =
                svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                        middle_time_useconds,
                                                        finish_time_seconds,
                                                        finish_time_useconds);

            printf(
                "C vs Opt; Transform: ;%02ix%02i; %17s; Speed compare: "
                ";%5.2fx\n",
                tx_size_wide[tx_size_],
                tx_size_high[tx_size_],
                tx_type_name[tx_type],
                time_c / time_o);
        }
    }
    void populate_with_random() {
        for (int i = 0; i < height_; i++) {
            for (int j = 0; j < width_; j++) {
                input_[i * stride_ + j] = (int16_t)rnd_->random();
            }
        }

        return;
    }

  private:
    const TxSize tx_size_; /**< input param tx_size */
    const int bd_;         /**< input param 8bit or 10bit */
    EB_TRANS_COEFF_SHAPE shape_;
    const FwdTxfm2dFunc *ref_func_tbl_;
    const FwdTxfm2dFunc *test_func_tbl_;
    int width_;
    int height_;
    SVTRandom *rnd_;
    static const int stride_ = MAX_TX_SIZE;
    uint8_t input_buf_[MAX_TX_SQUARE * sizeof(int16_t) + ALIGNMENT - 1];
    uint8_t output_test_buf_[MAX_TX_SQUARE * sizeof(int32_t) + ALIGNMENT - 1 +
                             TEST_OFFSET];
    uint8_t output_ref_buf_[MAX_TX_SQUARE * sizeof(int32_t) + ALIGNMENT - 1 +
                            TEST_OFFSET];
    int16_t *input_;       /**< aligned address for input */
    int32_t *output_test_; /**< aligned address for output test */
    int32_t *output_ref_;  /**< aligned address for output ref */
};

TEST_P(FwdTxfm2dAsmTest, match_test) {
    run_match_test();
}

TEST_P(FwdTxfm2dAsmTest, DISABLED_speed_test) {
    speed_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(DEFAULT_SHAPE),
                       ::testing::Values(fwd_txfm_2d_c_func),
                       ::testing::Values(fwd_txfm_2d_sse4_1_func)));

INSTANTIATE_TEST_SUITE_P(
    N2_SSE4_1, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N2_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N2_c_func),
                       ::testing::Values(fwd_txfm_2d_N2_sse4_1_func)));

INSTANTIATE_TEST_SUITE_P(
    N4_SSE4_1, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N4_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N4_c_func),
                       ::testing::Values(fwd_txfm_2d_N4_sse4_1_func)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(DEFAULT_SHAPE),
                       ::testing::Values(fwd_txfm_2d_c_func),
                       ::testing::Values(fwd_txfm_2d_avx2_func)));

INSTANTIATE_TEST_SUITE_P(
    N2_AVX2, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N2_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N2_c_func),
                       ::testing::Values(fwd_txfm_2d_N2_avx2_func)));

INSTANTIATE_TEST_SUITE_P(
    N4_AVX2, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N4_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N4_c_func),
                       ::testing::Values(fwd_txfm_2d_N4_avx2_func)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(DEFAULT_SHAPE),
                       ::testing::Values(fwd_txfm_2d_c_func),
                       ::testing::Values(fwd_txfm_2d_avx512_func)));

INSTANTIATE_TEST_SUITE_P(
    N2_AVX512, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N2_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N2_c_func),
                       ::testing::Values(fwd_txfm_2d_N2_avx512_func)));

INSTANTIATE_TEST_SUITE_P(
    N4_AVX512, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N4_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N4_c_func),
                       ::testing::Values(fwd_txfm_2d_N4_avx512_func)));

#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(DEFAULT_SHAPE),
                       ::testing::Values(fwd_txfm_2d_c_func),
                       ::testing::Values(fwd_txfm_2d_neon_func)));

INSTANTIATE_TEST_SUITE_P(
    N4_NEON, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N4_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N4_c_func),
                       ::testing::Values(fwd_txfm_2d_N4_neon_func)));

INSTANTIATE_TEST_SUITE_P(
    N2_NEON, FwdTxfm2dAsmTest,
    ::testing::Combine(::testing::Range(static_cast<int>(TX_4X4),
                                        static_cast<int>(TX_SIZES_ALL), 1),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT)),
                       ::testing::Values(N2_SHAPE),
                       ::testing::Values(fwd_txfm_2d_N2_c_func),
                       ::testing::Values(fwd_txfm_2d_N2_neon_func)));
#endif
}  // namespace
