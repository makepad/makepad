/*
 * Copyright(c) 2025, Alliance for Open Media. All rights reserved
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
 * @file FwdTxfm2dApproxTest.c
 *
 * @brief Unit test for verifying N2 and N4 implementation of the forward
 * transforms match the full forward transform implementation.
 *
 * @author Kyle Siefring
 *
 ******************************************************************************/
#include "gtest/gtest.h"

#include <stdint.h>
#include <new>

#include "definitions.h"
#include "transforms.h"

#include "random.h"
#include "TxfmRef.h"
#include "util.h"
#include "TxfmCommon.h"

using svt_av1_test_tool::SVTRandom;
namespace {

using FwdTxfm2dApproxParam = std::tuple<TxSize, TxType, EB_TRANS_COEFF_SHAPE>;
class FwdTxfm2dApproxTest
    : public ::testing::TestWithParam<FwdTxfm2dApproxParam> {
  public:
    FwdTxfm2dApproxTest()
        : txfm_size_(TEST_GET_PARAM(0)),
          txfm_type_(TEST_GET_PARAM(1)),
          shape_(TEST_GET_PARAM(2)) {
    }

    void SetUp() override {
        input_test_ = reinterpret_cast<int16_t *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(int16_t)));
        output_test_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(int32_t)));
        input_ref_ = reinterpret_cast<int16_t *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(double)));
        output_ref_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(double)));
    }

    void TearDown() override {
        svt_aom_free(input_test_);
        svt_aom_free(output_test_);
        svt_aom_free(input_ref_);
        svt_aom_free(output_ref_);
    }

  protected:
    void run_approx_fwd_txfm_test() {
        for (int bd = 8; bd < 12; bd += 2) {
            SVTRandom rnd(bd, true);
            const int count_test_block = 100;
            const int width = tx_size_wide[txfm_size_];
            const int height = tx_size_high[txfm_size_];
            const int block_size = width * height;

            FwdTxfm2dFunc test_function;
            int output_width, output_height;
            switch (shape_) {
            case N2_SHAPE: {
                output_width = width >> 1;
                output_height = height >> 1;
                test_function = fwd_txfm_2d_N2_c_func[txfm_size_];
                break;
            }
            case N4_SHAPE: {
                output_width = width >> 2;
                output_height = height >> 2;
                test_function = fwd_txfm_2d_N4_c_func[txfm_size_];
                break;
            }
            default: FAIL() << "Shape not supported by tests.";
            }

            // TODO: test all bd
            for (int ti = 0; ti < count_test_block; ++ti) {
                // prepare random test data
                for (int ni = 0; ni < block_size; ++ni) {
                    input_ref_[ni] = input_test_[ni] = (int16_t)rnd.random();
                    output_ref_[ni] = 0;
                    output_test_[ni] = 255;
                }

                // calculate full forward transform
                fwd_txfm_2d_c_func[txfm_size_](
                    input_ref_, output_ref_, width, txfm_type_, bd);

                // calculate approximate transform
                test_function(input_test_, output_test_, width, txfm_type_, bd);

                // check that: test == ref in corner of the output dimensions
                // and test == 0 outside of that corner
                for (int y = 0; y < height; y++) {
                    for (int x = 0; x < width; x++) {
                        if (x < output_width && y < output_height) {
                            ASSERT_EQ(output_ref_[y * width + x],
                                      output_test_[y * width + x])
                                << " y: " << y << " x: " << x;
                        } else {
                            ASSERT_EQ(output_test_[y * width + x], 0)
                                << " y: " << y << " x: " << x;
                        }
                    }
                }
            }
        }
    }

  private:
    const TxSize txfm_size_;
    const TxType txfm_type_;
    const EB_TRANS_COEFF_SHAPE shape_;
    int16_t *input_test_, *input_ref_;
    int32_t *output_test_, *output_ref_;
};

static std::vector<FwdTxfm2dApproxParam> gen_approx_txfm_2d_params(
    EB_TRANS_COEFF_SHAPE shape) {
    std::vector<FwdTxfm2dApproxParam> param_vec;
    for (int s = 0; s < TX_SIZES_ALL; ++s) {
        for (int t = 0; t < TX_TYPES; ++t) {
            const TxType txfm_type = static_cast<TxType>(t);
            const TxSize txfm_size = static_cast<TxSize>(s);
            if (is_txfm_allowed(txfm_type, txfm_size)) {
                param_vec.push_back(
                    FwdTxfm2dApproxParam(txfm_size, txfm_type, shape));
            }
        }
    }
    return param_vec;
}

INSTANTIATE_TEST_SUITE_P(
    N2, FwdTxfm2dApproxTest,
    ::testing::ValuesIn(gen_approx_txfm_2d_params(N2_SHAPE)));
INSTANTIATE_TEST_SUITE_P(
    N4, FwdTxfm2dApproxTest,
    ::testing::ValuesIn(gen_approx_txfm_2d_params(N4_SHAPE)));

TEST_P(FwdTxfm2dApproxTest, run_fwd_accuracy_check) {
    run_approx_fwd_txfm_test();
}

}  // namespace
