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
 * @file FFTTest.cc
 *
 * @brief Unit test for FFT and iFFT 2d functions:
 * - svt_aom_fft{2x2, 4x4, 8x8, 16x16, 32x32}_float_{c, sse2, avx2}
 * - svt_aom_ifft{2x2, 4x4, 8x8, 16x16, 32x32}_float_{c, sse2, avx2}
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <sstream>

#include "gtest/gtest.h"
#include "random.h"
#include "util.h"
#include "utility.h"
#include "aom_dsp_rtcd.h"

using svt_av1_test_tool::SVTRandom;
namespace {
/**
 * @brief Unit test for FFT and iFFT 2d functions:
 * - svt_aom_fft{2x2, 4x4, 8x8, 16x16, 32x32}_float_{c, sse2, avx2}
 * - svt_aom_ifft{2x2, 4x4, 8x8, 16x16, 32x32}_float_{c, sse2, avx2}
 *
 * Test strategy:
 * 1) Verify these FFT functions by comparing with reference implementation.
 * Feed the same data and check the difference between test output
 * and reference output.
 * 2) Verify these FFT functions by comparing with the results from iFFT
 * functions.
 *
 * Expected result:
 * The difference should be smaller than the max_error.
 *
 * Test cases:
 * - FFT/FFT2DTest.run_fft_accuracy_check
 * - FFT/FFT2DTest.run_fft_ifft_check
 */

static float max_error = 0.00001f;

static const std::string print_data(const float *data, const int width,
                                    const int height) {
    std::string print_str;
    std::stringstream ss(print_str);
    ss << "test data dump:\n";
    for (int j = 0; j < height; j++) {
        for (int i = 0; i < width; i++)
            ss << data[j * width + i] << ",\t";
        ss << "\n";
    }
    return ss.str();
}

typedef void (*FFTFloatFcn)(const float *, float *, float *);
typedef void (*IFFTFloatFcn)(const float *, float *, float *);

using FFT2DTestParam = std::tuple<FFTFloatFcn, FFTFloatFcn, IFFTFloatFcn, int>;
using IFFT2DTestParam = std::tuple<IFFTFloatFcn, IFFTFloatFcn, int>;

class FFT2DTest : public ::testing::TestWithParam<FFT2DTestParam> {
  public:
    FFT2DTest()
        : tst_fcn_(TEST_GET_PARAM(0)),
          ref_fcn_(TEST_GET_PARAM(1)),
          verify_fcn_(TEST_GET_PARAM(2)),
          txfm_size_(TEST_GET_PARAM(3)) {
    }

