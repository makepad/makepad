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
 * @file Convolve2dTest.cc
 *
 * @brief Unit test for interpolation in inter prediction:
 * - svt_av1_highbd_convolve_2d_copy_sr_avx2
 * - svt_av1_highbd_jnt_convolve_2d_copy_avx2
 * - svt_av1_highbd_convolve_x_sr_avx2
 * - svt_av1_highbd_convolve_y_sr_avx2
 * - svt_av1_highbd_convolve_2d_sr_avx2
 * - svt_av1_highbd_jnt_convolve_x_avx2
 * - svt_av1_highbd_jnt_convolve_y_avx2
 * - svt_av1_highbd_jnt_convolve_2d_avx2
 * - svt_av1_convolve_2d_copy_sr_avx2
 * - svt_av1_jnt_convolve_2d_copy_avx2
 * - svt_av1_convolve_x_sr_avx2
 * - svt_av1_convolve_y_sr_avx2
 * - svt_av1_convolve_2d_sr_avx2
 * - svt_av1_jnt_convolve_x_avx2
 * - svt_av1_jnt_convolve_y_avx2
 * - svt_av1_jnt_convolve_2d_avx2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include <stdlib.h>
#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "random.h"
#include "util.h"
#include "svt_time.h"
#include "utility.h"
#include "convolve.h"
#include "filter.h"

#ifdef ARCH_X86_64
#include "convolve_avx2.h"
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
#include "convolve_neon.h"
#endif  // ARCH_AARCH64

#if defined(_MSC_VER)
#pragma warning(suppress : 4324)
#endif

/**
 * @brief Unit test for interpolation in inter prediction:
 * - av1_{highbd, }_{jnt, }_convolve_{x, y, 2d}_{sr, copy}
 *
 * Test strategy:
 * Verify this assembly code by comparing with reference c implementation.
 * Feed the same data and check test output and reference output.
 * Define a template class to handle the common process, and
 * declare sub class to handle different bitdepth and function types.
 *
 * Expect result:
 * Output from assemble functions should be the same with output from c.
 *
 * Test coverage:
 * Test cases:
 * input value: Fill with random values
 * modes: jnt, sr, copy, x, y modes
 * TxSize: all the TxSize.
 * BitDepth: 8bit, 10bit, 12bit
 *
 */

const int kMaxSize = 128 + 32;  // padding
using svt_av1_test_tool::SVTRandom;
namespace {

using highbd_convolve_func =
    void (*)(const uint16_t *src, int src_stride, uint16_t *dst, int dst_stride,
             int w, int h, const InterpFilterParams *filter_params_x,
             const InterpFilterParams *filter_params_y, const int subpel_x_qn,
             const int subpel_y_qn, ConvolveParams *conv_params, int bd);

using lowbd_convolve_func = void (*)(const uint8_t *src, int src_stride,
                                     uint8_t *dst, int dst_stride, int w, int h,
                                     const InterpFilterParams *filter_params_x,
                                     const InterpFilterParams *filter_params_y,
                                     const int subpel_x_qn,
                                     const int subpel_y_qn,
                                     ConvolveParams *conv_params);

using LowbdConvolveParam =
    std::tuple<int, int, int, lowbd_convolve_func, BlockSize>;
using HighbdConvolveParam =
    std::tuple<int, int, int, highbd_convolve_func, BlockSize>;

template <typename Sample, typename FuncType, typename ConvolveParam>
class AV1ConvolveTest : public ::testing::TestWithParam<ConvolveParam> {
  public:
    virtual ~AV1ConvolveTest() {
    }

