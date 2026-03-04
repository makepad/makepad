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
 * @file wiener_convolve_test.cc
 *
 * @brief Unit test of wiener convolve add source:
 * - av1_wiener_convolve_add_src
 * - av1_highbd_wiener_convolve_add_src
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "convolve.h"
#include "definitions.h"
#include "restoration.h"
#include "svt_time.h"
#include "utility.h"
#include "random.h"
#include "util.h"

/**
 * @brief Unit test of wiener convolve add source:
 * - svt_av1_wiener_convolve_add_src
 * - svt_av1_highbd_wiener_convolve_add_src
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check test output and reference output.
 * Define a templete class to handle the common process, and
 * declare sub class to handle different bitdepth and function types.
 *
 * Expected result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * BitDepth: 8bit, 10bit and 12bit
 */

namespace {
using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

static const int input_stride = 640;
static const int output_stride = 640;
static const int h = 128;
typedef std::tuple<int, int> BlkSize;

typedef void (*WienerConvolveFunc)(
    const uint8_t* const src, const ptrdiff_t src_stride, uint8_t* const dst,
    const ptrdiff_t dst_stride, const int16_t* const filter_x,
    const int16_t* const filter_y, const int32_t w, const int32_t h,
    const ConvolveParams* const conv_params);
typedef void (*HbdWienerConvolveFunc)(
    const uint8_t* const src, const ptrdiff_t src_stride, uint8_t* const dst,
    const ptrdiff_t dst_stride, const int16_t* const filter_x,
    const int16_t* const filter_y, const int32_t w, const int32_t h,
    const ConvolveParams* const conv_params, const int32_t bd);
typedef std::tuple<BlkSize, WienerConvolveFunc> WienerConvolveParam;
typedef std::tuple<BlkSize, HbdWienerConvolveFunc, int32_t>
    HbdWienerConvolveParam;

static const BlkSize test_block_size_table[] = {
    BlkSize(96, 96), BlkSize(96, 97), BlkSize(88, 88), BlkSize(88, 85),
    BlkSize(80, 80), BlkSize(80, 79), BlkSize(72, 72), BlkSize(72, 71),
    BlkSize(64, 64), BlkSize(64, 63), BlkSize(56, 56), BlkSize(56, 57),
    BlkSize(48, 48), BlkSize(48, 49), BlkSize(32, 32), BlkSize(32, 33),
    BlkSize(24, 24), BlkSize(24, 23), BlkSize(16, 16), BlkSize(16, 15),
    BlkSize(8, 9),   BlkSize(8, 8)};

static const int test_tap_table[] = {7, 5, 3};

template <typename Sample, typename FuncType, typename ParamType>
class AV1WienerConvolveTest : public ::testing::TestWithParam<ParamType> {
  public:
    AV1WienerConvolveTest() : rnd_(16, false) {
        input_ = nullptr;
        output_ = nullptr;
        output_tst_ = nullptr;
        output_ref_ = nullptr;
        out_w_ = 0;
        out_h_ = 0;
        func_tst_ = nullptr;
        bd_ = 8;
    }

    void SetUp() override {
        rnd_.reset();
        malloc_data();
    }

    void TearDown() override {
        if (input_) {
            svt_aom_free(input_);
            input_ = nullptr;
        }
        if (output_) {
            svt_aom_free(output_);
            output_ = nullptr;
        }
        if (output_tst_) {
            svt_aom_free(output_tst_);
            output_tst_ = nullptr;
        }
        if (output_ref_) {
            svt_aom_free(output_ref_);
            output_ref_ = nullptr;
        }
    }

  protected:
    void malloc_data() {
        input_ = reinterpret_cast<Sample*>(
            svt_aom_memalign(32, input_stride * h * sizeof(Sample)));
        ASSERT_NE(input_, nullptr) << "create input buffer failed!";
        output_ = reinterpret_cast<Sample*>(
            svt_aom_memalign(32, output_stride * h * sizeof(Sample)));
        ASSERT_NE(output_, nullptr) << "create output buffer failed!";
        output_tst_ = reinterpret_cast<Sample*>(
            svt_aom_memalign(32, output_stride * h * sizeof(Sample)));
        ASSERT_NE(output_tst_, nullptr) << "create test output buffer failed!";
        output_ref_ = reinterpret_cast<Sample*>(
            svt_aom_memalign(32, output_stride * h * sizeof(Sample)));
        ASSERT_NE(output_ref_, nullptr) << "create ref output buffer failed!";
    }

