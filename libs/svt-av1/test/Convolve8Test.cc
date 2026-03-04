/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */
#include <stdlib.h>

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "filter.h"
#include "inter_prediction.h"
#include "random.h"
#include "svt_time.h"
#include "util.h"
#include "utility.h"

using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

namespace {

// For w<=4, MULTITAP_SHARP is the same as EIGHTTAP_REGULAR
static const InterpFilterParams av1_interp_4tap[SWITCHABLE_FILTERS + 1] = {
    {(const int16_t *)sub_pel_filters_4,
     SUBPEL_TAPS,
     SUBPEL_SHIFTS,
     EIGHTTAP_REGULAR},
    {(const int16_t *)sub_pel_filters_4smooth,
     SUBPEL_TAPS,
     SUBPEL_SHIFTS,
     EIGHTTAP_SMOOTH},
    {(const int16_t *)sub_pel_filters_4,
     SUBPEL_TAPS,
     SUBPEL_SHIFTS,
     EIGHTTAP_REGULAR},
    {(const int16_t *)bilinear_filters, SUBPEL_TAPS, SUBPEL_SHIFTS, BILINEAR},
};

static const InterpFilterParams
    av1_interp_filter_params_list[SWITCHABLE_FILTERS + 1] = {
        {(const int16_t *)sub_pel_filters_8,
         SUBPEL_TAPS,
         SUBPEL_SHIFTS,
         EIGHTTAP_REGULAR},
        {(const int16_t *)sub_pel_filters_8smooth,
         SUBPEL_TAPS,
         SUBPEL_SHIFTS,
         EIGHTTAP_SMOOTH},
        {(const int16_t *)sub_pel_filters_8sharp,
         SUBPEL_TAPS,
         SUBPEL_SHIFTS,
         MULTITAP_SHARP},
        {(const int16_t *)bilinear_filters,
         SUBPEL_TAPS,
         SUBPEL_SHIFTS,
         BILINEAR}};

static INLINE const InterpFilterParams *get_4tap_interp_filter_params(
    const InterpFilter interp_filter) {
    return &av1_interp_4tap[interp_filter];
}

static INLINE const InterpFilterParams *av1_get_filter(int subpel_search) {
    assert(subpel_search >= USE_2_TAPS);

    switch (subpel_search) {
    case USE_2_TAPS: return get_4tap_interp_filter_params(BILINEAR);
    case USE_4_TAPS: return get_4tap_interp_filter_params(EIGHTTAP_REGULAR);
    case USE_8_TAPS: return &av1_interp_filter_params_list[EIGHTTAP_REGULAR];
    default: assert(0); return NULL;
    }
}

static const unsigned int kMaxDimension = MAX_SB_SIZE;
static const int kDataAlignment = 16;
static const int kOuterBlockSize = 4 * kMaxDimension;
static const int kInputStride = kOuterBlockSize;
static const int kOutputStride = kOuterBlockSize;
static const int kInputBufferSize = kOuterBlockSize * kOuterBlockSize;
static const int kOutputBufferSize = kOuterBlockSize * kOuterBlockSize;
static const int16_t kInvalidFilter[8] = {};
static const int kNumFilters = 16;

typedef void (*ConvolveFunc)(const uint8_t *src, ptrdiff_t src_stride,
                             uint8_t *dst, ptrdiff_t dst_stride,
                             const int16_t *filter_x, int filter_x_stride,
                             const int16_t *filter_y, int filter_y_stride,
                             int w, int h);

struct ConvolveFunctions {
    ConvolveFunctions(ConvolveFunc h8, ConvolveFunc v8) : h8_(h8), v8_(v8) {
    }