    // make the address aligned to 32.
    void SetUp() override {
        conv_buf_init_ = reinterpret_cast<ConvBufType *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(ConvBufType)));
        conv_buf_ref_ = reinterpret_cast<ConvBufType *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(ConvBufType)));
        conv_buf_tst_ = reinterpret_cast<ConvBufType *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(ConvBufType)));
        output_init_ = reinterpret_cast<Sample *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(Sample)));
        output_ref_ = reinterpret_cast<Sample *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(Sample)));
        output_tst_ = reinterpret_cast<Sample *>(
            svt_aom_memalign(32, MAX_SB_SQUARE * sizeof(Sample)));
    }

    void TearDown() override {
        svt_aom_free(conv_buf_init_);
        svt_aom_free(conv_buf_ref_);
        svt_aom_free(conv_buf_tst_);
        svt_aom_free(output_init_);
        svt_aom_free(output_ref_);
        svt_aom_free(output_tst_);
    }

    virtual void run_convolve(int offset_r, int offset_c, int src_stride,
                              int dst_stride, int w, int h,
                              const InterpFilterParams *filter_params_x,
                              const InterpFilterParams *filter_params_y,
                              const int32_t subpel_x_q4,
                              const int32_t subpel_y_q4,
                              ConvolveParams *conv_params1,
                              ConvolveParams *conv_params2) = 0;

    virtual void speed_convolve(int offset_r, int offset_c, int src_stride,
                                int dst_stride, int w, int h,
                                const InterpFilterParams *filter_params_x,
                                const InterpFilterParams *filter_params_y,
                                const int32_t subpel_x_q4,
                                const int32_t subpel_y_q4,
                                ConvolveParams *conv_params1,
                                ConvolveParams *conv_params2) = 0;

    void test_convolve(int has_subx, int has_suby, int src_stride,
                       int dst_stride, int output_w, int output_h,
                       const InterpFilterParams *filter_params_x,
                       const InterpFilterParams *filter_params_y,
                       ConvolveParams *conv_params_ref,
                       ConvolveParams *conv_params_tst) {
        const int subx_range = has_subx ? 16 : 1;
        const int suby_range = has_suby ? 16 : 1;
        int subx, suby;
        for (subx = 0; subx < subx_range; ++subx) {
            for (suby = 0; suby < suby_range; ++suby) {
                const int offset_r = 3;
                const int offset_c = 3;

                reset_output();

                run_convolve(offset_r,
                             offset_c,
                             src_stride,
                             dst_stride,
                             output_w,
                             output_h,
                             filter_params_x,
                             filter_params_y,
                             subx,
                             suby,
                             conv_params_ref,
                             conv_params_tst);

                if (memcmp(output_ref_,
                           output_tst_,
                           MAX_SB_SQUARE * sizeof(output_ref_[0]))) {
                    for (int i = 0; i < MAX_SB_SIZE; ++i) {
                        for (int j = 0; j < MAX_SB_SIZE; ++j) {
                            int idx = i * MAX_SB_SIZE + j;
                            ASSERT_EQ(output_ref_[idx], output_tst_[idx])
                                << output_w << "x" << output_h
                                << " Pixel mismatch at "
                                   "index "
                                << idx << " = (" << j << ", " << i
                                << "), sub pixel offset = (" << suby << ", "
                                << subx << ")"
                                << " tap = ("
                                << get_convolve_tap(filter_params_x->filter_ptr)
                                << "x"
                                << get_convolve_tap(filter_params_y->filter_ptr)
                                << ") do_average: "
                                << conv_params_tst->do_average
                                << " use_jnt_comp_avg: "
                                << conv_params_tst->use_jnt_comp_avg;
                        }
                    }
                }

                if (is_jnt_) {
                    if (memcmp(conv_buf_ref_,
                               conv_buf_tst_,
                               MAX_SB_SQUARE * sizeof(conv_buf_ref_[0]))) {
                        for (int i = 0; i < MAX_SB_SIZE; ++i) {
                            for (int j = 0; j < MAX_SB_SIZE; ++j) {
                                int idx = i * MAX_SB_SIZE + j;
                                ASSERT_EQ(conv_buf_ref_[idx],
                                          conv_buf_tst_[idx])
                                    << output_w << "x" << output_h
                                    << " Pixel mismatch at index " << idx
                                    << " = (" << j << ", " << i
                                    << "), sub pixel offset = (" << suby << ", "
                                    << subx << ")"
                                    << " tap = ("
                                    << get_convolve_tap(
                                           filter_params_x->filter_ptr)
                                    << "x"
                                    << get_convolve_tap(
                                           filter_params_y->filter_ptr)
                                    << ") do_average: "
                                    << conv_params_tst->do_average
                                    << " use_jnt_comp_avg: "
                                    << conv_params_tst->use_jnt_comp_avg;
                            }
                        }
                    }
                }
            }
        }
    }

    void test_speed(int has_subx, int has_suby, int src_stride, int dst_stride,
                    int output_w, int output_h,
                    const InterpFilterParams *filter_params_x,
                    const InterpFilterParams *filter_params_y,
                    ConvolveParams *conv_params_ref,
                    ConvolveParams *conv_params_tst) {
        const int subx_range = has_subx ? 16 : 1;
        const int suby_range = has_suby ? 16 : 1;
        int subx, suby;
        for (subx = 0; subx < subx_range; ++subx) {
            if (subx)
                continue;
            for (suby = 0; suby < suby_range; ++suby) {
                if (suby)
                    continue;
                const int offset_r = 3;
                const int offset_c = 3;

                speed_convolve(offset_r,
                               offset_c,
                               src_stride,
                               dst_stride,
                               output_w,
                               output_h,
                               filter_params_x,
                               filter_params_y,
                               subx,
                               suby,
                               conv_params_ref,
                               conv_params_tst);

                if (memcmp(output_ref_,
                           output_tst_,
                           MAX_SB_SQUARE * sizeof(output_ref_[0]))) {
                    for (int i = 0; i < output_h; ++i) {
                        for (int j = 0; j < output_w; ++j) {
                            int idx = i * MAX_SB_SIZE + j;
                            ASSERT_EQ(output_ref_[idx], output_tst_[idx])
                                << output_w << "x" << output_h
                                << " Pixel mismatch at "
                                   "index "
                                << idx << " = (" << j << ", " << i
                                << "), sub pixel offset = (" << suby << ", "
                                << subx << ") do_average: "
                                << conv_params_tst->do_average
                                << " use_jnt_comp_avg: "
                                << conv_params_tst->use_jnt_comp_avg;
                        }
                    }
                }

                if (is_jnt_) {
                    if (memcmp(conv_buf_ref_,
                               conv_buf_tst_,
                               MAX_SB_SQUARE * sizeof(conv_buf_ref_[0]))) {
                        for (int i = 0; i < output_h; ++i) {
                            for (int j = 0; j < output_w; ++j) {
                                int idx = i * MAX_SB_SIZE + j;
                                ASSERT_EQ(conv_buf_ref_[idx],
                                          conv_buf_tst_[idx])
                                    << output_w << "x" << output_h
                                    << " Pixel mismatch at index " << idx
                                    << " = (" << j << ", " << i
                                    << "), sub pixel offset = (" << suby << ", "
                                    << subx << ") do_average: "
                                    << conv_params_tst->do_average
                                    << " use_jnt_comp_avg: "
                                    << conv_params_tst->use_jnt_comp_avg;
                            }
                        }
                    }
                }
            }
        }
    }

  protected:
    void prepare_data(int w, int h) {
        SVTRandom rnd_(bd_, false);  // bd_-bits, unsigned
        SVTRandom rnd12_(12, false);

        for (int i = 0; i < h; ++i)
            for (int j = 0; j < w; ++j)
                input[i * w + j] = (Sample)rnd_.random();

        for (int i = 0; i < MAX_SB_SQUARE; ++i) {
            conv_buf_init_[i] = rnd12_.random();
            output_init_[i] = (Sample)rnd_.random();
        }
    }

    void reset_output() {
        memcpy(conv_buf_ref_,
               conv_buf_init_,
               MAX_SB_SQUARE * sizeof(*conv_buf_init_));
        memcpy(conv_buf_tst_,
               conv_buf_init_,
               MAX_SB_SQUARE * sizeof(*conv_buf_init_));
        memcpy(
            output_ref_, output_init_, MAX_SB_SQUARE * sizeof(*output_init_));
        memcpy(
            output_tst_, output_init_, MAX_SB_SQUARE * sizeof(*output_init_));
    }

    void run_test() {
        const int input_w = kMaxSize, input_h = kMaxSize;
        int hfilter, vfilter;

        // fill the input data with random
        prepare_data(input_w, input_h);

        svt_aom_setup_common_rtcd_internal(svt_aom_get_cpu_flags_to_use());

        // loop the filter type and subpixel position
        const int output_w = block_size_wide[block_idx_];
        const int output_h = block_size_high[block_idx_];
        const InterpFilter max_hfilter =
            has_subx_ ? INTERP_FILTERS_ALL : EIGHTTAP_SMOOTH;
        const InterpFilter max_vfilter =
            has_suby_ ? INTERP_FILTERS_ALL : EIGHTTAP_SMOOTH;
        for (int compIdx = 0; compIdx < 2; ++compIdx) {
            for (hfilter = EIGHTTAP_REGULAR; hfilter < max_hfilter; ++hfilter) {
                for (vfilter = EIGHTTAP_REGULAR; vfilter < max_vfilter;
                     ++vfilter) {
                    InterpFilterParams filter_params_x =
                        av1_get_interp_filter_params_with_block_size(
                            (InterpFilter)hfilter, output_w >> compIdx);
                    InterpFilterParams filter_params_y =
                        av1_get_interp_filter_params_with_block_size(
                            (InterpFilter)vfilter, output_h >> compIdx);
                    for (int do_average = 0; do_average < 1 + is_jnt_;
                         ++do_average) {
                        // setup convolveParams according to jnt or sr
                        ConvolveParams conv_params_ref;
                        ConvolveParams conv_params_tst;
                        if (is_jnt_) {
                            conv_params_ref =
                                get_conv_params_no_round(0,
                                                         do_average,
                                                         0,
                                                         conv_buf_ref_,
                                                         MAX_SB_SIZE,
                                                         1,
                                                         bd_);
                            conv_params_tst =
                                get_conv_params_no_round(0,
                                                         do_average,
                                                         0,
                                                         conv_buf_tst_,
                                                         MAX_SB_SIZE,
                                                         1,
                                                         bd_);
                            // Test special case where dist_wtd_comp_avg is not
                            // used
                            conv_params_ref.use_jnt_comp_avg = 0;
                            conv_params_tst.use_jnt_comp_avg = 0;

                        } else {
                            conv_params_ref = get_conv_params_no_round(
                                0, do_average, 0, nullptr, 0, 0, bd_);
                            conv_params_tst = get_conv_params_no_round(
                                0, do_average, 0, nullptr, 0, 0, bd_);
                        }

                        test_convolve(has_subx_,
                                      has_suby_,
                                      input_w,
                                      MAX_SB_SIZE,
                                      output_w >> compIdx,
                                      output_h >> compIdx,
                                      &filter_params_x,
                                      &filter_params_y,
                                      &conv_params_ref,
                                      &conv_params_tst);

                        // AV1 standard won't have 32x4 case.
                        // This only favors some optimization feature which
                        // subsamples 32x8 to 32x4 and triggers 4-tap filter.
                        if ((is_jnt_ == 0) && has_suby_ &&
                            ((output_w >> compIdx) == 32) &&
                            ((output_h >> compIdx) == 8)) {
                            filter_params_y =
                                av1_get_interp_filter_params_with_block_size(
                                    (InterpFilter)vfilter, 4);
                            test_convolve(has_subx_,
                                          has_suby_,
                                          input_w,
                                          MAX_SB_SIZE,
                                          32,
                                          4,
                                          &filter_params_x,
                                          &filter_params_y,
                                          &conv_params_ref,
                                          &conv_params_tst);
                        }

                        if (is_jnt_ == 0)
                            continue;

                        const int quant_dist_lookup_table[2][4][2] = {
                            {{9, 7}, {11, 5}, {12, 4}, {13, 3}},
                            {{7, 9}, {5, 11}, {4, 12}, {3, 13}},
                        };
                        // Test different combination of fwd and bck offset
                        // weights
                        for (int k = 0; k < 2; ++k) {
                            for (int l = 0; l < 4; ++l) {
                                conv_params_ref.use_jnt_comp_avg = 1;
                                conv_params_tst.use_jnt_comp_avg = 1;
                                conv_params_ref.fwd_offset =
                                    quant_dist_lookup_table[k][l][0];
                                conv_params_ref.bck_offset =
                                    quant_dist_lookup_table[k][l][1];
                                conv_params_tst.fwd_offset =
                                    quant_dist_lookup_table[k][l][0];
                                conv_params_tst.bck_offset =
                                    quant_dist_lookup_table[k][l][1];

                                test_convolve(has_subx_,
                                              has_suby_,
                                              input_w,
                                              MAX_SB_SIZE,
                                              output_w >> compIdx,
                                              output_h >> compIdx,
                                              &filter_params_x,
                                              &filter_params_y,
                                              &conv_params_ref,
                                              &conv_params_tst);
                            }
                        }
                    }
                }
            }
        }
    }

    void speed_test() {
        const int input_w = kMaxSize, input_h = kMaxSize;
        int hfilter, vfilter;

        // fill the input data with random
        prepare_data(input_w, input_h);
        reset_output();

        // loop the filter type and subpixel position
        const int output_w = block_size_wide[block_idx_];
        const int output_h = block_size_high[block_idx_];
        const InterpFilter max_hfilter =
            has_subx_ ? INTERP_FILTERS_ALL : EIGHTTAP_SMOOTH;
        const InterpFilter max_vfilter =
            has_suby_ ? INTERP_FILTERS_ALL : EIGHTTAP_SMOOTH;
        for (hfilter = EIGHTTAP_REGULAR; hfilter < max_hfilter; ++hfilter) {
            if ((max_hfilter == INTERP_FILTERS_ALL) && (hfilter != BILINEAR))
                continue;
            for (vfilter = EIGHTTAP_REGULAR; vfilter < max_vfilter; ++vfilter) {
                if ((max_vfilter == INTERP_FILTERS_ALL) &&
                    (vfilter != BILINEAR))
                    continue;

                InterpFilterParams filter_params_x =
                    av1_get_interp_filter_params_with_block_size(
                        (InterpFilter)hfilter, output_w);
                InterpFilterParams filter_params_y =
                    av1_get_interp_filter_params_with_block_size(
                        (InterpFilter)vfilter, output_h);
                const int32_t h_tap =
                    get_convolve_tap(filter_params_x.filter_ptr);
                const int32_t v_tap =
                    get_convolve_tap(filter_params_y.filter_ptr);
                if (h_tap != v_tap)
                    continue;
                // setup convolveParams according to jnt or sr
                ConvolveParams conv_params_ref;
                ConvolveParams conv_params_tst;

                conv_params_ref = get_conv_params_no_round(
                    0, is_jnt_, 0, conv_buf_ref_, MAX_SB_SIZE, is_jnt_, bd_);
                conv_params_tst = get_conv_params_no_round(
                    0, is_jnt_, 0, conv_buf_tst_, MAX_SB_SIZE, is_jnt_, bd_);
                conv_params_ref.use_jnt_comp_avg = 0;
                conv_params_tst.use_jnt_comp_avg = 0;
                conv_params_ref.fwd_offset = conv_params_tst.fwd_offset = 9;
                conv_params_ref.bck_offset = conv_params_tst.bck_offset = 7;

                test_speed(has_subx_,
                           has_suby_,
                           input_w,
                           MAX_SB_SIZE,
                           output_w,
                           output_h,
                           &filter_params_x,
                           &filter_params_y,
                           &conv_params_ref,
                           &conv_params_tst);
            }
        }
    }

  protected:
    FuncType func_ref_;
    FuncType func_tst_;
    int bd_;
    int is_jnt_;  // jnt or single reference
    int has_subx_;
    int has_suby_;
    int block_idx_;
    Sample input[kMaxSize * kMaxSize];
    ConvBufType *conv_buf_init_;  // aligned address
    ConvBufType *conv_buf_ref_;   // aligned address
    ConvBufType *conv_buf_tst_;   // aligned address
    Sample *output_init_;         // aligned address
    Sample *output_ref_;          // aligned address
    Sample *output_tst_;          // aligned address
};