    void prepare_random_data() {
        for (int i = 0; i < input_stride * h; ++i)
            input_[i] = (Sample)rnd_.random() & ((1 << bd_) - 1);
        for (int i = 0; i < output_stride * h; ++i)
            output_[i] = (Sample)rnd_.random() & ((1 << bd_) - 1);
    }

    void reset_output() {
        memcpy(output_ref_, output_, output_stride * h * sizeof(*output_));
        memcpy(output_tst_, output_, output_stride * h * sizeof(*output_));
    }

    // Generate a random pair of filter kernels, using the ranges
    // of possible values from the loop-restoration experiment
    void generate_kernels(InterpKernel hkernel, InterpKernel vkernel, int tap,
                          int kernel_type = 2) {
        if (kernel_type == 0) {
            // Low possible values for filter coefficients
            hkernel[0] = vkernel[0] = WIENER_FILT_TAP0_MINV;
            hkernel[1] = vkernel[1] = WIENER_FILT_TAP1_MINV;
            hkernel[2] = vkernel[2] = WIENER_FILT_TAP2_MINV;
        } else if (kernel_type == 1) {
            // Max possible values for filter coefficients
            hkernel[0] = vkernel[0] = WIENER_FILT_TAP0_MAXV;
            hkernel[1] = vkernel[1] = WIENER_FILT_TAP1_MAXV;
            hkernel[2] = vkernel[2] = WIENER_FILT_TAP2_MAXV;
        } else if (kernel_type == 2) {
            // Randomly generated values for filter coefficients
            hkernel[0] = WIENER_FILT_TAP0_MINV +
                         pseudo_uniform(WIENER_FILT_TAP0_MAXV + 1 -
                                        WIENER_FILT_TAP0_MINV);
            hkernel[1] = WIENER_FILT_TAP1_MINV +
                         pseudo_uniform(WIENER_FILT_TAP1_MAXV + 1 -
                                        WIENER_FILT_TAP1_MINV);
            hkernel[2] = WIENER_FILT_TAP2_MINV +
                         pseudo_uniform(WIENER_FILT_TAP2_MAXV + 1 -
                                        WIENER_FILT_TAP2_MINV);

            vkernel[0] = WIENER_FILT_TAP0_MINV +
                         pseudo_uniform(WIENER_FILT_TAP0_MAXV + 2 -
                                        WIENER_FILT_TAP0_MINV);
            vkernel[1] = WIENER_FILT_TAP1_MINV +
                         pseudo_uniform(WIENER_FILT_TAP1_MAXV + 2 -
                                        WIENER_FILT_TAP1_MINV);
            vkernel[2] = WIENER_FILT_TAP2_MINV +
                         pseudo_uniform(WIENER_FILT_TAP2_MAXV + 2 -
                                        WIENER_FILT_TAP2_MINV);
        } else if (kernel_type == 3) {
            // Check zerocoff path
            hkernel[0] = vkernel[0] = 0;
            hkernel[1] = vkernel[1] = 0;
            hkernel[2] = vkernel[2] = 0;
        } else if (kernel_type == 4) {
            hkernel[0] = vkernel[0] = 0;
            hkernel[1] = vkernel[1] = 0;
            hkernel[2] = vkernel[2] = WIENER_FILT_TAP2_MAXV;
        } else if (kernel_type == 5) {
            hkernel[0] = vkernel[0] = 0;
            hkernel[1] = vkernel[1] = WIENER_FILT_TAP1_MAXV;
            hkernel[2] = vkernel[2] = WIENER_FILT_TAP2_MAXV;
        }

        if (tap <= 5) {
            hkernel[0] = vkernel[0] = 0;
            if (tap <= 3)
                hkernel[1] = vkernel[1] = 0;
        }
        hkernel[3] = -2 * (hkernel[0] + hkernel[1] + hkernel[2]);
        vkernel[3] = -2 * (vkernel[0] + vkernel[1] + vkernel[2]);
        hkernel[4] = hkernel[2];
        hkernel[5] = hkernel[1];
        hkernel[6] = hkernel[0];
        vkernel[4] = vkernel[2];
        vkernel[5] = vkernel[1];
        vkernel[6] = vkernel[0];
        hkernel[7] = vkernel[7] = 0;
    }

    int pseudo_uniform(const int range) {
        return rnd_.random() % (range + 1);
    }

