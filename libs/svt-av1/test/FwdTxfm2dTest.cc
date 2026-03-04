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
 * @file FwdTxfm2dTest.c
 *
 * @brief Unit test for forward 2d transform functions:
 * - Av1TransformTwoD_{4x4, 8x8, 16x16, 32x32, 64x64}
 * - av1_fwd_txfm2d_{rectangle}
 *
 * @author Cidana-Edmond, Cidana-Wenyao
 *
 ******************************************************************************/
#include "gtest/gtest.h"

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <new>

#include "definitions.h"
#include "transforms.h"

#include "random.h"
#include "TxfmRef.h"
#include "util.h"
#include "TxfmCommon.h"

using svt_av1_test_reference::get_scale_factor;
using svt_av1_test_reference::reference_txfm_2d;
using svt_av1_test_tool::SVTRandom;
namespace {

using FwdTxfm2dParam = std::tuple<TxSize, TxType, double, int>;
/**
 * @brief Unit test for forward 2d tx functions:
 * - Av1TransformTwoD_{4x4, 8x8, 16x16, 32x32, 64x64}
 * - av1_fwd_txfm2d_{rectangle}
 *
 * Test strategy:
 * Verify these tx function by comparing with reference implementation.
 * Feed the same data and check the difference between test output
 * and reference output.
 *
 * Expected result:
 * The difference should be smaller than the max_error, which is specified
 * by algorithm analysis.
 *
 * Test coverage:
 * The input to this kind of function should be residual.
 *
 * Test cases:
 * - C/AV1FwdTxfm2dTest.run_fwd_accuracy_check
 */
class AV1FwdTxfm2dTest : public ::testing::TestWithParam<FwdTxfm2dParam> {
  public:
    AV1FwdTxfm2dTest()
        : max_error_(TEST_GET_PARAM(2)),
          txfm_size_(TEST_GET_PARAM(0)),
          txfm_type_(TEST_GET_PARAM(1)),
          bd_(TEST_GET_PARAM(3)) {
        svt_aom_transform_config(txfm_type_, txfm_size_, &cfg_);
    }