::testing::internal::ParamGenerator<LowbdConvolveParam> BuildParamsLbd(
    int has_subx, int has_suby, lowbd_convolve_func func) {
    return ::testing::Combine(::testing::Values(8),
                              ::testing::Values(has_subx),
                              ::testing::Values(has_suby),
                              ::testing::Values(func),
                              ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL));
}

class AV1LbdConvolveTest
    : public AV1ConvolveTest<uint8_t, lowbd_convolve_func, LowbdConvolveParam> {
  public:
    AV1LbdConvolveTest() {
    }
    virtual ~AV1LbdConvolveTest() {
    }
    void run_convolve(int offset_r, int offset_c, int src_stride,
                      int dst_stride, int output_w, int output_h,
                      const InterpFilterParams *filter_params_x,
                      const InterpFilterParams *filter_params_y,
                      const int32_t subpel_x_q4, const int32_t subpel_y_q4,
                      ConvolveParams *conv_params_ref,
                      ConvolveParams *conv_params_tst) override {
        func_ref_(input + offset_r * src_stride + offset_c,
                  src_stride,
                  output_ref_,
                  dst_stride,
                  output_w,
                  output_h,
                  filter_params_x,
                  filter_params_y,
                  subpel_x_q4,
                  subpel_y_q4,
                  conv_params_ref);
        func_tst_(input + offset_r * src_stride + offset_c,
                  src_stride,
                  output_tst_,
                  dst_stride,
                  output_w,
                  output_h,
                  filter_params_x,
                  filter_params_y,
                  subpel_x_q4,
                  subpel_y_q4,
                  conv_params_tst);
    }

    void speed_convolve(int offset_r, int offset_c, int src_stride,
                        int dst_stride, int output_w, int output_h,
                        const InterpFilterParams *filter_params_x,
                        const InterpFilterParams *filter_params_y,
                        const int32_t subpel_x_q4, const int32_t subpel_y_q4,
                        ConvolveParams *conv_params_ref,
                        ConvolveParams *conv_params_tst) override {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const uint64_t num_loop = 10000000000 / (output_w * output_h);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func_ref_(input + offset_r * src_stride + offset_c,
                      src_stride,
                      output_ref_,
                      dst_stride,
                      output_w,
                      output_h,
                      filter_params_x,
                      filter_params_y,
                      subpel_x_q4,
                      subpel_y_q4,
                      conv_params_ref);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func_tst_(input + offset_r * src_stride + offset_c,
                      src_stride,
                      output_tst_,
                      dst_stride,
                      output_w,
                      output_h,
                      filter_params_x,
                      filter_params_y,
                      subpel_x_q4,
                      subpel_y_q4,
                      conv_params_tst);
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

        printf("convolve(%3dx%3d, tap (%d, %d)): %6.2f\n",
               output_w,
               output_h,
               get_convolve_tap(filter_params_x->filter_ptr),
               get_convolve_tap(filter_params_y->filter_ptr),
               time_c / time_o);
    }
};