    virtual void run_and_check(const InterpKernel& hkernel,
                               const InterpKernel& vkernel,
                               const ConvolveParams& params) = 0;

    virtual void speed_and_check(const InterpKernel& hkernel,
                                 const InterpKernel& vkernel,
                                 const ConvolveParams& params,
                                 const int tap) = 0;

    virtual void run_random_test(const int test_times) {
        // Generate random filter kernels
        DECLARE_ALIGNED(16, InterpKernel, hkernel);
        DECLARE_ALIGNED(16, InterpKernel, vkernel);

        prepare_random_data();

        const ConvolveParams conv_params = get_conv_params_wiener(bd_);

        for (unsigned int tap_idx = 0;
             tap_idx < sizeof(test_tap_table) / sizeof(*test_tap_table);
             tap_idx++) {
            for (int kernel_type = 0; kernel_type < 6; kernel_type++) {
                generate_kernels(
                    hkernel, vkernel, test_tap_table[tap_idx], kernel_type);
                for (int i = 0;
                     i < test_times && !::testing::Test::HasFatalFailure();
                     ++i) {
                    reset_output();
                    run_and_check(hkernel, vkernel, conv_params);
                }
            }
        }
    }

    virtual void run_speed_test() {
        // Generate random filter kernels
        DECLARE_ALIGNED(16, InterpKernel, hkernel);
        DECLARE_ALIGNED(16, InterpKernel, vkernel);

        prepare_random_data();
        reset_output();

        const ConvolveParams conv_params = get_conv_params_wiener(bd_);
        for (unsigned int tap_idx = 0;
             tap_idx < sizeof(test_tap_table) / sizeof(*test_tap_table);
             tap_idx++) {
            generate_kernels(hkernel, vkernel, test_tap_table[tap_idx]);
            speed_and_check(
                hkernel, vkernel, conv_params, test_tap_table[tap_idx]);
        }
    }

  protected:
    FuncType func_tst_;  /**< function to test */
    int32_t out_w_;      /**< width of output */
    int32_t out_h_;      /**< height of output */
    int32_t bd_;         /**< bit-depth */
    Sample* input_;      /**< buffer of input data */
    Sample* output_;     /**< buffer of test output data */
    Sample* output_tst_; /**< buffer of test output data */
    Sample* output_ref_; /**< buffer of reference output data */
    SVTRandom rnd_;      /**< random tools*/
};

class AV1WienerConvolveLbdTest
    : public AV1WienerConvolveTest<uint8_t, WienerConvolveFunc,
                                   WienerConvolveParam> {
  protected:
    void SetUp() override {
        out_w_ = std::get<0>(TEST_GET_PARAM(0));
        out_h_ = std::get<1>(TEST_GET_PARAM(0));
        func_tst_ = TEST_GET_PARAM(1);
        AV1WienerConvolveTest::SetUp();
    }

    void run_and_check(const InterpKernel& hkernel, const InterpKernel& vkernel,
                       const ConvolveParams& params) override {
        uint8_t* input = input_;
        // Choose random locations within the source block
        int offset_r = 3 + pseudo_uniform(h - out_h_ - 7);
        int offset_c = 3 + pseudo_uniform(input_stride - out_w_ - 7);
        svt_av1_wiener_convolve_add_src_c(
            input + offset_r * input_stride + offset_c,
            input_stride,
            output_ref_,
            output_stride,
            hkernel,
            vkernel,
            out_w_,
            out_h_,
            &params);
        func_tst_(input + offset_r * input_stride + offset_c,
                  input_stride,
                  output_tst_,
                  output_stride,
                  hkernel,
                  vkernel,
                  out_w_,
                  out_h_,
                  &params);

        if (memcmp(output_ref_, output_tst_, output_stride * h)) {
            for (int32_t j = 0; j < output_stride * h; ++j)
                ASSERT_EQ(output_tst_[j], output_ref_[j])
                    << "Pixel mismatch at index " << j << " = ("
                    << (j % output_stride) << ", " << (j / output_stride)
                    << ")";
        }
    }

    void speed_and_check(const InterpKernel& hkernel,
                         const InterpKernel& vkernel,
                         const ConvolveParams& params, const int tap) override {
        uint8_t* input = input_;
        // Choose random locations within the source block
        int offset_r = 3 + pseudo_uniform(h - out_h_ - 7);
        int offset_c = 3 + pseudo_uniform(input_stride - out_w_ - 7);
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const uint64_t num_loop = 10000000000 / (out_w_ * out_h_);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            svt_av1_wiener_convolve_add_src_c(
                input + offset_r * input_stride + offset_c,
                input_stride,
                output_ref_,
                output_stride,
                hkernel,
                vkernel,
                out_w_,
                out_h_,
                &params);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func_tst_(input + offset_r * input_stride + offset_c,
                      input_stride,
                      output_tst_,
                      output_stride,
                      hkernel,
                      vkernel,
                      out_w_,
                      out_h_,
                      &params);
        }

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("convolve(%3dx%3d, tap %d): %6.2f\n",
               out_w_,
               out_h_,
               tap,
               time_c / time_o);
    }
};

