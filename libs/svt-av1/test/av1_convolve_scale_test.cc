/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <tuple>
#include <vector>

#include "acm_random.h"
#include "aom_dsp_rtcd.h"
#include "convolve.h"
#include "definitions.h"
#include "filter.h"
#include "gtest/gtest.h"
#include "inter_prediction.h"
#include "svt_time.h"
#include "svt_time.h"
#include "util.h"
#include "utility.h"
#if defined(_MSC_VER)
#pragma warning(suppress : 4324)
#endif

namespace {
const int kTestIters = 10;
const int kPerfIters = 1000;

const int kVPad = 32;
const int kHPad = 32;
const int kXStepQn = 16;
const int kYStepQn = 20;

const int kNumFilterBanks = SWITCHABLE_FILTERS;

using libaom_test::ACMRandom;
using std::make_tuple;
using std::tuple;

// As defined in Source/Lib/Codec/inter_prediction.c.
static const int quant_dist_lookup_table[4][2] = {
    {9, 7},
    {11, 5},
    {12, 4},
    {13, 3},
};

template <typename SrcPixel>
class TestImage {
  public:
    TestImage(int w, int h, int bd) : w_(w), h_(h), bd_(bd) {
        assert(bd < 16);
        assert(bd <= 8 * static_cast<int>(sizeof(SrcPixel)));

        // Pad width by 2*kHPad and then round up to the next multiple of 16
        // to get src_stride_. Add another 16 for dst_stride_ (to make sure
        // something goes wrong if we use the wrong one)
        src_stride_ = (w_ + 2 * kHPad + 15) & ~15;
        dst_stride_ = src_stride_ + 16;

        // Allocate image data
        src_data_.resize(2 * src_block_size());
        dst_data_.resize(2 * dst_block_size());
        dst_16_data_.resize(2 * dst_block_size());
    }

    void Initialize(ACMRandom *rnd);
    void Check() const;

    int src_stride() const {
        return src_stride_;
    }
    int dst_stride() const {
        return dst_stride_;
    }

    int src_block_size() const {
        return (h_ + 2 * kVPad) * src_stride();
    }
    int dst_block_size() const {
        return (h_ + 2 * kVPad) * dst_stride();
    }

    const SrcPixel *GetSrcData(bool ref, bool borders) const {
        const SrcPixel *block = &src_data_[ref ? 0 : src_block_size()];
        return borders ? block : block + kHPad + src_stride_ * kVPad;
    }

    SrcPixel *GetDstData(bool ref, bool borders) {
        SrcPixel *block = &dst_data_[ref ? 0 : dst_block_size()];
        return borders ? block : block + kHPad + dst_stride_ * kVPad;
    }

    CONV_BUF_TYPE *GetDst16Data(bool ref, bool borders) {
        CONV_BUF_TYPE *block = &dst_16_data_[ref ? 0 : dst_block_size()];
        return borders ? block : block + kHPad + dst_stride_ * kVPad;
    }

  private:
    int w_, h_, bd_;
    int src_stride_, dst_stride_;