    ConvolveFunc h8_;
    ConvolveFunc v8_;
};

typedef std::tuple<int, int, const ConvolveFunctions *> ConvolveParam;

#define ALL_SIZES(convolve_fn)                                               \
    make_tuple(4, 4, &convolve_fn), make_tuple(8, 4, &convolve_fn),          \
        make_tuple(4, 8, &convolve_fn), make_tuple(8, 8, &convolve_fn),      \
        make_tuple(16, 8, &convolve_fn), make_tuple(8, 16, &convolve_fn),    \
        make_tuple(16, 16, &convolve_fn), make_tuple(32, 16, &convolve_fn),  \
        make_tuple(16, 32, &convolve_fn), make_tuple(32, 32, &convolve_fn),  \
        make_tuple(64, 32, &convolve_fn), make_tuple(32, 64, &convolve_fn),  \
        make_tuple(64, 64, &convolve_fn), make_tuple(128, 64, &convolve_fn), \
        make_tuple(64, 128, &convolve_fn), make_tuple(128, 128, &convolve_fn)

// Reference 8-tap subpixel filter, slightly modified to fit into this test.
#define AV1_FILTER_WEIGHT 128
#define AV1_FILTER_SHIFT 7
uint8_t clip_pixel(int x) {
    return x < 0 ? 0 : x > 255 ? 255 : x;
}

void filter_block2d_8_c(const uint8_t *src_ptr, unsigned int src_stride,
                        const int16_t *HFilter, const int16_t *VFilter,
                        uint8_t *dst_ptr, unsigned int dst_stride,
                        unsigned int output_width, unsigned int output_height) {
    // Between passes, we use an intermediate buffer whose height is extended to
    // have enough horizontally filtered values as input for the vertical pass.
    // This buffer is allocated to be big enough for the largest block type we
    // support.
    const int kInterp_Extend = 4;
    const unsigned int intermediate_height =
        (kInterp_Extend - 1) + output_height + kInterp_Extend;
    unsigned int i, j;

    assert(intermediate_height > 7);

    // Size of intermediate_buffer is max_intermediate_height *
    // filter_max_width, where max_intermediate_height = (kInterp_Extend - 1) +
    // filter_max_height
    //                                 + kInterp_Extend
    //                               = 3 + 16 + 4
    //                               = 23
    // and filter_max_width          = 16
    //
    uint8_t intermediate_buffer[(kMaxDimension + 8) * kMaxDimension];
    const int intermediate_next_stride =
        1 - static_cast<int>(intermediate_height * output_width);

    // Horizontal pass (src -> transposed intermediate).
    uint8_t *output_ptr = intermediate_buffer;
    const int src_next_row_stride = src_stride - output_width;
    src_ptr -= (kInterp_Extend - 1) * src_stride + (kInterp_Extend - 1);
    for (i = 0; i < intermediate_height; ++i) {
        for (j = 0; j < output_width; ++j) {
            // Apply filter...
            const int temp =
                (src_ptr[0] * HFilter[0]) + (src_ptr[1] * HFilter[1]) +
                (src_ptr[2] * HFilter[2]) + (src_ptr[3] * HFilter[3]) +
                (src_ptr[4] * HFilter[4]) + (src_ptr[5] * HFilter[5]) +
                (src_ptr[6] * HFilter[6]) + (src_ptr[7] * HFilter[7]) +
                (AV1_FILTER_WEIGHT >> 1);  // Rounding

            // Normalize back to 0-255...
            *output_ptr = clip_pixel(temp >> AV1_FILTER_SHIFT);
            ++src_ptr;
            output_ptr += intermediate_height;
        }
        src_ptr += src_next_row_stride;
        output_ptr += intermediate_next_stride;
    }

    // Vertical pass (transposed intermediate -> dst).
    src_ptr = intermediate_buffer;
    const int dst_next_row_stride = dst_stride - output_width;
    for (i = 0; i < output_height; ++i) {
        for (j = 0; j < output_width; ++j) {
            // Apply filter...
            const int temp =
                (src_ptr[0] * VFilter[0]) + (src_ptr[1] * VFilter[1]) +
                (src_ptr[2] * VFilter[2]) + (src_ptr[3] * VFilter[3]) +
                (src_ptr[4] * VFilter[4]) + (src_ptr[5] * VFilter[5]) +
                (src_ptr[6] * VFilter[6]) + (src_ptr[7] * VFilter[7]) +
                (AV1_FILTER_WEIGHT >> 1);  // Rounding

            // Normalize back to 0-255...
            *dst_ptr++ = clip_pixel(temp >> AV1_FILTER_SHIFT);
            src_ptr += intermediate_height;
        }
        src_ptr += intermediate_next_stride;
        dst_ptr += dst_next_row_stride;
    }
}

class Convolve8Test : public ::testing::TestWithParam<ConvolveParam> {
  public:
    Convolve8Test()
        : width_(TEST_GET_PARAM(0)),
          height_(TEST_GET_PARAM(1)),
          test_funcs_(TEST_GET_PARAM(2)) {
    }