class AV1LbdJntConvolveTest : public AV1LbdConvolveTest {
  public:
    AV1LbdJntConvolveTest() {
        is_jnt_ = 1;
        bd_ = TEST_GET_PARAM(0);
        has_subx_ = TEST_GET_PARAM(1);
        has_suby_ = TEST_GET_PARAM(2);
        func_tst_ = TEST_GET_PARAM(3);
        block_idx_ = TEST_GET_PARAM(4);
        func_ref_ = svt_av1_jnt_convolve_2d_c;
    }
    virtual ~AV1LbdJntConvolveTest() {
    }
};

TEST_P(AV1LbdJntConvolveTest, MatchTest) {
    run_test();
}

TEST_P(AV1LbdJntConvolveTest, DISABLED_SpeedTest) {
    speed_test();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_AVX2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_jnt_convolve_2d_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_AVX2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_jnt_convolve_x_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_AVX2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_jnt_convolve_y_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_AVX2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_jnt_convolve_2d_copy_avx2));

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SSE2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_jnt_convolve_2d_sse2));
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SSSE3, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_jnt_convolve_2d_ssse3));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_SSE2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_jnt_convolve_x_sse2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_SSE2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_jnt_convolve_y_sse2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_SSE2, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_jnt_convolve_2d_copy_sse2));
#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_AVX512, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_jnt_convolve_2d_avx512));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_AVX512, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_jnt_convolve_x_avx512));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_AVX512, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_jnt_convolve_y_avx512));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_AVX512, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_jnt_convolve_2d_copy_avx512));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_jnt_convolve_2d_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_jnt_convolve_x_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_jnt_convolve_y_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_NEON, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_jnt_convolve_2d_copy_neon));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON_DOTPROD, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1,
                                        svt_av1_jnt_convolve_2d_neon_dotprod));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON_DOTPROD, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 0,
                                        svt_av1_jnt_convolve_x_neon_dotprod));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON_DOTPROD, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 1,
                                        svt_av1_jnt_convolve_y_neon_dotprod));