class AV1WienerConvolveHbdTest
    : public AV1WienerConvolveTest<uint16_t, HbdWienerConvolveFunc,
                                   HbdWienerConvolveParam> {
  protected:
    void SetUp() override {
        out_w_ = std::get<0>(TEST_GET_PARAM(0));
        out_h_ = std::get<1>(TEST_GET_PARAM(0));
        func_tst_ = TEST_GET_PARAM(1);
        bd_ = TEST_GET_PARAM(2);
        AV1WienerConvolveTest::SetUp();
    }

    void run_and_check(const InterpKernel& hkernel, const InterpKernel& vkernel,
                       const ConvolveParams& params) override {
        uint8_t* input = CONVERT_TO_BYTEPTR(input_);
        uint8_t* out_tst = CONVERT_TO_BYTEPTR(output_tst_);
        uint8_t* out_ref = CONVERT_TO_BYTEPTR(output_ref_);
        // Choose random locations within the source block
        int offset_r = 3 + pseudo_uniform(h - out_h_ - 7);
        int offset_c = 3 + pseudo_uniform(input_stride - out_w_ - 7);
        svt_av1_highbd_wiener_convolve_add_src_c(
            input + offset_r * input_stride + offset_c,
            input_stride,
            out_ref,
            output_stride,
            hkernel,
            vkernel,
            out_w_,
            out_h_,
            &params,
            bd_);
        func_tst_(input + offset_r * input_stride + offset_c,
                  input_stride,
                  out_tst,
                  output_stride,
                  hkernel,
                  vkernel,
                  out_w_,
                  out_h_,
                  &params,
                  bd_);

        if (memcmp(output_ref_, output_tst_, output_stride * h)) {
            for (int32_t j = 0; j < output_stride * h; ++j)
                ASSERT_EQ(output_tst_[j], output_ref_[j])
                    << "Pixel mismatch at index " << j << " = ("
                    << (j % output_stride) << ", " << (j / output_stride)
                    << ")";
        }
    }

    void speed_and_check(const InterpKernel& hkernel,
                         const InterpKernel& vkernel,
                         const ConvolveParams& params, const int tap) override {
        (void)hkernel;
        (void)vkernel;
        (void)params;
        (void)tap;
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(AV1WienerConvolveHbdTest);

TEST_P(AV1WienerConvolveLbdTest, random_test) {
    run_random_test(1000);
}

TEST_P(AV1WienerConvolveLbdTest, DISABLED_speed_test) {
    run_speed_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, AV1WienerConvolveLbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_wiener_convolve_add_src_sse2)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, AV1WienerConvolveLbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_wiener_convolve_add_src_avx2)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, AV1WienerConvolveLbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_wiener_convolve_add_src_avx512)));
#endif
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AV1WienerConvolveLbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_wiener_convolve_add_src_neon)));
#endif  // ARCH_AARCH64

TEST_P(AV1WienerConvolveHbdTest, random_test) {
    run_random_test(1000);
}

TEST_P(AV1WienerConvolveHbdTest, DISABLED_speed_test) {
    run_speed_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSSE3, AV1WienerConvolveHbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_highbd_wiener_convolve_add_src_ssse3),
        testing::Values(8, 10, 12)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, AV1WienerConvolveHbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_highbd_wiener_convolve_add_src_avx2),
        testing::Values(8, 10, 12)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AV1WienerConvolveHbdTest,
    ::testing::Combine(
        ::testing::ValuesIn(test_block_size_table),
        ::testing::Values(svt_av1_highbd_wiener_convolve_add_src_neon),
        testing::Values(8, 10)));
#endif  // ARCH_AARCH64

}  // namespace