    /* cppcheck-suppress duplInheritedMember */
    static void SetUpTestSuite() {
        // Force input_ to be unaligned, output to be 16 byte aligned.
        input_ = reinterpret_cast<uint8_t *>(
                     svt_aom_memalign(kDataAlignment, kInputBufferSize + 1)) +
                 1;
        ASSERT_NE(input_, nullptr);
        ref_ = reinterpret_cast<uint8_t *>(
            svt_aom_memalign(kDataAlignment, kOutputStride * kMaxDimension));
        ASSERT_NE(ref_, nullptr);
        output_ = reinterpret_cast<uint8_t *>(
            svt_aom_memalign(kDataAlignment, kOutputBufferSize));
        ASSERT_NE(output_, nullptr);
        output_ref_ = reinterpret_cast<uint8_t *>(
            svt_aom_memalign(kDataAlignment, kOutputBufferSize));
        ASSERT_NE(output_ref_, nullptr);
    }

    /* cppcheck-suppress duplInheritedMember */
    static void TearDownTestSuite() {
        svt_aom_free(input_ - 1);
        input_ = nullptr;
        svt_aom_free(ref_);
        ref_ = nullptr;
        svt_aom_free(output_);
        output_ = nullptr;
        svt_aom_free(output_ref_);
        output_ref_ = nullptr;
    }

  protected:
    int BorderLeft() const {
        const int center = (kOuterBlockSize - width_) / 2;
        return (center + (kDataAlignment - 1)) & ~(kDataAlignment - 1);
    }

    int BorderTop() const {
        return (kOuterBlockSize - height_) / 2;
    }

    bool IsIndexInBorder(int i) {
        return (i < BorderTop() * kOuterBlockSize ||
                i >= (BorderTop() + height_) * kOuterBlockSize ||
                i % kOuterBlockSize < BorderLeft() ||
                i % kOuterBlockSize >= (BorderLeft() + width_));
    }

    void SetUp() override {
        mask_ = 255;
        /* Set up guard blocks for an inner block centered in the outer block */
        for (int i = 0; i < kOutputBufferSize; ++i) {
            if (IsIndexInBorder(i)) {
                output_[i] = 255;
            } else {
                output_[i] = 0;
            }
        }

        SVTRandom rnd(0, 255);
        for (int i = 0; i < kInputBufferSize; ++i) {
            if (i & 1) {
                input_[i] = 255;
            } else {
                input_[i] = rnd.Rand8();
            }
        }
    }

    void CheckGuardBlocks() {
        for (int i = 0; i < kOutputBufferSize; ++i) {
            if (IsIndexInBorder(i)) {
                EXPECT_EQ(255, output_[i]);
            }
        }
    }

    uint8_t *input() const {
        const int offset = BorderTop() * kOuterBlockSize + BorderLeft();
        return input_ + offset;
    }

    uint8_t *output() const {
        const int offset = BorderTop() * kOuterBlockSize + BorderLeft();
        return output_ + offset;
    }

    uint8_t *output_ref() const {
        const int offset = BorderTop() * kOuterBlockSize + BorderLeft();
        return output_ref_ + offset;
    }

    void MatchesReferenceSubpixelFilter() {
        uint8_t *const in = input();
        uint8_t *const out = output();
        int subpel_search;
        for (subpel_search = USE_2_TAPS; subpel_search <= USE_8_TAPS;
             ++subpel_search) {
            const InterpFilterParams *filter_params =
                av1_get_filter(subpel_search);
            const InterpKernel *filters =
                (const InterpKernel *)filter_params->filter_ptr;
            for (int filter_x = 0; filter_x < kNumFilters; ++filter_x) {
                for (int filter_y = 0; filter_y < kNumFilters; ++filter_y) {
                    filter_block2d_8_c(in,
                                       kInputStride,
                                       filters[filter_x],
                                       filters[filter_y],
                                       ref_,
                                       kOutputStride,
                                       width_,
                                       height_);

                    if (filter_x && filter_y)
                        continue;
                    else if (filter_y)
                        test_funcs_->v8_(in,
                                         kInputStride,
                                         out,
                                         kOutputStride,
                                         kInvalidFilter,
                                         16,
                                         filters[filter_y],
                                         16,
                                         width_,
                                         height_);
                    else if (filter_x)
                        test_funcs_->h8_(in,
                                         kInputStride,
                                         out,
                                         kOutputStride,
                                         filters[filter_x],
                                         16,
                                         kInvalidFilter,
                                         16,
                                         width_,
                                         height_);
                    else
                        continue;

                    CheckGuardBlocks();

                    for (int y = 0; y < height_; ++y)
                        for (int x = 0; x < width_; ++x)
                            ASSERT_EQ(ref_[y * kOutputStride + x],
                                      out[y * kOutputStride + x])
                                << "mismatch at (" << x << "," << y << "), "
                                << "subpel_search (" << subpel_search << ","
                                << filter_x << "," << filter_y << ")";
                }
            }
        }
    }