#endif  // HAVE_DOTPROD

#if HAVE_NEON_I8MM
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON_I8MM, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 1,
                                        svt_av1_jnt_convolve_2d_neon_i8mm));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON_I8MM, AV1LbdJntConvolveTest,
                         BuildParamsLbd(1, 0,
                                        svt_av1_jnt_convolve_x_neon_i8mm));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON_I8MM, AV1LbdJntConvolveTest,
                         BuildParamsLbd(0, 1,
                                        svt_av1_jnt_convolve_y_neon_i8mm));
#endif  // HAVE_NEON_I8MM
#endif  // ARCH_AARCH64

class AV1LbdSrConvolveTest : public AV1LbdConvolveTest {
  public:
    AV1LbdSrConvolveTest() {
        is_jnt_ = 0;
        bd_ = TEST_GET_PARAM(0);
        has_subx_ = TEST_GET_PARAM(1);
        has_suby_ = TEST_GET_PARAM(2);
        func_tst_ = TEST_GET_PARAM(3);
        block_idx_ = TEST_GET_PARAM(4);
        func_ref_ = svt_av1_convolve_2d_sr_c;
    }
    virtual ~AV1LbdSrConvolveTest() {
    }
};

TEST_P(AV1LbdSrConvolveTest, MatchTest) {
    run_test();
}