    std::vector<SrcPixel> src_data_;
    std::vector<SrcPixel> dst_data_;
    std::vector<CONV_BUF_TYPE> dst_16_data_;
};

template <typename Pixel>
void FillEdge(ACMRandom *rnd, int num_pixels, int bd, bool trash, Pixel *data) {
    if (!trash) {
        memset(data, 0, sizeof(*data) * num_pixels);
        return;
    }
    const Pixel mask = (1 << bd) - 1;
    for (int i = 0; i < num_pixels; ++i)
        data[i] = rnd->Rand16() & mask;
}

template <typename Pixel>
void PrepBuffers(ACMRandom *rnd, int w, int h, int stride, int bd,
                 bool trash_edges, Pixel *data) {
    assert(rnd);
    const Pixel mask = (1 << bd) - 1;

    // Fill in the first buffer with random data
    // Top border
    FillEdge(rnd, stride * kVPad, bd, trash_edges, data);
    for (int r = 0; r < h; ++r) {
        Pixel *row_data = data + (kVPad + r) * stride;
        // Left border, contents, right border
        FillEdge(rnd, kHPad, bd, trash_edges, row_data);
        for (int c = 0; c < w; ++c)
            row_data[kHPad + c] = rnd->Rand16() & mask;
        FillEdge(rnd, kHPad, bd, trash_edges, row_data + kHPad + w);
    }
    // Bottom border
    FillEdge(rnd, stride * kVPad, bd, trash_edges, data + stride * (kVPad + h));

    const int bpp = sizeof(*data);
    const int block_elts = stride * (h + 2 * kVPad);
    const int block_size = bpp * block_elts;

    // Now copy that to the second buffer
    memcpy(data + block_elts, data, block_size);
}

template <typename SrcPixel>
void TestImage<SrcPixel>::Initialize(ACMRandom *rnd) {
    PrepBuffers(rnd, w_, h_, src_stride_, bd_, false, &src_data_[0]);
    PrepBuffers(rnd, w_, h_, dst_stride_, bd_, true, &dst_data_[0]);
    PrepBuffers(rnd, w_, h_, dst_stride_, bd_, true, &dst_16_data_[0]);
}

template <typename SrcPixel>
void TestImage<SrcPixel>::Check() const {
    // If memcmp returns 0, there's nothing to do.
    const int num_pixels = dst_block_size();
    const SrcPixel *ref_dst = &dst_data_[0];
    const SrcPixel *tst_dst = &dst_data_[num_pixels];

    const CONV_BUF_TYPE *ref_16_dst = &dst_16_data_[0];
    const CONV_BUF_TYPE *tst_16_dst = &dst_16_data_[num_pixels];

    if (0 == memcmp(ref_dst, tst_dst, sizeof(*ref_dst) * num_pixels)) {
        if (0 ==
            memcmp(ref_16_dst, tst_16_dst, sizeof(*ref_16_dst) * num_pixels))
            return;
    }
    // Otherwise, iterate through the buffer looking for differences (including
    // the edges)
    const int stride = dst_stride_;
    for (int r = 0; r < h_ + 2 * kVPad; ++r) {
        for (int c = 0; c < w_ + 2 * kHPad; ++c) {
            const int32_t ref_value = ref_dst[r * stride + c];
            const int32_t tst_value = tst_dst[r * stride + c];

            EXPECT_EQ(tst_value, ref_value)
                << "Error at row: " << (r - kVPad) << ", col: " << (c - kHPad);
        }
    }

    for (int r = 0; r < h_ + 2 * kVPad; ++r) {
        for (int c = 0; c < w_ + 2 * kHPad; ++c) {
            const int32_t ref_value = ref_16_dst[r * stride + c];
            const int32_t tst_value = tst_16_dst[r * stride + c];

            EXPECT_EQ(tst_value, ref_value)
                << "Error in 16 bit buffer "
                << "Error at row: " << (r - kVPad) << ", col: " << (c - kHPad);
        }
    }
}

typedef tuple<int, int> BlockDimension;

template <typename SrcPixel>
class ConvolveScaleTestBase : public ::testing::Test {
  public:
    ConvolveScaleTestBase() : image_(NULL) {
    }
    virtual ~ConvolveScaleTestBase() {
        delete image_;
    }
    virtual void TearDown() {
    }

    // Implemented by subclasses (SetUp depends on the parameters passed
    // in and RunOne depends on the function to be tested. These can't
    // be templated for low/high bit depths because they have different
    // numbers of parameters)
    virtual void SetUp() = 0;
    virtual void RunOne(bool ref) = 0;

  protected:
    void SetParams(const BlockDimension dims, int bd) {
        width_ = std::get<0>(dims);
        height_ = std::get<1>(dims);
        bd_ = bd;

        delete image_;
        image_ = new TestImage<SrcPixel>(width_, height_, bd_);
    }

    std::vector<ConvolveParams> GetConvParams() {
        std::vector<ConvolveParams> convolve_params;

        ConvolveParams param_no_compound =
            get_conv_params_no_round(0, 0, 0, nullptr, 0, 0, bd_);
        convolve_params.push_back(param_no_compound);

        ConvolveParams param_compound_avg =
            get_conv_params_no_round(0, 1, 0, nullptr, 0, 1, bd_);
        param_compound_avg.use_dist_wtd_comp_avg = 0;
        convolve_params.push_back(param_compound_avg);

        ConvolveParams param_compound_avg_dist_wtd = param_compound_avg;
        param_compound_avg_dist_wtd.use_dist_wtd_comp_avg = 1;

        for (int i = 0; i < 2; ++i) {
            for (int j = 0; j < 4; ++j) {
                param_compound_avg_dist_wtd.fwd_offset =
                    quant_dist_lookup_table[j][i];
                param_compound_avg_dist_wtd.bck_offset =
                    quant_dist_lookup_table[j][1 - i];
                convolve_params.push_back(param_compound_avg_dist_wtd);
            }
        }

        return convolve_params;
    }

    void Run() {
        ACMRandom rnd(ACMRandom::DeterministicSeed());
        std::vector<ConvolveParams> conv_params = GetConvParams();

        for (int i = 0; i < kTestIters; ++i) {
            for (int filter_bank_y = 0; filter_bank_y < kNumFilterBanks;
                 ++filter_bank_y) {
                const InterpFilter filter_y =
                    static_cast<InterpFilter>(filter_bank_y);
                filter_y_ = av1_get_interp_filter_params_with_block_size(
                    filter_y, height_);

                for (int filter_bank_x = 0; filter_bank_x < kNumFilterBanks;
                     ++filter_bank_x) {
                    const InterpFilter filter_x =
                        static_cast<InterpFilter>(filter_bank_x);
                    filter_x_ = av1_get_interp_filter_params_with_block_size(
                        filter_x, width_);

                    for (const auto &c : conv_params) {
                        convolve_params_ = c;
                        Prep(&rnd);
                        RunOne(true);
                        RunOne(false);
                        image_->Check();
                    }
                }
            }
        }
    }