    void SetUp() override {
        input_test_ = reinterpret_cast<int16_t *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(int16_t)));
        output_test_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(int32_t)));
        input_ref_ = reinterpret_cast<double *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(double)));
        output_ref_ = reinterpret_cast<double *>(
            svt_aom_memalign(32, MAX_TX_SQUARE * sizeof(double)));
    }

    void TearDown() override {
        svt_aom_free(input_test_);
        svt_aom_free(output_test_);
        svt_aom_free(input_ref_);
        svt_aom_free(output_ref_);
    }

  protected:
    void run_fwd_accuracy_check() {
        SVTRandom rnd(bd_, false);
        const int count_test_block = 1000;
        const int width = tx_size_wide[cfg_.tx_size];
        const int height = tx_size_high[cfg_.tx_size];
        const int block_size = width * height;
        scale_factor_ = get_scale_factor(cfg_, width, height);

        for (int ti = 0; ti < count_test_block; ++ti) {
            // prepare random test data
            for (int ni = 0; ni < block_size; ++ni) {
                input_test_[ni] = (int16_t)rnd.random();
                input_ref_[ni] = static_cast<double>(input_test_[ni]);
                output_ref_[ni] = 0;
                output_test_[ni] = 255;
            }

            // calculate in forward transform functions
            fwd_txfm_2d_c_func[txfm_size_](input_test_,
                                           output_test_,
                                           tx_size_wide[cfg_.tx_size],
                                           txfm_type_,
                                           bd_);

            // calculate in reference forward transform functions
            fwd_txfm_2d_reference(input_ref_, output_ref_);

            // repack the coefficents for some tx_size
            repack_coeffs(width, height);

            // compare for the result is in accuracy
            double test_max_error = 0;
            for (int ni = 0; ni < block_size; ++ni) {
                test_max_error =
                    AOMMAX(test_max_error,
                           fabs(output_test_[ni] - round(output_ref_[ni])));
            }
            test_max_error /= scale_factor_;
            ASSERT_GE(max_error_, test_max_error)
                << "fwd txfm 2d test tx_type: " << txfm_type_
                << " tx_size: " << txfm_size_ << " bd: " << bd_
                << " loop: " << ti;
        }
    }

  private:
    void flip_input(double *input) {
        int width = tx_size_wide[cfg_.tx_size];
        int height = tx_size_high[cfg_.tx_size];
        if (cfg_.lr_flip) {
            for (int r = 0; r < height; ++r) {
                for (int c = 0; c < width / 2; ++c) {
                    const double tmp = input[r * width + c];
                    input[r * width + c] = input[r * width + width - 1 - c];
                    input[r * width + width - 1 - c] = tmp;
                }
            }
        }
        if (cfg_.ud_flip) {
            for (int c = 0; c < width; ++c) {
                for (int r = 0; r < height / 2; ++r) {
                    const double tmp = input[r * width + c];
                    input[r * width + c] = input[(height - 1 - r) * width + c];
                    input[(height - 1 - r) * width + c] = tmp;
                }
            }
        }
    }

    void fwd_txfm_2d_reference(double *input, double *output) {
        flip_input(input);
        reference_txfm_2d(input, output, txfm_type_, txfm_size_, scale_factor_);
    }

    // The max txb_width or txb_height is 32, as specified in spec 7.12.3.
    // Clear the high frequency coefficents and repack it in linear layout.
    void repack_coeffs(int tx_width, int tx_height) {
        for (int i = 0; i < 2; ++i) {
            const int e_size =
                i == 0 ? sizeof(output_test_[0]) : sizeof(output_ref_[0]);
            uint8_t *out = i == 0 ? reinterpret_cast<uint8_t *>(output_test_)
                                  : reinterpret_cast<uint8_t *>(output_ref_);

            if (tx_width == 64 && tx_height == 64) {  // tx_size == TX_64X64
                // zero out top-right 32x32 area.
                for (int row = 0; row < 32; ++row) {
                    memset(out + (row * 64 + 32) * e_size, 0, 32 * e_size);
                }
                // zero out the bottom 64x32 area.
                memset(out + 32 * 64 * e_size, 0, 32 * 64 * e_size);
                // Re-pack non-zero coeffs in the first 32x32 indices.
                for (int row = 1; row < 32; ++row) {
                    memcpy(out + row * 32 * e_size,
                           out + row * 64 * e_size,
                           32 * e_size);
                }
            } else if (tx_width == 32 &&
                       tx_height == 64) {  // tx_size == TX_32X64
                // zero out the bottom 32x32 area.
                memset(out + 32 * 32 * e_size, 0, 32 * 32 * e_size);
                // Note: no repacking needed here.
            } else if (tx_width == 64 &&
                       tx_height == 32) {  // tx_size == TX_64X32
                // zero out right 32x32 area.
                for (int row = 0; row < 32; ++row) {
                    memset(out + (row * 64 + 32) * e_size, 0, 32 * e_size);
                }
                // Re-pack non-zero coeffs in the first 32x32 indices.
                for (int row = 1; row < 32; ++row) {
                    memcpy(out + row * 32 * e_size,
                           out + row * 64 * e_size,
                           32 * e_size);
                }
            } else if (tx_width == 16 &&
                       tx_height == 64) {  // tx_size == TX_16X64
                // zero out the bottom 16x32 area.
                memset(out + 16 * 32 * e_size, 0, 16 * 32 * e_size);
                // Note: no repacking needed here.
            } else if (tx_width == 64 &&
                       tx_height == 16) {  // tx_size == TX_64X16
                // zero out right 32x16 area.
                for (int row = 0; row < 16; ++row) {
                    memset(out + (row * 64 + 32) * e_size, 0, 32 * e_size);
                }
                // Re-pack non-zero coeffs in the first 32x16 indices.
                for (int row = 1; row < 16; ++row) {
                    memcpy(out + row * 32 * e_size,
                           out + row * 64 * e_size,
                           32 * e_size);
                }
            }
        }
    }

  private:
    const double max_error_;
    const TxSize txfm_size_;
    const TxType txfm_type_;
    const int bd_;
    double scale_factor_;
    Txfm2dFlipCfg cfg_;
    int16_t *input_test_;
    int32_t *output_test_;
    double *input_ref_;
    double *output_ref_;
};

TEST_P(AV1FwdTxfm2dTest, run_fwd_accuracy_check) {
    run_fwd_accuracy_check();
}

static double max_error_ls[TX_SIZES_ALL] = {
    3,    // 4x4 transform
    5,    // 8x8 transform
    11,   // 16x16 transform
    70,   // 32x32 transform
    64,   // 64x64 transform
    3.9,  // 4x8 transform
    4.3,  // 8x4 transform
    12,   // 8x16 transform
    12,   // 16x8 transform
    32,   // 16x32 transform
    46,   // 32x16 transform
    136,  // 32x64 transform
    136,  // 64x32 transform
    5,    // 4x16 transform
    6,    // 16x4 transform
    21,   // 8x32 transform
    13,   // 32x8 transform
    30,   // 16x64 transform
    41,   // 64x16 transform
};

static std::vector<FwdTxfm2dParam> gen_txfm_2d_params() {
    std::vector<FwdTxfm2dParam> param_vec;
    for (int s = 0; s < TX_SIZES_ALL; ++s) {
        double max_error = max_error_ls[s];
        for (int t = 0; t < TX_TYPES; ++t) {
            const TxType txfm_type = static_cast<TxType>(t);
            const TxSize txfm_size = static_cast<TxSize>(s);
            if (is_txfm_allowed(txfm_type, txfm_size)) {
                param_vec.push_back(
                    FwdTxfm2dParam(txfm_size, txfm_type, max_error, 10));
            }
        }
    }
    return param_vec;
}

INSTANTIATE_TEST_SUITE_P(TX, AV1FwdTxfm2dTest,
                         ::testing::ValuesIn(gen_txfm_2d_params()));

}  // namespace