TEST_P(AV1LbdSrConvolveTest, DISABLED_SpeedTest) {
    speed_test();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_AVX2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_convolve_2d_sr_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_AVX2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_convolve_x_sr_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_AVX2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_convolve_y_sr_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_AVX2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_convolve_2d_copy_sr_avx2));

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SSE2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_convolve_2d_sr_sse2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_SSE2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_convolve_x_sr_sse2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_SSE2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_convolve_y_sr_sse2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_SSE2, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_convolve_2d_copy_sr_sse2));
#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_AVX512, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_convolve_2d_sr_avx512));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_AVX512, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_convolve_x_sr_avx512));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_AVX512, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_convolve_y_sr_avx512));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_AVX512, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_convolve_2d_copy_sr_avx512));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 1, svt_av1_convolve_2d_sr_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_convolve_x_sr_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_convolve_y_sr_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestCOPY_NEON, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 0,
                                        svt_av1_convolve_2d_copy_sr_neon));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON_DOTPROD, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 1,
                                        svt_av1_convolve_2d_sr_neon_dotprod));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON_DOTPROD, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 0,
                                        svt_av1_convolve_x_sr_neon_dotprod));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON_DOTPROD, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 1,
                                        svt_av1_convolve_y_sr_neon_dotprod));