    void SetUp() override {
        input_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(input_, 0, txfm_size_ * txfm_size_ * sizeof(float));
        temp_tst_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(temp_tst_, 0, txfm_size_ * txfm_size_ * sizeof(float));
        temp_ref_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(temp_ref_, 0, txfm_size_ * txfm_size_ * sizeof(float));
        output_tst_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, 2 * txfm_size_ * txfm_size_ * sizeof(float)));
        memset(output_tst_, 0, 2 * txfm_size_ * txfm_size_ * sizeof(float));
        output_ref_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, 2 * txfm_size_ * txfm_size_ * sizeof(float)));
        memset(output_ref_, 0, 2 * txfm_size_ * txfm_size_ * sizeof(float));
    }

    void TearDown() override {
        svt_aom_free(input_);
        svt_aom_free(temp_tst_);
        svt_aom_free(temp_ref_);
        svt_aom_free(output_tst_);
        svt_aom_free(output_ref_);
    }

    void run_fft_accuracy_check() {
        SVTRandom rnd(0.0f, 1.0f);
        const int test_times = 5000;
        for (int i = 0; i < test_times; ++i) {
            // prepare random test data
            int data_size = txfm_size_ * txfm_size_;
            for (int j = 0; j < data_size; ++j)
                input_[j] = rnd.random_float();

            // calculate in reference FFT function
            ref_fcn_(input_, temp_ref_, output_ref_);
            // calculate in test FFT function
            tst_fcn_(input_, temp_tst_, output_tst_);

            // compare for the results
            for (int j = 0; j < data_size * 2; ++j) {
                ASSERT_FLOAT_EQ(output_tst_[j], output_ref_[j])
                    << "txfm_size_: " << txfm_size_ << "[" << j << "]"
                    << " failed at test #" << i << "\n"
                    << print_data(input_, txfm_size_, txfm_size_);
            }
        }
    }

    void run_fft_ifft_check() {
        SVTRandom rnd(0.0f, 1.0f);
        const int test_times = 5000;
        for (int i = 0; i < test_times; ++i) {
            // prepare random test data
            int data_size = txfm_size_ * txfm_size_;
            for (int j = 0; j < data_size; ++j)
                input_[j] = rnd.random_float();

            // calculate in test FFT function
            tst_fcn_(input_, temp_tst_, output_tst_);
            // calculate result in inverse FFT function
            verify_fcn_(output_tst_, temp_ref_, output_ref_);

            // compare for the results
            for (int j = 0; j < data_size; ++j) {
                float verify = output_ref_[j] / data_size;
                ASSERT_LE(fabs(input_[j] - verify), max_error)
                    << "txfm_size_: " << txfm_size_ << " failed at test #" << i
                    << "\n"
                    << print_data(input_, txfm_size_, txfm_size_);
            }
        }
    }

  protected:
    FFTFloatFcn tst_fcn_;     /**< pointer of FFT test function */
    FFTFloatFcn ref_fcn_;     /**< pointer of FFT reference function */
    IFFTFloatFcn verify_fcn_; /**< pointer of iFFT verify function */
    int txfm_size_;           /**< transform size, max transform is DCT64 */
    float *input_;            /**< FFT input data buffer */
    float *temp_tst_;         /**< temp buffer for FFT/iFFT test function */
    float *temp_ref_;         /**< temp buffer for FFT/iFFT test function */
    float *output_tst_;       /**< output buffer for FFT test function */
    float *output_ref_; /**< output buufer for FFT/iFFT reference function*/
};

TEST_P(FFT2DTest, run_fft_accuracy_check) {
    run_fft_accuracy_check();
}

TEST_P(FFT2DTest, run_fft_ifft_check) {
    run_fft_ifft_check();
}

INSTANTIATE_TEST_SUITE_P(
    FFT, FFT2DTest,
    ::testing::Values(
        FFT2DTestParam(svt_aom_fft2x2_float_c, svt_aom_fft2x2_float_c,
                       svt_aom_ifft2x2_float_c, 2),
        FFT2DTestParam(svt_aom_fft4x4_float_sse2, svt_aom_fft4x4_float_c,
                       svt_aom_ifft4x4_float_sse2, 4),
        FFT2DTestParam(svt_aom_fft8x8_float_avx2, svt_aom_fft8x8_float_c,
                       svt_aom_ifft8x8_float_avx2, 8),
        FFT2DTestParam(svt_aom_fft16x16_float_avx2, svt_aom_fft16x16_float_c,
                       svt_aom_ifft16x16_float_avx2, 16),
        FFT2DTestParam(svt_aom_fft32x32_float_avx2, svt_aom_fft32x32_float_c,
                       svt_aom_ifft32x32_float_avx2, 32),
        FFT2DTestParam(svt_aom_fft2x2_float_c, svt_aom_fft2x2_float_c,
                       svt_aom_ifft2x2_float_c, 2),
        FFT2DTestParam(svt_aom_fft4x4_float_sse2, svt_aom_fft4x4_float_c,
                       svt_aom_ifft4x4_float_c, 4),
        FFT2DTestParam(svt_aom_fft8x8_float_avx2, svt_aom_fft8x8_float_c,
                       svt_aom_ifft8x8_float_c, 8),
        FFT2DTestParam(svt_aom_fft16x16_float_avx2, svt_aom_fft16x16_float_c,
                       svt_aom_ifft16x16_float_c, 16),
        FFT2DTestParam(svt_aom_fft32x32_float_avx2, svt_aom_fft32x32_float_c,
                       svt_aom_ifft32x32_float_c, 32)));