    void SpeedTest() {
        ACMRandom rnd(ACMRandom::DeterministicSeed());
        Prep(&rnd);

        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < kPerfIters; ++i)
            RunOne(true);
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        const double ref_time =
            svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                    start_time_useconds,
                                                    finish_time_seconds,
                                                    finish_time_useconds);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (int i = 0; i < kPerfIters; ++i)
            RunOne(false);
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        const double tst_time =
            svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                    start_time_useconds,
                                                    finish_time_seconds,
                                                    finish_time_useconds);
        std::cout << "[          ] C time = " << ref_time / 1000
                  << " ms, SIMD time = " << tst_time / 1000 << " ms\n";

        EXPECT_GT(ref_time, tst_time)
            << "Error: CDEFSpeedTest, SIMD slower than C.\n"
            << "C time: " << ref_time << " us\n"
            << "SIMD time: " << tst_time << " us\n";
    }

    static int RandomSubpel(ACMRandom *rnd) {
        const uint8_t subpel_mode = rnd->Rand8();
        if ((subpel_mode & 7) == 0) {
            return 0;
        } else if ((subpel_mode & 7) == 1) {
            return SCALE_SUBPEL_SHIFTS - 1;
        } else {
            return 1 + rnd->PseudoUniform(SCALE_SUBPEL_SHIFTS - 2);
        }
    }

    void Prep(ACMRandom *rnd) {
        assert(rnd);

        // Choose subpel_x_ and subpel_y_. They should be less than
        // SCALE_SUBPEL_SHIFTS; we also want to add extra weight to
        // "interesting" values: 0 and SCALE_SUBPEL_SHIFTS - 1
        subpel_x_ = RandomSubpel(rnd);
        subpel_y_ = RandomSubpel(rnd);

        image_->Initialize(rnd);
    }

    int width_, height_, bd_;
    int subpel_x_, subpel_y_;
    InterpFilterParams filter_x_, filter_y_;
    TestImage<SrcPixel> *image_;
    ConvolveParams convolve_params_;
};

typedef tuple<int, int> BlockDimension;

typedef void (*LowbdConvolveFunc)(const uint8_t *src, int src_stride,
                                  uint8_t *dst, int dst_stride, int w, int h,
                                  const InterpFilterParams *filter_params_x,
                                  const InterpFilterParams *filter_params_y,
                                  const int subpel_x_qn, const int x_step_qn,
                                  const int subpel_y_qn, const int y_step_qn,
                                  ConvolveParams *conv_params);

// Test parameter list:
//  <tst_fun, dims>
typedef tuple<LowbdConvolveFunc, BlockDimension> LowBDParams;

class LowBDConvolveScaleTest
    : public ConvolveScaleTestBase<uint8_t>,
      public ::testing::WithParamInterface<LowBDParams> {
  public:
    LowBDConvolveScaleTest() = default;
    ~LowBDConvolveScaleTest() = default;

    void SetUp() {
        tst_fun_ = TEST_GET_PARAM(0);

        const BlockDimension &block = TEST_GET_PARAM(1);
        const int bd = 8;

        SetParams(block, bd);
    }

    void RunOne(bool ref) {
        const uint8_t *src = image_->GetSrcData(ref, false);
        uint8_t *dst = image_->GetDstData(ref, false);
        convolve_params_.dst = image_->GetDst16Data(ref, false);
        const int src_stride = image_->src_stride();
        const int dst_stride = image_->dst_stride();
        if (ref) {
            svt_av1_convolve_2d_scale_c(src,
                                        src_stride,
                                        dst,
                                        dst_stride,
                                        width_,
                                        height_,
                                        &filter_x_,
                                        &filter_y_,
                                        subpel_x_,
                                        kXStepQn,
                                        subpel_y_,
                                        kYStepQn,
                                        &convolve_params_);
        } else {
            tst_fun_(src,
                     src_stride,
                     dst,
                     dst_stride,
                     width_,
                     height_,
                     &filter_x_,
                     &filter_y_,
                     subpel_x_,
                     kXStepQn,
                     subpel_y_,
                     kYStepQn,
                     &convolve_params_);
        }
    }

  private:
    LowbdConvolveFunc tst_fun_;
};