#endif  // HAVE_NEON_DOTPROD

#if HAVE_NEON_I8MM
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON_I8MM, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 1,
                                        svt_av1_convolve_2d_sr_neon_i8mm));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON_I8MM, AV1LbdSrConvolveTest,
                         BuildParamsLbd(1, 0, svt_av1_convolve_x_sr_neon_i8mm));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON_I8MM, AV1LbdSrConvolveTest,
                         BuildParamsLbd(0, 1, svt_av1_convolve_y_sr_neon_i8mm));
#endif  // HAVE_NEON_I8MM
#endif  // ARCH_AARCH64

::testing::internal::ParamGenerator<HighbdConvolveParam> BuildParamsHbd(
    int has_subx, int has_suby, highbd_convolve_func func) {
    return ::testing::Combine(::testing::Range(8, 11, 2),
                              ::testing::Values(has_subx),
                              ::testing::Values(has_suby),
                              ::testing::Values(func),
                              ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL));
}

class AV1HbdConvolveTest
    : public AV1ConvolveTest<uint16_t, highbd_convolve_func,
                             HighbdConvolveParam> {
  public:
    AV1HbdConvolveTest() {
    }
    virtual ~AV1HbdConvolveTest() {
    }
    void run_convolve(int offset_r, int offset_c, int src_stride,
                      int dst_stride, int blk_w, int blk_h,
                      const InterpFilterParams *filter_params_x,
                      const InterpFilterParams *filter_params_y,
                      const int32_t subpel_x_q4, const int32_t subpel_y_q4,
                      ConvolveParams *conv_params_ref,
                      ConvolveParams *conv_params_tst) override {
        func_ref_(input + offset_r * src_stride + offset_c,
                  src_stride,
                  output_ref_,
                  dst_stride,
                  blk_w,
                  blk_h,
                  filter_params_x,
                  filter_params_y,
                  subpel_x_q4,
                  subpel_y_q4,
                  conv_params_ref,
                  bd_);
        func_tst_(input + offset_r * src_stride + offset_c,
                  src_stride,
                  output_tst_,
                  dst_stride,
                  blk_w,
                  blk_h,
                  filter_params_x,
                  filter_params_y,
                  subpel_x_q4,
                  subpel_y_q4,
                  conv_params_tst,
                  bd_);
    }

    void speed_convolve(int offset_r, int offset_c, int src_stride,
                        int dst_stride, int output_w, int output_h,
                        const InterpFilterParams *filter_params_x,
                        const InterpFilterParams *filter_params_y,
                        const int32_t subpel_x_q4, const int32_t subpel_y_q4,
                        ConvolveParams *conv_params_ref,
                        ConvolveParams *conv_params_tst) override {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const uint64_t num_loop = 10000000 / (output_w * output_h);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func_ref_(input + offset_r * src_stride + offset_c,
                      src_stride,
                      output_ref_,
                      dst_stride,
                      output_w,
                      output_h,
                      filter_params_x,
                      filter_params_y,
                      subpel_x_q4,
                      subpel_y_q4,
                      conv_params_ref,
                      bd_);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func_tst_(input + offset_r * src_stride + offset_c,
                      src_stride,
                      output_tst_,
                      dst_stride,
                      output_w,
                      output_h,
                      filter_params_x,
                      filter_params_y,
                      subpel_x_q4,
                      subpel_y_q4,
                      conv_params_tst,
                      bd_);
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

        printf(
            "convolve(%3dx%3d): %6.2f\n", output_w, output_h, time_c / time_o);
    }
};

class AV1HbdJntConvolveTest : public AV1HbdConvolveTest {
  public:
    AV1HbdJntConvolveTest() {
        is_jnt_ = 1;
        bd_ = TEST_GET_PARAM(0);
        has_subx_ = TEST_GET_PARAM(1);
        has_suby_ = TEST_GET_PARAM(2);
        func_tst_ = TEST_GET_PARAM(3);
        block_idx_ = TEST_GET_PARAM(4);
        func_ref_ = svt_av1_highbd_jnt_convolve_2d_c;
    }

    virtual ~AV1HbdJntConvolveTest() {
    }
};

TEST_P(AV1HbdJntConvolveTest, MatchTest) {
    run_test();
}