    void FilterExtremes() {
        uint8_t *const in = input();
        uint8_t *const out = output();
        SVTRandom rnd(0, 255);
        for (int y = 0; y < height_; ++y) {
            for (int x = 0; x < width_; ++x) {
                uint8_t r = rnd.Rand8();
                out[y * kOutputStride + x] = r;
                ref_[y * kOutputStride + x] = r;
            }
        }

        for (int axis = 0; axis < 2; axis++) {
            int seed_val = 0;
            while (seed_val < 256) {
                for (int y = 0; y < 8; ++y) {
                    for (int x = 0; x < 8; ++x) {
                        in[y * kOutputStride + x - SUBPEL_TAPS / 2 + 1] =
                            ((seed_val >> (axis ? y : x)) & 1) * mask_;
                        if (axis)
                            seed_val++;
                    }
                    if (axis)
                        seed_val -= 8;
                    else
                        seed_val++;
                }
                if (axis)
                    seed_val += 8;
                int subpel_search;
                for (subpel_search = USE_2_TAPS; subpel_search <= USE_8_TAPS;
                     ++subpel_search) {
                    const InterpFilterParams *filter_params =
                        av1_get_filter(subpel_search);
                    const InterpKernel *filters =
                        (const InterpKernel *)filter_params->filter_ptr;
                    for (int filter_x = 0; filter_x < kNumFilters; ++filter_x) {
                        for (int filter_y = 0; filter_y < kNumFilters;
                             ++filter_y) {
                            filter_block2d_8_c(in,
                                               kInputStride,
                                               filters[filter_x],
                                               filters[filter_y],
                                               ref_,
                                               kOutputStride,
                                               width_,
                                               height_);
                            if (filter_x && filter_y)
                                continue;
                            else if (filter_y)
                                test_funcs_->v8_(in,
                                                 kInputStride,
                                                 out,
                                                 kOutputStride,
                                                 kInvalidFilter,
                                                 16,
                                                 filters[filter_y],
                                                 16,
                                                 width_,
                                                 height_);
                            else if (filter_x)
                                test_funcs_->h8_(in,
                                                 kInputStride,
                                                 out,
                                                 kOutputStride,
                                                 filters[filter_x],
                                                 16,
                                                 kInvalidFilter,
                                                 16,
                                                 width_,
                                                 height_);
                            else
                                continue;

                            for (int y = 0; y < height_; ++y)
                                for (int x = 0; x < width_; ++x)
                                    ASSERT_EQ(ref_[y * kOutputStride + x],
                                              out[y * kOutputStride + x])
                                        << "mismatch at (" << x << "," << y
                                        << "), "
                                        << "subpel_search (" << subpel_search
                                        << "," << filter_x << "," << filter_y
                                        << ")";
                        }
                    }
                }
            }
        }
    }