const BlockDimension kBlockDim[] = {
    make_tuple(2, 2),
    make_tuple(2, 4),
    make_tuple(4, 4),
    make_tuple(4, 8),
    make_tuple(8, 4),
    make_tuple(8, 8),
    make_tuple(8, 16),
    make_tuple(16, 8),
    make_tuple(16, 16),
    make_tuple(16, 32),
    make_tuple(32, 16),
    make_tuple(32, 32),
    make_tuple(32, 64),
    make_tuple(64, 32),
    make_tuple(64, 64),
    make_tuple(64, 128),
    make_tuple(128, 64),
    make_tuple(128, 128),
};

TEST_P(LowBDConvolveScaleTest, Check) {
    Run();
}
TEST_P(LowBDConvolveScaleTest, DISABLED_Speed) {
    SpeedTest();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, LowBDConvolveScaleTest,
    ::testing::Combine(::testing::Values(svt_av1_convolve_2d_scale_sse4_1),
                       ::testing::ValuesIn(kBlockDim)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LowBDConvolveScaleTest,
    ::testing::Combine(::testing::Values(svt_av1_convolve_2d_scale_neon),
                       ::testing::ValuesIn(kBlockDim)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, LowBDConvolveScaleTest,
    ::testing::Combine(
        ::testing::Values(svt_av1_convolve_2d_scale_neon_dotprod),
        ::testing::ValuesIn(kBlockDim)));
#endif  // HAVE_NEON_DOTPROD

#if HAVE_NEON_I8MM
INSTANTIATE_TEST_SUITE_P(
    NEON_I8MM, LowBDConvolveScaleTest,
    ::testing::Combine(::testing::Values(svt_av1_convolve_2d_scale_neon_i8mm),
                       ::testing::ValuesIn(kBlockDim)));
#endif  // HAVE_NEON_I8MM

#endif  // ARCH_AARCH64

typedef void (*HighbdConvolveFunc)(const uint16_t *src, int src_stride,
                                   uint16_t *dst, int dst_stride, int w, int h,
                                   const InterpFilterParams *filter_params_x,
                                   const InterpFilterParams *filter_params_y,
                                   const int subpel_x_qn, const int x_step_qn,
                                   const int subpel_y_qn, const int y_step_qn,
                                   ConvolveParams *conv_params, int bd);

// Test parameter list:
//  <tst_fun, dims, bd>
typedef tuple<HighbdConvolveFunc, BlockDimension, int> HighBDParams;

class HighBDConvolveScaleTest
    : public ConvolveScaleTestBase<uint16_t>,
      public ::testing::WithParamInterface<HighBDParams> {
  public:
    HighBDConvolveScaleTest() = default;
    virtual ~HighBDConvolveScaleTest() {
    }

    void SetUp() {
        tst_fun_ = TEST_GET_PARAM(0);

        const BlockDimension &block = TEST_GET_PARAM(1);
        const int bd = TEST_GET_PARAM(2);

        SetParams(block, bd);
    }

    void RunOne(bool ref) {
        const uint16_t *src = image_->GetSrcData(ref, false);
        uint16_t *dst = image_->GetDstData(ref, false);
        convolve_params_.dst = image_->GetDst16Data(ref, false);
        const int src_stride = image_->src_stride();
        const int dst_stride = image_->dst_stride();

        if (ref) {
            svt_av1_highbd_convolve_2d_scale_c(src,
                                               src_stride,
                                               dst,
                                               dst_stride,
                                               width_,
                                               height_,
                                               &filter_x_,
                                               &filter_y_,
                                               subpel_x_,
                                               kXStepQn,
                                               subpel_y_,
                                               kYStepQn,
                                               &convolve_params_,
                                               bd_);
        } else {
            tst_fun_(src,
                     src_stride,
                     dst,
                     dst_stride,
                     width_,
                     height_,
                     &filter_x_,
                     &filter_y_,
                     subpel_x_,
                     kXStepQn,
                     subpel_y_,
                     kYStepQn,
                     &convolve_params_,
                     bd_);
        }
    }

  private:
    HighbdConvolveFunc tst_fun_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(HighBDConvolveScaleTest);

TEST_P(HighBDConvolveScaleTest, Check) {
    Run();
}
TEST_P(HighBDConvolveScaleTest, DISABLED_Speed) {
    SpeedTest();
}

const int kBDs[] = {8, 10};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, HighBDConvolveScaleTest,
    ::testing::Combine(
        ::testing::Values(svt_av1_highbd_convolve_2d_scale_sse4_1),
        ::testing::ValuesIn(kBlockDim), ::testing::ValuesIn(kBDs)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HighBDConvolveScaleTest,
    ::testing::Combine(::testing::Values(svt_av1_highbd_convolve_2d_scale_neon),
                       ::testing::ValuesIn(kBlockDim),
                       ::testing::ValuesIn(kBDs)));
#endif  // ARCH_AARCH64
}  // namespace
