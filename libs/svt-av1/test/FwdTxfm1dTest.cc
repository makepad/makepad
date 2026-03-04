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
 * @file FwdTxfm1dTest.cc
 *
 * @brief Unit test for forward 1d transform functions:
 * - av1_fdct{4, 8, 16, 32, 64}_new
 * - av1_fadst{4, 8, 16}_new
 * - av1_fidentity{4, 8, 16, 32}_new
 *
 * @author Cidana-Edmond, Cidana-Wenyao
 *
 ******************************************************************************/

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <new>
#include "gtest/gtest.h"

#include "definitions.h"
#include "transforms.h"

#include "random.h"
#include "TxfmRef.h"
#include "util.h"
#include "TxfmCommon.h"

using svt_av1_test_reference::get_txfm1d_types;
using svt_av1_test_reference::reference_txfm_1d;
using svt_av1_test_tool::SVTRandom;
namespace {
/**
 * @brief Unit test for forward 1d tx functions:
 * - av1_fdct{4, 8, 16, 32, 64}_new
 * - av1_fadst{4, 8, 16}_new
 * - av1_fidentity{4, 8, 16, 32}_new
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
 * The input to this kind of function should residual buffer or intermedia
 * data in 2d fwd functions. For 8-bit, it should be in 10bit.
 *
 * Test cases:
 * - C/AV1FwdTxfm1dTest.run_fwd_accuracy_check
 */
using FwdTxfm1dParam = std::tuple<TxfmType, int>;
class AV1FwdTxfm1dTest : public ::testing::TestWithParam<FwdTxfm1dParam> {
  public:
    AV1FwdTxfm1dTest()
        : max_error_(TEST_GET_PARAM(1)), txfm_type_(TEST_GET_PARAM(0)) {
        txfm_size_ = get_txfm1d_size(txfm_type_);
    }

    void SetUp() override {
        input_test_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_TX_SIZE * sizeof(int32_t)));
        output_test_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(32, MAX_TX_SIZE * sizeof(int32_t)));
        input_ref_ = reinterpret_cast<double *>(
            svt_aom_memalign(32, MAX_TX_SIZE * sizeof(double)));
        output_ref_ = reinterpret_cast<double *>(
            svt_aom_memalign(32, MAX_TX_SIZE * sizeof(double)));
    }

    void TearDown() override {
        svt_aom_free(input_test_);
        svt_aom_free(output_test_);
        svt_aom_free(input_ref_);
        svt_aom_free(output_ref_);
    }

    void run_fwd_accuracy_check() {
        const int bd = 10;
        // The input is residual, and it should be signed bd+1 bits
        SVTRandom rnd(bd + 1, true);
        const int cos_bit = 14;
        const int count_test_block = 5000;
        for (int ti = 0; ti < count_test_block; ++ti) {
            // prepare random test data
            for (int ni = 0; ni < txfm_size_; ++ni) {
                input_test_[ni] = rnd.random();
                input_ref_[ni] = static_cast<double>(input_test_[ni]);
                output_test_[ni] = 0;
                output_ref_[ni] = 255;  // setup different output
            }

            // calculate in forward transform functions
            svt_aom_fwd_txfm_type_to_func(txfm_type_)(
                input_test_, output_test_, cos_bit, test_txfm_range);
            // calculate in reference forward transform functions
            reference_txfm_1d(get_txfm1d_types(txfm_type_),
                              input_ref_,
                              output_ref_,
                              txfm_size_);

            // compare for the result is in accuracy
            for (int ni = 0; ni < txfm_size_; ++ni) {
                ASSERT_LE(abs(output_test_[ni] -
                              static_cast<int32_t>(round(output_ref_[ni]))),
                          max_error_)
                    << "tx_size: " << txfm_size_ << "tx_type: " << txfm_type_;
            }
        }
    }

  private:
    const int max_error_;      /**< max error allowed */
    int txfm_size_;            /**< transform size, max transform is DCT64 */
    const TxfmType txfm_type_; /**< tx type, including dct, iadst, idtx */
    int32_t *input_test_;
    int32_t *output_test_;
    double *input_ref_;
    double *output_ref_;
};

TEST_P(AV1FwdTxfm1dTest, run_fwd_accuracy_check) {
    run_fwd_accuracy_check();
}

INSTANTIATE_TEST_SUITE_P(
    TX, AV1FwdTxfm1dTest,
    ::testing::Values(
        FwdTxfm1dParam(TXFM_TYPE_DCT4, 7), FwdTxfm1dParam(TXFM_TYPE_DCT8, 7),
        FwdTxfm1dParam(TXFM_TYPE_DCT16, 7), FwdTxfm1dParam(TXFM_TYPE_DCT32, 7),
        FwdTxfm1dParam(TXFM_TYPE_DCT64, 7), FwdTxfm1dParam(TXFM_TYPE_ADST4, 7),
        FwdTxfm1dParam(TXFM_TYPE_ADST8, 7), FwdTxfm1dParam(TXFM_TYPE_ADST16, 7),
        FwdTxfm1dParam(TXFM_TYPE_IDENTITY4, 7),
        FwdTxfm1dParam(TXFM_TYPE_IDENTITY8, 7),
        FwdTxfm1dParam(TXFM_TYPE_IDENTITY16, 7),
        FwdTxfm1dParam(TXFM_TYPE_IDENTITY32, 7)));
}  // namespace