TEST_P(AV1HbdJntConvolveTest, DISABLED_SpeedTest) {
    speed_test();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SSE4_1, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_jnt_convolve_2d_sse4_1));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_SSE4_1, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_jnt_convolve_x_sse4_1));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_SSE4_1, AV1HbdJntConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_jnt_convolve_y_sse4_1));
INSTANTIATE_TEST_SUITE_P(
    ConvolveTestCOPY_SSE4_1, AV1HbdJntConvolveTest,
    BuildParamsHbd(0, 0, svt_av1_highbd_jnt_convolve_2d_copy_sse4_1));

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_AVX2, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_jnt_convolve_2d_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_AVX2, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_jnt_convolve_x_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_AVX2, AV1HbdJntConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_jnt_convolve_y_avx2));
INSTANTIATE_TEST_SUITE_P(
    ConvolveTestCOPY_AVX2, AV1HbdJntConvolveTest,
    BuildParamsHbd(0, 0, svt_av1_highbd_jnt_convolve_2d_copy_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_jnt_convolve_2d_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_jnt_convolve_x_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON, AV1HbdJntConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_jnt_convolve_y_neon));
INSTANTIATE_TEST_SUITE_P(
    ConvolveTestCOPY_NEON, AV1HbdJntConvolveTest,
    BuildParamsHbd(0, 0, svt_av1_highbd_jnt_convolve_2d_copy_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_SVE, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_jnt_convolve_x_sve));
#endif  // HAVE_SVE

#if HAVE_SVE2
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SVE2, AV1HbdJntConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_jnt_convolve_2d_sve2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_SVE2, AV1HbdJntConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_jnt_convolve_y_sve2));
#endif  // HAVE_SVE2
#endif  // ARCH_AARCH64

class AV1HbdSrConvolveTest : public AV1HbdConvolveTest {
  public:
    AV1HbdSrConvolveTest() {
        is_jnt_ = 0;
        bd_ = TEST_GET_PARAM(0);
        has_subx_ = TEST_GET_PARAM(1);
        has_suby_ = TEST_GET_PARAM(2);
        func_tst_ = TEST_GET_PARAM(3);
        block_idx_ = TEST_GET_PARAM(4);
        func_ref_ = svt_av1_highbd_convolve_2d_sr_c;
    }
    virtual ~AV1HbdSrConvolveTest() {
    }
};

TEST_P(AV1HbdSrConvolveTest, MatchTest) {
    run_test();
}

TEST_P(AV1HbdSrConvolveTest, DISABLED_SpeedTest) {
    speed_test();
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SSSE3, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_convolve_2d_sr_ssse3));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_SSSE3, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_convolve_x_sr_ssse3));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_SSSE3, AV1HbdSrConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_convolve_y_sr_ssse3));
INSTANTIATE_TEST_SUITE_P(
    ConvolveTestCOPY_SSSE3, AV1HbdSrConvolveTest,
    BuildParamsHbd(0, 0, svt_av1_highbd_convolve_2d_copy_sr_ssse3));

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_AVX2, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_convolve_2d_sr_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_AVX2, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_convolve_x_sr_avx2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_AVX2, AV1HbdSrConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_convolve_y_sr_avx2));
INSTANTIATE_TEST_SUITE_P(
    ConvolveTestCOPY_AVX2, AV1HbdSrConvolveTest,
    BuildParamsHbd(0, 0, svt_av1_highbd_convolve_2d_copy_sr_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_NEON, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_convolve_2d_sr_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_NEON, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_convolve_x_sr_neon));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_NEON, AV1HbdSrConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_convolve_y_sr_neon));
INSTANTIATE_TEST_SUITE_P(
    ConvolveTestCOPY_NEON, AV1HbdSrConvolveTest,
    BuildParamsHbd(0, 0, svt_av1_highbd_convolve_2d_copy_sr_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(ConvolveTestX_SVE, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 0,
                                        svt_av1_highbd_convolve_x_sr_sve));
#endif  // HAVE_SVE

#if HAVE_SVE2
INSTANTIATE_TEST_SUITE_P(ConvolveTest2D_SVE2, AV1HbdSrConvolveTest,
                         BuildParamsHbd(1, 1,
                                        svt_av1_highbd_convolve_2d_sr_sve2));
INSTANTIATE_TEST_SUITE_P(ConvolveTestY_SVE2, AV1HbdSrConvolveTest,
                         BuildParamsHbd(0, 1,
                                        svt_av1_highbd_convolve_y_sr_sve2));
#endif  // HAVE_SVE2
#endif  // ARCH_AARCH64

}  // namespace