class IFFT2DTest : public ::testing::TestWithParam<IFFT2DTestParam> {
  public:
    IFFT2DTest()
        : tst_fcn_(TEST_GET_PARAM(0)),
          ref_fcn_(TEST_GET_PARAM(1)),
          txfm_size_(TEST_GET_PARAM(2)) {
    }

    void SetUp() override {
        input_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, 2 * txfm_size_ * txfm_size_ * sizeof(float)));
        memset(input_, 0, 2 * txfm_size_ * txfm_size_ * sizeof(float));
        temp_tst_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(temp_tst_, 0, txfm_size_ * txfm_size_ * sizeof(float));
        temp_ref_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(temp_ref_, 0, txfm_size_ * txfm_size_ * sizeof(float));
        output_tst_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(output_tst_, 0, txfm_size_ * txfm_size_ * sizeof(float));
        output_ref_ = reinterpret_cast<float *>(
            svt_aom_memalign(32, txfm_size_ * txfm_size_ * sizeof(float)));
        memset(output_ref_, 0, txfm_size_ * txfm_size_ * sizeof(float));
    }

    void TearDown() override {
        svt_aom_free(input_);
        svt_aom_free(temp_tst_);
        svt_aom_free(temp_ref_);
        svt_aom_free(output_tst_);
        svt_aom_free(output_ref_);
    }

    void run_ifft_accuracy_check() {
        SVTRandom rnd(0.0f, 1.0f);
        const int test_times = 5000;
        for (int i = 0; i < test_times; ++i) {
            // prepare random test data
            int data_size = txfm_size_ * txfm_size_;
            for (int j = 0; j < data_size * 2; ++j)
                input_[j] = rnd.random_float();

            // calculate in reference FFT function
            ref_fcn_(input_, temp_ref_, output_ref_);
            // calculate in test FFT function
            tst_fcn_(input_, temp_tst_, output_tst_);

            // compare for the results
            for (int j = 0; j < data_size; ++j) {
                ASSERT_FLOAT_EQ(output_tst_[j], output_ref_[j])
                    << "txfm_size_: " << txfm_size_ << "[" << j << "]"
                    << " failed at test #" << i << "\n"
                    << print_data(input_, txfm_size_, txfm_size_);
            }
        }
    }

  protected:
    IFFTFloatFcn tst_fcn_; /**< pointer of IFFT test function */
    IFFTFloatFcn ref_fcn_; /**< pointer of IFFT reference function */
    int txfm_size_;        /**< transform size, max transform is DCT64 */
    float *input_;         /**< IFFT input data buffer */
    float *temp_tst_;      /**< temp buffer for IFFT test function */
    float *temp_ref_;      /**< temp buffer for IFFT test function */
    float *output_tst_;    /**< output buffer for IFFT test function */
    float *output_ref_;    /**< output buufer for IFFT reference function*/
};

TEST_P(IFFT2DTest, run_ifft_accuracy_check) {
    run_ifft_accuracy_check();
}

INSTANTIATE_TEST_SUITE_P(
    IFFT, IFFT2DTest,
    ::testing::Values(
        IFFT2DTestParam(svt_aom_ifft2x2_float_c, svt_aom_ifft2x2_float_c, 2),
        IFFT2DTestParam(svt_aom_ifft4x4_float_sse2, svt_aom_ifft4x4_float_c, 4),
        IFFT2DTestParam(svt_aom_ifft8x8_float_avx2, svt_aom_ifft8x8_float_c, 8),
        IFFT2DTestParam(svt_aom_ifft16x16_float_avx2, svt_aom_ifft16x16_float_c,
                        16),
        IFFT2DTestParam(svt_aom_ifft32x32_float_avx2, svt_aom_ifft32x32_float_c,
                        32)));
}  // namespace