    void SpeedTest() {
        uint8_t *const in = input();
        uint8_t *const out = output();
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        // Populate ref and out with some random data
        SVTRandom rnd(0, 255);
        for (int y = 0; y < height_; ++y) {
            for (int x = 0; x < width_; ++x) {
                uint8_t r = rnd.Rand8();
                out[y * kOutputStride + x] = r;
                ref_[y * kOutputStride + x] = r;
            }
        }

        int tests_num = 1000;

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        while (tests_num > 0) {
            const InterpFilterParams *filter_params =
                av1_get_filter(USE_8_TAPS);
            const InterpKernel *filters =
                (const InterpKernel *)filter_params->filter_ptr;
            for (int filter_x = 0; filter_x < kNumFilters; ++filter_x) {
                for (int filter_y = 0; filter_y < kNumFilters; ++filter_y) {
                    if (filter_x && filter_y)
                        continue;
                    if (filter_y)
                        test_funcs_->v8_(in,
                                         kInputStride,
                                         out,
                                         kOutputStride,
                                         kInvalidFilter,
                                         16,
                                         filters[filter_y],
                                         16,
                                         width_,
                                         height_);
                    else if (filter_x)
                        test_funcs_->h8_(in,
                                         kInputStride,
                                         out,
                                         kOutputStride,
                                         filters[filter_x],
                                         16,
                                         kInvalidFilter,
                                         16,
                                         width_,
                                         height_);
                }
            }
            tests_num--;
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        double elapsed_time =
            svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                    start_time_useconds,
                                                    finish_time_seconds,
                                                    finish_time_useconds);

        printf("%dx%d time: %6.2f ms\n", width_, height_, elapsed_time);
    }

    const int width_;
    const int height_;
    const ConvolveFunctions *test_funcs_;
    static uint8_t *input_;
    static uint8_t *ref_;
    static uint8_t *output_;
    static uint8_t *output_ref_;
    int mask_;
};

uint8_t *Convolve8Test::input_ = nullptr;
uint8_t *Convolve8Test::ref_ = nullptr;
uint8_t *Convolve8Test::output_ = nullptr;
uint8_t *Convolve8Test::output_ref_ = nullptr;

TEST_P(Convolve8Test, GuardBlocks) {
    CheckGuardBlocks();
}

TEST_P(Convolve8Test, MatchesReferenceSubpixelFilter) {
    MatchesReferenceSubpixelFilter();
}

TEST_P(Convolve8Test, FilterExtremes) {
    FilterExtremes();
}

TEST_P(Convolve8Test, DISABLED_Speed) {
    SpeedTest();
}

#ifdef ARCH_X86_64
const ConvolveFunctions convolve8_ssse3(svt_aom_convolve8_horiz_ssse3,
                                        svt_aom_convolve8_vert_ssse3);
const ConvolveParam kArrayConvolve_ssse3[] = {ALL_SIZES(convolve8_ssse3)};

INSTANTIATE_TEST_SUITE_P(SSSE3, Convolve8Test,
                         ::testing::ValuesIn(kArrayConvolve_ssse3));

const ConvolveFunctions convolve8_avx2(svt_aom_convolve8_horiz_avx2,
                                       svt_aom_convolve8_vert_avx2);
const ConvolveParam kArrayConvolve_avx2[] = {ALL_SIZES(convolve8_avx2)};

INSTANTIATE_TEST_SUITE_P(AVX2, Convolve8Test,
                         ::testing::ValuesIn(kArrayConvolve_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
const ConvolveFunctions convolve8_neon(svt_aom_convolve8_horiz_neon,
                                       svt_aom_convolve8_vert_neon);
const ConvolveParam kArrayConvolve_neon[] = {ALL_SIZES(convolve8_neon)};

INSTANTIATE_TEST_SUITE_P(NEON, Convolve8Test,
                         ::testing::ValuesIn(kArrayConvolve_neon));

#if HAVE_NEON_DOTPROD
const ConvolveFunctions convolve8_neon_dotprod(
    svt_aom_convolve8_horiz_neon_dotprod, svt_aom_convolve8_vert_neon_dotprod);
const ConvolveParam kArrayConvolve_neon_dotprod[] = {
    ALL_SIZES(convolve8_neon_dotprod)};

INSTANTIATE_TEST_SUITE_P(NEON_DOTPROD, Convolve8Test,
                         ::testing::ValuesIn(kArrayConvolve_neon_dotprod));
#endif  // HAVE_NEON_DOTPROD

#if HAVE_NEON_I8MM
const ConvolveFunctions convolve8_neon_i8mm(svt_aom_convolve8_horiz_neon_i8mm,
                                            svt_aom_convolve8_vert_neon_i8mm);
const ConvolveParam kArrayConvolve_neon_i8mm[] = {
    ALL_SIZES(convolve8_neon_i8mm)};

INSTANTIATE_TEST_SUITE_P(NEON_I8MM, Convolve8Test,
                         ::testing::ValuesIn(kArrayConvolve_neon_i8mm));
#endif  // HAVE_NEON_I8MM
#endif  // ARCH_AARCH64

}  // namespace
