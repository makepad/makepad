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
 * @file sad_Test.cc
 *
 * @brief Unit test for SAD functions:
 * - svt_nxm_sad_kernel_func
 * - svt_sad_loop_kernel_func
 * - svt_ext_ext_all_sad_calculation_8x8_16x16_func
 * - svt_ext_ext_eight_sad_calculation_32x32_64x64_func
 * - svt_ext_sad_calculation_8x8_16x16_func
 * - svt_ext_sad_calculation_32x32_64x64_func
 * - svt_initialize_buffer_32bits_func
 * - svt_aom_sad_16bit_kernel_func
 * - svt_pme_sad_loop_kernel_func
 *
 * @author Cidana-Ryan, Cidana-Wenyao, Cidana-Ivy
 *
 ******************************************************************************/
#include <array>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <limits.h>
#include <new>

#include "gtest/gtest.h"
#include "aom_dsp_rtcd.h"
#include "compute_sad.h"
#include "me_sad_calculation.h"
#include "motion_estimation.h"
#include "me_context.h"
#include "svt_time.h"
#include "random.h"
#include "util.h"

#include "mcomp.h"

using svt_av1_test_tool::SVTRandom;  // to generate the random

namespace {
/**
 * @Brief Test param definition.
 * - TEST_BLOCK_SIZES:All of sad_ calculation funcs in test cover block size
 *  Width {4, 8, 16, 24, 32, 48, 64} x height{ 4, 8, 16, 24, 32, 48, 64).
 */
#define MAX_BLOCK_SIZE (MAX_SB_SIZE * MAX_SB_SIZE)
#define MAX_REF_BLOCK_SIZE \
    ((MAX_SB_SIZE + PAD_VALUE) * (MAX_SB_SIZE + PAD_VALUE))
typedef std::tuple<int, int> BlkSize;
typedef enum { REF_MAX, SRC_MAX, RANDOM, UNALIGN } TestPattern;
typedef enum { BUF_MAX, BUF_MIN, BUF_SMALL, BUF_RANDOM } SADPattern;
BlkSize TEST_BLOCK_SIZES[] = {
    BlkSize(4, 10),  BlkSize(4, 12),  BlkSize(8, 10),  BlkSize(8, 12),
    BlkSize(24, 10), BlkSize(40, 10), BlkSize(48, 10), BlkSize(56, 10),
    BlkSize(24, 14), BlkSize(40, 14), BlkSize(48, 14), BlkSize(56, 14),
    BlkSize(16, 10), BlkSize(16, 5),  BlkSize(32, 10), BlkSize(32, 20),
    BlkSize(64, 20), BlkSize(64, 64), BlkSize(64, 32), BlkSize(32, 64),
    BlkSize(32, 32), BlkSize(32, 16), BlkSize(16, 32), BlkSize(16, 16),
    BlkSize(16, 8),  BlkSize(8, 16),  BlkSize(8, 8),   BlkSize(8, 4),
    BlkSize(4, 4),   BlkSize(4, 8),   BlkSize(4, 16),  BlkSize(16, 4),
    BlkSize(8, 32),  BlkSize(32, 8),  BlkSize(16, 64), BlkSize(64, 16),
    BlkSize(24, 24), BlkSize(24, 16), BlkSize(16, 24), BlkSize(24, 8),
    BlkSize(8, 24),  BlkSize(64, 24), BlkSize(48, 24), BlkSize(32, 24),
    BlkSize(24, 32), BlkSize(48, 48), BlkSize(48, 16), BlkSize(48, 32),
    BlkSize(16, 48), BlkSize(32, 48), BlkSize(48, 64), BlkSize(64, 48),
    BlkSize(56, 32), BlkSize(40, 32)};

BlkSize TEST_BLOCK_SIZES_SMALL[] = {
    BlkSize(6, 2),   BlkSize(6, 4),   BlkSize(6, 8),   BlkSize(6, 16),
    BlkSize(6, 32),  BlkSize(12, 2),  BlkSize(12, 4),  BlkSize(12, 8),
    BlkSize(12, 16), BlkSize(12, 32), BlkSize(31, 1),  BlkSize(31, 2),
    BlkSize(31, 3),  BlkSize(15, 10), BlkSize(6, 10),  BlkSize(7, 10),
    BlkSize(5, 11),  BlkSize(6, 11),  BlkSize(4, 11),  BlkSize(8, 11),
    BlkSize(7, 11),  BlkSize(5, 10),  BlkSize(16, 10), BlkSize(17, 10),
    BlkSize(15, 11), BlkSize(16, 11), BlkSize(17, 11), BlkSize(18, 11),
    BlkSize(19, 11), BlkSize(31, 8),  BlkSize(16, 5),  BlkSize(16, 17),
    BlkSize(16, 31), BlkSize(16, 33), BlkSize(12, 5),  BlkSize(12, 17),
    BlkSize(12, 31), BlkSize(12, 33), BlkSize(31, 7),  BlkSize(31, 6),
    BlkSize(31, 5),  BlkSize(31, 4),  BlkSize(39, 1),  BlkSize(3, 40),
    BlkSize(43, 4),  BlkSize(43, 5),  BlkSize(41, 5),  BlkSize(55, 3),
    BlkSize(37, 37), BlkSize(41, 21), BlkSize(51, 21), BlkSize(63, 21),
    BlkSize(63, 27), BlkSize(63, 33), BlkSize(63, 32), BlkSize(4, 2),
    BlkSize(4, 3),   BlkSize(4, 4),   BlkSize(4, 8),   BlkSize(4, 9),
    BlkSize(4, 16),  BlkSize(4, 17),  BlkSize(4, 32),  BlkSize(4, 33),
    BlkSize(6, 3),   BlkSize(6, 9),   BlkSize(6, 17),  BlkSize(6, 33),
    BlkSize(8, 2),   BlkSize(8, 3),   BlkSize(8, 4),   BlkSize(8, 8),
    BlkSize(8, 9),   BlkSize(8, 15),  BlkSize(8, 16),  BlkSize(8, 31),
    BlkSize(8, 32),  BlkSize(12, 3),  BlkSize(12, 9),  BlkSize(12, 15),
    BlkSize(12, 31), BlkSize(16, 2),  BlkSize(16, 3),  BlkSize(16, 4),
    BlkSize(16, 8),  BlkSize(16, 9),  BlkSize(16, 15), BlkSize(16, 16),
    BlkSize(16, 31), BlkSize(16, 32), BlkSize(24, 2),  BlkSize(24, 3),
    BlkSize(24, 4),  BlkSize(24, 8),  BlkSize(24, 9),  BlkSize(24, 15),
    BlkSize(24, 16), BlkSize(24, 31), BlkSize(24, 32), BlkSize(32, 2),
    BlkSize(32, 3),  BlkSize(32, 4),  BlkSize(32, 8),  BlkSize(32, 9),
    BlkSize(32, 15), BlkSize(32, 16), BlkSize(32, 31), BlkSize(32, 32),
    BlkSize(32, 33),
};
TestPattern TEST_PATTERNS[] = {REF_MAX, SRC_MAX, RANDOM, UNALIGN};
SADPattern TEST_SAD_PATTERNS[] = {BUF_MAX, BUF_MIN, BUF_SMALL, BUF_RANDOM};
typedef std::tuple<TestPattern> Testsad_Param;

/**
 * @Brief Base class for SAD test. SADTestBase handle test vector in memory,
 * provide SAD and SAD avg reference function
 */
class SADTestBase : public ::testing::Test {
  public:
    SADTestBase(TestPattern test_pattern) {
        src_stride_ = MAX_SB_SIZE;
        ref1_stride_ = ref2_stride_ = MAX_SB_SIZE;
        test_pattern_ = test_pattern;
        src_aligned_ = nullptr;
        ref1_aligned_ = nullptr;
        ref2_aligned_ = nullptr;
    }

    SADTestBase(TestPattern test_pattern, SADPattern test_sad_pattern) {
        src_stride_ = MAX_SB_SIZE;
        ref1_stride_ = ref2_stride_ = MAX_SB_SIZE;
        test_pattern_ = test_pattern;
        test_sad_pattern_ = test_sad_pattern;
    }

    SADTestBase(TestPattern test_pattern, const int search_area_width,
                const int search_area_height) {
        src_stride_ = MAX_SB_SIZE;
        ref1_stride_ = ref2_stride_ = MAX_SB_SIZE;
        test_pattern_ = test_pattern;
        search_area_width_ = search_area_width;
        search_area_height_ = search_area_height;
    }

    void SetUp() override {
        src_aligned_ = (uint8_t *)svt_aom_memalign(32, MAX_BLOCK_SIZE);
        ref1_aligned_ = (uint8_t *)svt_aom_memalign(32, MAX_REF_BLOCK_SIZE);
        ref2_aligned_ = (uint8_t *)svt_aom_memalign(32, MAX_REF_BLOCK_SIZE);
        ASSERT_NE(src_aligned_, nullptr);
        ASSERT_NE(ref1_aligned_, nullptr);
        ASSERT_NE(ref2_aligned_, nullptr);
    }

    void TearDown() override {
        if (src_aligned_)
            svt_aom_free(src_aligned_);
        if (ref1_aligned_)
            svt_aom_free(ref1_aligned_);
        if (ref2_aligned_)
            svt_aom_free(ref2_aligned_);
    }

    void prepare_data() {
        const int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        switch (test_pattern_) {
        case REF_MAX: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_aligned_[i] = 0;

            for (int i = 0; i < MAX_REF_BLOCK_SIZE; i++)
                ref1_aligned_[i] = ref2_aligned_[i] = mask;

            break;
        }
        case SRC_MAX: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_aligned_[i] = mask;

            for (int i = 0; i < MAX_REF_BLOCK_SIZE; i++)
                ref1_aligned_[i] = ref2_aligned_[i] = 0;

            break;
        }
        case RANDOM: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_aligned_[i] = rnd.random();

            for (int i = 0; i < MAX_REF_BLOCK_SIZE; ++i) {
                ref1_aligned_[i] = rnd.random();
                ref2_aligned_[i] = rnd.random();
            }
            break;
        };
        case UNALIGN: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_aligned_[i] = rnd.random();

            for (int i = 0; i < MAX_REF_BLOCK_SIZE; ++i) {
                ref1_aligned_[i] = rnd.random();
                ref2_aligned_[i] = rnd.random();
            }
            ref1_stride_ = MAX_SB_SIZE - 1;
            ref2_stride_ = MAX_SB_SIZE - 1;
            break;
        }
        default: break;
        }
    }

    void prepare_data(int width, int height) {
        const int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        switch (test_pattern_) {
        case REF_MAX: {
            for (int j = 0; j < height; ++j) {
                for (int i = 0; i < width; ++i) {
                    src_aligned_[j * src_stride_ + i] = 0;
                    ref1_aligned_[j * ref1_stride_ + i] =
                        ref2_aligned_[j * ref2_stride_ + i] = mask;
                }
            }
            break;
        }
        case SRC_MAX: {
            for (int j = 0; j < height; ++j) {
                for (int i = 0; i < width; ++i) {
                    src_aligned_[j * src_stride_ + i] = mask;
                    ref1_aligned_[j * ref1_stride_ + i] =
                        ref2_aligned_[j * ref2_stride_ + i] = mask;
                }
            }
            break;
        }
        case RANDOM: {
            for (int j = 0; j < height; ++j) {
                for (int i = 0; i < width; ++i) {
                    src_aligned_[j * src_stride_ + i] = rnd.random();
                    ref1_aligned_[j * ref1_stride_ + i] = rnd.random();
                    ref2_aligned_[j * ref2_stride_ + i] = rnd.random();
                }
            }
            break;
        };
        case UNALIGN: {
            ref1_stride_ = MAX_SB_SIZE - 1;
            ref2_stride_ = MAX_SB_SIZE - 1;
            for (int j = 0; j < height; ++j) {
                for (int i = 0; i < width; ++i) {
                    src_aligned_[j * src_stride_ + i] = rnd.random();
                    ref1_aligned_[j * ref1_stride_ + i] = rnd.random();
                    ref2_aligned_[j * ref2_stride_ + i] = rnd.random();
                }
            }
            break;
        }
        default: break;
        }
    }

    void fill_buf_with_value(uint32_t *buf, int num, uint32_t value) {
        for (int i = 0; i < num; ++i)
            buf[i] = value;
    }

    void prepare_sad_data_32b() {
        const int32_t mask = (1 << 8) - 1;
        SVTRandom rnd(0, mask);
        switch (test_sad_pattern_) {
        case BUF_MAX: {
            fill_buf_with_value(&sad16x16_32b[0][0], 16 * 8, mask);
            break;
        }
        case BUF_MIN: {
            fill_buf_with_value(&sad16x16_32b[0][0], 16 * 8, 0);
            break;
        }
        case BUF_SMALL: {
            SVTRandom rnd_small(0, 256);
            for (int i = 0; i < 16; i++)
                for (int j = 0; j < 8; j++)
                    sad16x16_32b[i][j] = rnd_small.random();
            break;
        }
        case BUF_RANDOM: {
            for (int i = 0; i < 16; i++)
                for (int j = 0; j < 8; j++)
                    sad16x16_32b[i][j] = rnd.random();
            break;
        }
        default: break;
        }
    }

    virtual void check_sad(int width, int height) = 0;
    virtual void speed_sad(int width, int height) {
        printf("Usage not override a function, %i, %i\n", width, height);
        ASSERT_TRUE(0);
    }

    void test_sad_size(BlkSize size) {
        check_sad(std::get<0>(size), std::get<1>(size));
    }

    void test_sad_sizes(BlkSize *test_block_sizes,
                        size_t test_block_sizes_count) {
        for (uint32_t i = 0; i < test_block_sizes_count; ++i) {
            test_sad_size(test_block_sizes[i]);
        }
    }

    void speed_sad_size(BlkSize size) {
        speed_sad(std::get<0>(size), std::get<1>(size));
    }

    void speed_sad_sizes(BlkSize *test_block_sizes,
                         size_t test_block_sizes_count) {
        for (uint32_t i = 0; i < test_block_sizes_count; ++i) {
            speed_sad_size(test_block_sizes[i]);
        }
    }

  protected:
    int src_stride_;
    int ref1_stride_;
    int ref2_stride_;
    int16_t search_area_height_, search_area_width_;
    TestPattern test_pattern_;
    SADPattern test_sad_pattern_;
    uint8_t *src_aligned_;
    uint8_t *ref1_aligned_;
    uint8_t *ref2_aligned_;
    uint16_t sad16x16_16b[16][8];
    uint32_t sad8x8[64][8];
    // std::array is used here to silence GCC's stringop-overflow warning
    // since it gets confused by the function signature of
    // svt_ext_eight_sad_calculation_32x32_64x64_c
    std::array<uint32_t[8], 16> sad16x16_32b;
    uint32_t sad32x32[4][8];
};

/**
 * @brief Unit test for svt_nxm_sad_kernel_helper:
 *
 * Test strategy:
 *  This test case combines different width{4-64} x height{4-64} and different
 * test pattern(REF_MAX, SRC_MAX, RANDOM, UNALIGN). Check the result by
 * comparing results from reference function and SIMD function.
 *
 *
 * Expect result:
 *  Results from reference function and SIMD function are equal.
 *
 * Test cases:
 *  Width {4, 8, 16, 24, 32, 48, 64} x height{ 4, 8, 16, 24, 32, 48, 64)
 *  Test vector pattern {REF_MAX, SRC_MAX, RANDOM, UNALIGN}
 *
 */
typedef uint32_t (*nxm_sad_kernel_fn_ptr)(const uint8_t *src,
                                          uint32_t src_stride,
                                          const uint8_t *ref,
                                          uint32_t ref_stride, uint32_t height,
                                          uint32_t width);

typedef std::tuple<TestPattern, nxm_sad_kernel_fn_ptr> Testsad_Param_nxm_kernel;

class SADTest : public ::testing::WithParamInterface<Testsad_Param_nxm_kernel>,
                public SADTestBase {
  protected:
    nxm_sad_kernel_fn_ptr test_func_;

  public:
    SADTest() : SADTestBase(TEST_GET_PARAM(0)) {
        test_func_ = TEST_GET_PARAM(1);
    }

  protected:
    void check_sad(int width, int height) {
        uint32_t ref_sad = 0;
        uint32_t test_sad = 0;

        prepare_data(width, height);

        ref_sad = svt_nxm_sad_kernel_helper_c(src_aligned_,
                                              src_stride_,
                                              ref1_aligned_,
                                              ref1_stride_,
                                              height,
                                              width);
        test_sad = test_func_(src_aligned_,
                              src_stride_,
                              ref1_aligned_,
                              ref1_stride_,
                              height,
                              width);
        EXPECT_EQ(ref_sad, test_sad)
            << "Size: " << width << "x" << height << " " << std::endl
            << "compare ref_sad(" << ref_sad << ") and test_sad(" << test_sad
            << ") error";
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(SADTest);

TEST_P(SADTest, SADTest) {
    test_sad_sizes(TEST_BLOCK_SIZES,
                   sizeof(TEST_BLOCK_SIZES) / sizeof(TEST_BLOCK_SIZES[0]));
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, SADTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_nxm_sad_kernel_helper_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, SADTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_nxm_sad_kernel_helper_avx2)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, SADTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_nxm_sad_kernel_helper_avx512)));
#endif

#endif

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, SADTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_nxm_sad_kernel_helper_neon)));
#endif  // ARCH_AARCH64

typedef std::tuple<int16_t, int16_t> SearchArea;
SearchArea TEST_LOOP_AREAS[] = {
    SearchArea(8, 15),    SearchArea(16, 31),   SearchArea(12, 31),
    SearchArea(64, 125),  SearchArea(192, 75),  SearchArea(128, 50),
    SearchArea(64, 25),   SearchArea(240, 200), SearchArea(144, 120),
    SearchArea(96, 80),   SearchArea(48, 40),   SearchArea(240, 120),
    SearchArea(144, 72),  SearchArea(96, 48),   SearchArea(48, 24),
    SearchArea(560, 320), SearchArea(336, 192), SearchArea(224, 128),
    SearchArea(112, 64),  SearchArea(640, 400), SearchArea(384, 240),
    SearchArea(256, 160), SearchArea(128, 80),  SearchArea(480, 120),
    SearchArea(288, 72),  SearchArea(192, 48),  SearchArea(96, 24),
    SearchArea(160, 60),  SearchArea(96, 36),   SearchArea(64, 24),
    SearchArea(32, 12),   SearchArea(15, 6)};

typedef void (*Ebsad_LoopKernelNxMType)(
    uint8_t *src,         // input parameter, source samples Ptr
    uint32_t src_stride,  // input parameter, source stride
    uint8_t *ref,         // input parameter, reference samples Ptr
    uint32_t ref_stride,  // input parameter, reference stride
    uint32_t height,      // input parameter, block height (M)
    uint32_t width,       // input parameter, block width (N)
    uint64_t *best_sad, int16_t *x_search_center, int16_t *y_search_center,
    uint32_t
        src_stride_raw,  // input parameter, source stride (no line skipping)
    uint8_t skip_search_line, int16_t search_area_width,
    int16_t search_area_height);

typedef std::tuple<TestPattern, SearchArea, uint8_t, Ebsad_LoopKernelNxMType>
    sad_LoopTestParam;

/**
 * @brief Unit test for svt_sad_loop_kernel:
 *
 * Test strategy:
 *  This test case combines different width{4-64} x height{4-64} and different
 * test pattern(REF_MAX, SRC_MAX, RANDOM, UNALIGN). Check the result by
 * comparing results from reference function and SIMD function.
 *
 *
 * Expect result:
 *  Results from reference function and SIMD function are equal.
 */
class sad_LoopTest : public ::testing::WithParamInterface<sad_LoopTestParam>,
                     public SADTestBase {
  public:
    sad_LoopTest()
        : SADTestBase(TEST_GET_PARAM(0), std::get<0>(TEST_GET_PARAM(1)),
                      std::get<1>(TEST_GET_PARAM(1))),
          skip_search_line(TEST_GET_PARAM(2)),
          test_func_(TEST_GET_PARAM(3)) {
    }

  protected:
    uint8_t skip_search_line;
    Ebsad_LoopKernelNxMType test_func_;

    void check_sad(int width, int height) {
        prepare_data();

        Ebsad_LoopKernelNxMType func_c_ = svt_sad_loop_kernel_c;

        uint64_t best_sad0 = UINT64_MAX;
        int16_t x_search_center0 = 0;
        int16_t y_search_center0 = 0;
        func_c_(src_aligned_,
                src_stride_,
                ref1_aligned_,
                ref1_stride_,
                height,
                width,
                &best_sad0,
                &x_search_center0,
                &y_search_center0,
                ref1_stride_,
                skip_search_line,
                search_area_width_,
                search_area_height_);

        uint64_t best_sad1 = UINT64_MAX;
        int16_t x_search_center1 = 0;
        int16_t y_search_center1 = 0;
        test_func_(src_aligned_,
                   src_stride_,
                   ref1_aligned_,
                   ref1_stride_,
                   height,
                   width,
                   &best_sad1,
                   &x_search_center1,
                   &y_search_center1,
                   ref1_stride_,
                   skip_search_line,
                   search_area_width_,
                   search_area_height_);

        EXPECT_EQ(best_sad0, best_sad1)
            << "compare best_sad error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(x_search_center0, x_search_center1)
            << "compare x_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(y_search_center0, y_search_center1)
            << "compare y_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
    }

    void speed_sad_loop(int width, int height) {
        const uint64_t num_loop = 100000;
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        prepare_data();

        uint64_t best_sad0 = UINT64_MAX;
        uint64_t best_sad1 = UINT64_MAX;
        int16_t x_search_center0 = 0;
        int16_t x_search_center1 = 0;
        int16_t y_search_center0 = 0;
        int16_t y_search_center1 = 0;

        Ebsad_LoopKernelNxMType func_c_ = svt_sad_loop_kernel_c;

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (uint64_t i = 0; i < num_loop; i++) {
            func_c_(src_aligned_,
                    src_stride_,
                    ref1_aligned_,
                    ref1_stride_,
                    height,
                    width,
                    &best_sad0,
                    &x_search_center0,
                    &y_search_center0,
                    ref1_stride_,
                    skip_search_line,
                    search_area_width_,
                    search_area_height_);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);
        for (uint64_t i = 0; i < num_loop; i++) {
            test_func_(src_aligned_,
                       src_stride_,
                       ref1_aligned_,
                       ref1_stride_,
                       height,
                       width,
                       &best_sad1,
                       &x_search_center1,
                       &y_search_center1,
                       ref1_stride_,
                       skip_search_line,
                       search_area_width_,
                       search_area_height_);
        }
        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        EXPECT_EQ(best_sad0, best_sad1)
            << "compare best_sad error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(x_search_center0, x_search_center1)
            << "compare x_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(y_search_center0, y_search_center1)
            << "compare y_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";

        time_o = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf(
            "    svt_sad_loop_kernel(%dx%d) search "
            "area[%dx%d]: %5.2fx)\n",
            width,
            height,
            search_area_width_,
            search_area_height_,
            time_c / time_o);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(sad_LoopTest);

TEST_P(sad_LoopTest, sad_LoopTest) {
    test_sad_sizes(TEST_BLOCK_SIZES,
                   sizeof(TEST_BLOCK_SIZES) / sizeof(TEST_BLOCK_SIZES[0]));
    test_sad_sizes(
        TEST_BLOCK_SIZES_SMALL,
        sizeof(TEST_BLOCK_SIZES_SMALL) / sizeof(TEST_BLOCK_SIZES_SMALL[0]));
}

TEST_P(sad_LoopTest, DISABLED_sad_LoopSpeedTest) {
    speed_sad_sizes(TEST_BLOCK_SIZES,
                    sizeof(TEST_BLOCK_SIZES) / sizeof(TEST_BLOCK_SIZES[0]));
    speed_sad_sizes(
        TEST_BLOCK_SIZES_SMALL,
        sizeof(TEST_BLOCK_SIZES_SMALL) / sizeof(TEST_BLOCK_SIZES_SMALL[0]));
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_sse4_1_intrin)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_avx2_intrin)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_avx512_intrin)));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_neon)));
#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_neon_dotprod)));
#endif  // HAVE_NEON_DOTPROD

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_sve)));

INSTANTIATE_TEST_SUITE_P(
    NEOVERSE_V2, sad_LoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(0, 1),
                       ::testing::Values(svt_sad_loop_kernel_neoverse_v2)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

/**
 * best_sadmxn in GetEightsad_Test,Allsad_CalculationTest and
 * Extsad_CalculationTest must be less than 0x7FFFFFFF because signed comparison
 * is used in test functions, which is as follow:
 *   - svt_ext_all_sad_calculation_8x8_16x16_avx2
 *   - svt_ext_sad_calculation_8x8_16x16_avx2_intrin
 *   - svt_ext_sad_calculation_32x32_64x64_sse4_intrin
 */
#define BEST_SAD_MAX 0x7FFFFFFF

typedef void (*svt_ext_all_sad_calculation_8x8_16x16_fn)(
    uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride,
    uint32_t mv, uint32_t *p_best_sad_8x8, uint32_t *p_best_sad_16x16,
    uint32_t *p_best_mv8x8, uint32_t *p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8], uint32_t p_eight_sad8x8[64][8],
    bool sub_sad);

typedef std::tuple<TestPattern, SADPattern,
                   svt_ext_all_sad_calculation_8x8_16x16_fn>
    sad8x8_CalTestParam;

/**
 * @brief Unit test for svt_ext_all_sad_calculation_8x8_16x16:
 *
 *
 * Test strategy:
 *  This test use different test pattern {REF_MAX, SRC_MAX, RANDOM, UNALIGN}
 *  to generate test vector,sad pattern {BUF_MAX, BUF_MIN, BUF_RANDOM} to
 *  generate test sad16x16. Check the result by comparing results from reference
 *  function and SIMD function.
 *
 *
 * Expect result:
 *  Results come from reference function and SIMD function are equal.
 *
 **/

class Allsad8x8_CalculationTest
    : public ::testing::WithParamInterface<sad8x8_CalTestParam>,
      public SADTestBase {
  public:
    Allsad8x8_CalculationTest()
        : SADTestBase(TEST_GET_PARAM(0), TEST_GET_PARAM(1)) {
        src_stride_ = ref1_stride_ = MAX_SB_SIZE;
        test_func_ = TEST_GET_PARAM(2);
    }

  protected:
    void check_with_sub_sad(bool sub_sad) {
        // Arbitrary large numbers to check for overflows.
        SVTRandom rnd(0, 1 << 30);
        int iterations = 100;
        for (int i = 0; i < iterations; i++) {
            uint32_t mv = rnd.random();
            uint32_t best_sad8x8[2][64];
            uint32_t best_mv8x8[2][64] = {{0}};
            uint32_t best_sad16x16[2][16];
            uint32_t best_mv16x16[2][16] = {{0}};
            uint32_t eight_sad16x16[2][16][8];
            uint32_t eight_sad8x8[2][64][8];
            fill_buf_with_value(&best_sad8x8[0][0], 2 * 64, BEST_SAD_MAX);
            fill_buf_with_value(&best_sad16x16[0][0], 2 * 16, UINT_MAX);
            fill_buf_with_value(&eight_sad16x16[0][0][0], 2 * 16 * 8, UINT_MAX);
            fill_buf_with_value(&eight_sad8x8[0][0][0], 2 * 64 * 8, UINT_MAX);

            prepare_data();

            svt_ext_all_sad_calculation_8x8_16x16_c(src_aligned_,
                                                    src_stride_,
                                                    ref1_aligned_,
                                                    ref1_stride_,
                                                    mv,
                                                    best_sad8x8[0],
                                                    best_sad16x16[0],
                                                    best_mv8x8[0],
                                                    best_mv16x16[0],
                                                    eight_sad16x16[0],
                                                    eight_sad8x8[0],
                                                    sub_sad);

            test_func_(src_aligned_,
                       src_stride_,
                       ref1_aligned_,
                       ref1_stride_,
                       mv,
                       best_sad8x8[1],
                       best_sad16x16[1],
                       best_mv8x8[1],
                       best_mv16x16[1],
                       eight_sad16x16[1],
                       eight_sad8x8[1],
                       sub_sad);

            EXPECT_EQ(
                0,
                memcmp(best_sad8x8[0], best_sad8x8[1], sizeof(best_sad8x8[0])))
                << "compare best_sad8x8 error sub_sad " << sub_sad;
            EXPECT_EQ(
                0, memcmp(best_mv8x8[0], best_mv8x8[1], sizeof(best_mv8x8[0])))
                << "compare best_mv8x8 error sub_sad " << sub_sad;
            EXPECT_EQ(0,
                      memcmp(best_sad16x16[0],
                             best_sad16x16[1],
                             sizeof(best_sad16x16[0])))
                << "compare best_sad16x16 error sub_sad " << sub_sad;
            EXPECT_EQ(
                0,
                memcmp(
                    best_mv16x16[0], best_mv16x16[1], sizeof(best_mv16x16[0])))
                << "compare best_mv16x16 error sub_sad " << sub_sad;
            EXPECT_EQ(
                0,
                memcmp(
                    eight_sad8x8[0], eight_sad8x8[1], sizeof(eight_sad8x8[0])))
                << "compare eight_sad8x8 error sub_sad " << sub_sad;
            EXPECT_EQ(0,
                      memcmp(eight_sad16x16[0],
                             eight_sad16x16[1],
                             sizeof(eight_sad16x16[0])))
                << "compare eight_sad16x16 error sub_sad " << sub_sad;
        }
    }

    void check_sad() {
        check_with_sub_sad(false);
        check_with_sub_sad(true);
    }

    void check_sad(int width, int height) {
        printf("Usage not override a function, %i, %i\n", width, height);
        ASSERT_TRUE(0);
    }

    svt_ext_all_sad_calculation_8x8_16x16_fn test_func_;
};

TEST_P(Allsad8x8_CalculationTest, check_sad8x8) {
    check_sad();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, Allsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_all_sad_calculation_8x8_16x16_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, Allsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_all_sad_calculation_8x8_16x16_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, Allsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_all_sad_calculation_8x8_16x16_neon)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, Allsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_all_sad_calculation_8x8_16x16_neon_dotprod)));
#endif  // HAVE_NEON_DOTPROD

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, Allsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_all_sad_calculation_8x8_16x16_sve)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

typedef void (*svt_ext_eight_sad_calculation_32x32_64x64_fn)(
    const uint32_t p_sad16x16[16][8], uint32_t *p_best_sad_32x32,
    uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32,
    uint32_t *p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]);

typedef std::tuple<TestPattern, SADPattern,
                   svt_ext_eight_sad_calculation_32x32_64x64_fn>
    sad32x32_CalTestParam;

class Allsad32x32_CalculationTest
    : public ::testing::WithParamInterface<sad32x32_CalTestParam>,
      public SADTestBase {
  public:
    Allsad32x32_CalculationTest()
        : SADTestBase(TEST_GET_PARAM(0), TEST_GET_PARAM(1)) {
        src_stride_ = ref1_stride_ = MAX_SB_SIZE;
        test_func_ = TEST_GET_PARAM(2);
    }

  protected:
    void check_sad() {
        // Arbitrary large numbers to check for overflows.
        SVTRandom rnd = SVTRandom(0, 1 << 30);
        int iterations = 100;
        for (int i = 0; i < iterations; i++) {
            uint32_t mv = rnd.random();
            uint32_t best_sad32x32[2][4];
            uint32_t best_sad64x64[2];
            uint32_t best_mv32x32[2][4] = {{0}};
            uint32_t best_mv64x64[2] = {0};
            uint32_t sad_32x32[2][4][8];
            fill_buf_with_value(&best_sad32x32[0][0], 2 * 4, UINT_MAX);
            fill_buf_with_value(&best_sad64x64[0], 2, UINT_MAX);
            fill_buf_with_value(&sad_32x32[0][0][0], 2 * 4 * 8, UINT_MAX);

            prepare_sad_data_32b();

            svt_ext_eight_sad_calculation_32x32_64x64_c(sad16x16_32b.data(),
                                                        best_sad32x32[0],
                                                        &best_sad64x64[0],
                                                        best_mv32x32[0],
                                                        &best_mv64x64[0],
                                                        mv,
                                                        sad_32x32[0]);

            test_func_(sad16x16_32b.data(),
                       best_sad32x32[1],
                       &best_sad64x64[1],
                       best_mv32x32[1],
                       &best_mv64x64[1],
                       mv,
                       sad_32x32[1]);

            EXPECT_EQ(0,
                      memcmp(best_sad32x32[0],
                             best_sad32x32[1],
                             sizeof(best_sad32x32[0])))
                << "compare best_sad32x32 error";
            EXPECT_EQ(
                0,
                memcmp(
                    best_mv32x32[0], best_mv32x32[1], sizeof(best_mv32x32[0])))
                << "compare best_mv32x32 error";
            EXPECT_EQ(best_sad64x64[0], best_sad64x64[1])
                << "compare best_sad64x64 error";
            EXPECT_EQ(best_mv64x64[0], best_mv64x64[1])
                << "compare best_mv64x64 error";
            EXPECT_EQ(0,
                      memcmp(sad_32x32[0], sad_32x32[1], sizeof(sad_32x32[0])))
                << "compare sad_32x32 error";
        }
    }

    void check_sad(int width, int height) {
        printf("Usage not override a function, %i, %i\n", width, height);
        ASSERT_TRUE(0);
    }

    svt_ext_eight_sad_calculation_32x32_64x64_fn test_func_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(Allsad32x32_CalculationTest);

TEST_P(Allsad32x32_CalculationTest, check_sad32x32) {
    check_sad();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, Allsad32x32_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_eight_sad_calculation_32x32_64x64_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, Allsad32x32_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_eight_sad_calculation_32x32_64x64_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, Allsad32x32_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_eight_sad_calculation_32x32_64x64_neon)));
#endif  // ARCH_AARCH64

/**
 * @brief Unit test for svt_ext_sad_calculation_8x8_16x16.
 *
 * Test strategy:
 *  This test uses different test patterns {REF_MAX, SRC_MAX, RANDOM, UNALIGN}
 *  to generate test vector, sad pattern {BUF_MAX, BUF_MIN, BUF_RANDOM} to
 *  generate test sad16x16. Check the result by comparing results from reference
 *  function and SIMD function.
 *
 *
 * Expect result:
 *  Results come from reference function and SIMD function are equal.
 *
 * Test coverage:
 *
 * Test cases:
 **/

typedef void (*svt_ext_sad_calculation_8x8_16x16_fn)(
    uint8_t *src, uint32_t src_stride, uint8_t *ref, uint32_t ref_stride,
    uint32_t *p_best_sad_8x8, uint32_t *p_best_sad_16x16,
    uint32_t *p_best_mv8x8, uint32_t *p_best_mv16x16, uint32_t mv,
    uint32_t *p_sad16x16, uint32_t *p_sad8x8, bool sub_sad);

typedef std::tuple<TestPattern, SADPattern,
                   svt_ext_sad_calculation_8x8_16x16_fn>
    Extsad8x8_CalTestParam;

class Extsad8x8_CalculationTest
    : public ::testing::WithParamInterface<Extsad8x8_CalTestParam>,
      public SADTestBase {
  public:
    Extsad8x8_CalculationTest()
        : SADTestBase(TEST_GET_PARAM(0), TEST_GET_PARAM(1)) {
        src_stride_ = ref1_stride_ = MAX_SB_SIZE;
        test_func_ = TEST_GET_PARAM(2);
    }

  protected:
    void check_sad(int width, int height) {
        printf("Usage not override a function, %i, %i\n", width, height);
        ASSERT_TRUE(0);
    }

    void check_with_sub_sad(bool sub_sad) {
        // Arbitrary large numbers to check for overflows.
        SVTRandom rnd(0, 1 << 30);
        int iterations = 100;
        for (int i = 0; i < iterations; i++) {
            uint32_t mv = rnd.random();
            uint32_t best_sad8x8[2][4];
            uint32_t best_mv8x8[2][4] = {{0}};
            uint32_t best_sad16x16[2], best_mv16x16[2] = {0};
            uint32_t sad16x16[2];
            uint32_t sad_8x8[2][4];
            fill_buf_with_value(&best_sad8x8[0][0], 2 * 4, BEST_SAD_MAX);
            fill_buf_with_value(&best_sad16x16[0], 2, UINT_MAX);
            fill_buf_with_value(&sad16x16[0], 2, UINT_MAX);
            fill_buf_with_value(&sad_8x8[0][0], 2 * 4, UINT_MAX);

            prepare_data();

            svt_ext_sad_calculation_8x8_16x16_c(src_aligned_,
                                                src_stride_,
                                                ref1_aligned_,
                                                ref1_stride_,
                                                best_sad8x8[0],
                                                &best_sad16x16[0],
                                                best_mv8x8[0],
                                                &best_mv16x16[0],
                                                mv,
                                                &sad16x16[0],
                                                sad_8x8[0],
                                                sub_sad);

            test_func_(src_aligned_,
                       src_stride_,
                       ref1_aligned_,
                       ref1_stride_,
                       best_sad8x8[1],
                       &best_sad16x16[1],
                       best_mv8x8[1],
                       &best_mv16x16[1],
                       mv,
                       &sad16x16[1],
                       sad_8x8[1],
                       sub_sad);

            EXPECT_EQ(
                0,
                memcmp(best_sad8x8[0], best_sad8x8[1], sizeof(best_sad8x8[0])))
                << "compare best_sad8x8 error sub_sad " << sub_sad;
            EXPECT_EQ(
                0, memcmp(best_mv8x8[0], best_mv8x8[1], sizeof(best_mv8x8[0])))
                << "compare best_mv8x8 error sub_sad " << sub_sad;
            EXPECT_EQ(best_sad16x16[0], best_sad16x16[1])
                << "compare best_sad16x16 error sub_sad " << sub_sad;
            EXPECT_EQ(best_mv16x16[0], best_mv16x16[1])
                << "compare best_mv16x16 error sub_sad " << sub_sad;
            EXPECT_EQ(0, memcmp(sad_8x8[0], sad_8x8[1], sizeof(sad_8x8[0])))
                << "compare sad_8x8 error sub_sad " << sub_sad;
            EXPECT_EQ(sad16x16[0], sad16x16[1])
                << "compare sad16x16 error sub_sad " << sub_sad;
        }
    }

    void check_sad8x8() {
        check_with_sub_sad(false);
        check_with_sub_sad(true);
    }

    svt_ext_sad_calculation_8x8_16x16_fn test_func_;
};

TEST_P(Extsad8x8_CalculationTest, check_sad8x8) {
    check_sad8x8();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, Extsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_sad_calculation_8x8_16x16_sse4_1_intrin)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, Extsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_sad_calculation_8x8_16x16_avx2_intrin)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, Extsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_sad_calculation_8x8_16x16_neon)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, Extsad8x8_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_sad_calculation_8x8_16x16_neon_dotprod)));
#endif  // NEON_DOTPROD
#endif  // ARCH_AARCH64

/**
 * @brief Unit test for svt_ext_sad_calculation_32x32_64x64:
 *
 * Test strategy:
 *  This test uses different test patterns {REF_MAX, SRC_MAX, RANDOM, UNALIGN}
 *  to generate test vector, sad pattern {BUF_MAX, BUF_MIN, BUF_RANDOM} to
 *  generate test sad16x16. Check the result by comparing results from reference
 *  function and SIMD function.
 *
 *
 * Expect result:
 *  Results come from reference function and SIMD function are equal.
 **/

typedef void (*svt_ext_sad_calculation_32x32_64x64_fn)(
    uint32_t *p_sad16x16, uint32_t *p_best_sad_32x32,
    uint32_t *p_best_sad_64x64, uint32_t *p_best_mv32x32,
    uint32_t *p_best_mv64x64, uint32_t mv, uint32_t *p_sad32x32);

typedef std::tuple<TestPattern, SADPattern,
                   svt_ext_sad_calculation_32x32_64x64_fn>
    Extsad32x32_CalTestParam;

class Extsad32x32_CalculationTest
    : public ::testing::WithParamInterface<Extsad32x32_CalTestParam>,
      public SADTestBase {
  public:
    Extsad32x32_CalculationTest()
        : SADTestBase(TEST_GET_PARAM(0), TEST_GET_PARAM(1)) {
        src_stride_ = ref1_stride_ = MAX_SB_SIZE;
        test_func_ = TEST_GET_PARAM(2);
    }

  protected:
    void check_sad32x32() {
        // Arbitrary large numbers to check for overflows.
        SVTRandom rnd = SVTRandom(0, 1 << 30);
        int iterations = 100;
        for (int i = 0; i < iterations; i++) {
            uint32_t mv = rnd.random();
            uint32_t best_sad32x32[2][4];
            uint32_t best_mv32x32[2][4] = {{0}};
            uint32_t best_sad64x64[2], best_mv64x64[2] = {0};
            uint32_t sad_32x32[2][4];
            fill_buf_with_value(&best_sad32x32[0][0], 2 * 4, BEST_SAD_MAX);
            fill_buf_with_value(&best_sad64x64[0], 2, UINT_MAX);
            fill_buf_with_value(&sad_32x32[0][0], 2 * 4, UINT_MAX);

            prepare_sad_data_32b();

            svt_ext_sad_calculation_32x32_64x64_c(*sad16x16_32b.data(),
                                                  best_sad32x32[0],
                                                  &best_sad64x64[0],
                                                  best_mv32x32[0],
                                                  &best_mv64x64[0],
                                                  mv,
                                                  sad_32x32[0]);

            test_func_(*sad16x16_32b.data(),
                       best_sad32x32[1],
                       &best_sad64x64[1],
                       best_mv32x32[1],
                       &best_mv64x64[1],
                       mv,
                       sad_32x32[1]);

            EXPECT_EQ(0,
                      memcmp(best_sad32x32[0],
                             best_sad32x32[1],
                             sizeof(best_sad32x32[0])))
                << "compare best_sad32x32 error";
            EXPECT_EQ(
                0,
                memcmp(
                    best_mv32x32[0], best_mv32x32[1], sizeof(best_mv32x32[0])))
                << "compare best_mv32x32 error";
            EXPECT_EQ(best_sad64x64[0], best_sad64x64[1])
                << "compare best_sad64x64 error";
            EXPECT_EQ(best_mv64x64[0], best_mv64x64[1])
                << "compare best_mv64x64 error";
            EXPECT_EQ(0,
                      memcmp(sad_32x32[0], sad_32x32[1], sizeof(sad_32x32[0])))
                << "compare sad_32x32 error";
        }
    }

    void check_sad(int width, int height) {
        printf("Usage not override a function, %i, %i\n", width, height);
        ASSERT_TRUE(0);
    }

    svt_ext_sad_calculation_32x32_64x64_fn test_func_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(Extsad32x32_CalculationTest);

TEST_P(Extsad32x32_CalculationTest, check_sad32x32) {
    check_sad32x32();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, Extsad32x32_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_sad_calculation_32x32_64x64_sse4_intrin)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, Extsad32x32_CalculationTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PATTERNS),
        ::testing::ValuesIn(TEST_SAD_PATTERNS),
        ::testing::Values(svt_ext_sad_calculation_32x32_64x64_neon)));
#endif  // ARCH_AARCH64

typedef void (*InitBufferFunc)(uint32_t *pointer, uint32_t count128,
                               uint32_t count32, uint32_t value);

using InitializeBuffer_param_t =
    ::testing::tuple<uint32_t, uint32_t, InitBufferFunc>;

#define MAX_BUFFER_SIZE 100  // const value to simplify
class InitializeBuffer32
    : public ::testing::TestWithParam<InitializeBuffer_param_t> {
  public:
    InitializeBuffer32()
        : count128(TEST_GET_PARAM(0)),
          count32(TEST_GET_PARAM(1)),
          test_func_(TEST_GET_PARAM(2)),
          rnd_(0, (1 << 30) - 1) {
        value = rnd_.random();
        _ref_ = (uint32_t *)svt_aom_memalign(32, MAX_BUFFER_SIZE);
        _test_ = (uint32_t *)svt_aom_memalign(32, MAX_BUFFER_SIZE);
        memset(_ref_, 0, MAX_BUFFER_SIZE);
        memset(_test_, 0, MAX_BUFFER_SIZE);
    }

    ~InitializeBuffer32() {
        if (_ref_)
            svt_aom_free(_ref_);
        if (_test_)
            svt_aom_free(_test_);
    }

  protected:
    void checkWithSize() {
        svt_initialize_buffer_32bits_c(_ref_, count128, count32, value);
        test_func_(_test_, count128, count32, value);

        int cmpResult = memcmp(_ref_, _test_, MAX_BUFFER_SIZE);
        EXPECT_EQ(cmpResult, 0);
    }

  private:
    uint32_t *_ref_;
    uint32_t *_test_;
    uint32_t count128;
    uint32_t count32;
    InitBufferFunc test_func_;
    uint32_t value;
    SVTRandom rnd_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(InitializeBuffer32);

TEST_P(InitializeBuffer32, InitializeBuffer) {
    checkWithSize();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, InitializeBuffer32,
    ::testing::Combine(
        ::testing::Values(2, 3, 4), ::testing::Values(1, 2, 3),
        ::testing::Values(svt_initialize_buffer_32bits_sse2_intrin)));
#endif  // ARCH_X86_64

/**
 * @Brief Base class for SAD test. SADTestBasesad_16Bit handle test vector in
 * memory, provide SAD and SAD avg reference function
 */
class SADTestBase16bit : public ::testing::Test {
  public:
    SADTestBase16bit(TestPattern test_pattern) {
        src_stride_ = MAX_SB_SIZE;
        ref_stride_ = MAX_SB_SIZE / 2;
        test_pattern_ = test_pattern;
        src_ = nullptr;
        ref_ = nullptr;
    }

    void SetUp() override {
        src_ = (uint16_t *)svt_aom_memalign(32, MAX_BLOCK_SIZE * sizeof(*src_));
        ref_ = (uint16_t *)svt_aom_memalign(32, MAX_BLOCK_SIZE * sizeof(*ref_));
        ASSERT_NE(src_, nullptr);
        ASSERT_NE(ref_, nullptr);
    }

    void TearDown() override {
        if (src_)
            svt_aom_free(src_);
        if (ref_)
            svt_aom_free(ref_);
    }

    void prepare_data() {
        const int32_t mask = (1 << 10) - 1;
        SVTRandom rnd(0, mask);
        switch (test_pattern_) {
        case REF_MAX: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_[i] = 0;

            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                ref_[i] = mask;

            break;
        }
        case SRC_MAX: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_[i] = mask;

            for (int i = 0; i < MAX_SB_SIZE; i++)
                ref_[i] = 0;

            break;
        }
        case RANDOM: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_[i] = rnd.random();

            for (int i = 0; i < MAX_BLOCK_SIZE; ++i) {
                ref_[i] = rnd.random();
            }
            break;
        };
        case UNALIGN: {
            for (int i = 0; i < MAX_BLOCK_SIZE; i++)
                src_[i] = rnd.random();

            for (int i = 0; i < MAX_BLOCK_SIZE; ++i) {
                ref_[i] = rnd.random();
            }
            ref_stride_ = MAX_SB_SIZE - 1;
            break;
        }
        default: break;
        }
    }

    virtual void check_sad(int width, int height) = 0;
    virtual void speed_sad(int width, int height) = 0;

    void test_sad_size(BlkSize size) {
        check_sad(std::get<0>(size), std::get<1>(size));
    }

    void test_sad_sizes(BlkSize *test_block_sizes,
                        size_t test_block_sizes_count) {
        for (uint32_t i = 0; i < test_block_sizes_count; ++i) {
            test_sad_size(test_block_sizes[i]);
        }
    }

    void speed_sad_size(BlkSize size) {
        speed_sad(std::get<0>(size), std::get<1>(size));
    }

    void speed_sad_sizes(BlkSize *test_block_sizes,
                         size_t test_block_sizes_count) {
        for (uint32_t i = 0; i < test_block_sizes_count; ++i) {
            speed_sad_size(test_block_sizes[i]);
        }
    }

  protected:
    uint32_t src_stride_;
    uint32_t ref_stride_;
    TestPattern test_pattern_;
    SADPattern test_sad_pattern_;
    uint16_t *src_;
    uint16_t *ref_;
};

/**
 * @brief Unit test for svt_aom_sad_16bit_kernel.
 *
 * Test strategy:
 *  This test case combines different width{4-128} x height{4-128} and different
 *  test pattern(REF_MAX, SRC_MAX, RANDOM, UNALIGN). Check the result by compare
 *  result from reference function and SIMD function.
 *
 *
 * Expect result:
 *  Results from reference function SIMD function are equal.
 *
 * Test cases:
 *  Width {4, 8, 16, 24, 32, 48, 64, 128} x height{ 4, 8, 16, 24, 32, 48, 64,
 *  128) Test vector pattern {REF_MAX, SRC_MAX, RANDOM, UNALIGN}
 *
 */

typedef uint32_t (*SadSubsample16BitFunc)(uint16_t *src, uint32_t src_stride,
                                          uint16_t *ref, uint32_t ref_stride,
                                          uint32_t height, uint32_t width);
typedef std::tuple<TestPattern, SadSubsample16BitFunc> SadSubSample16BitParam;

class SADTestSubSample16bit
    : public ::testing::WithParamInterface<SadSubSample16BitParam>,
      public SADTestBase16bit {
  public:
    SADTestSubSample16bit() : SADTestBase16bit(TEST_GET_PARAM(0)) {
        test_func_ = TEST_GET_PARAM(1);
    }

  protected:
    void check_sad(int width, int height) {
        uint32_t repeat = 1;
        if (test_pattern_ == RANDOM) {
            repeat = 30;
        }

        for (uint32_t i = 0; i < repeat; ++i) {
            uint32_t sad_ref = 0;
            uint32_t sad_test = 0;

            prepare_data();

            sad_ref = svt_aom_sad_16b_kernel_c(
                src_, src_stride_, ref_, ref_stride_, height, width);

            sad_test =
                test_func_(src_, src_stride_, ref_, ref_stride_, height, width);

            EXPECT_EQ(sad_ref, sad_test)
                << "Size: " << width << "x" << height << " " << std::endl
                << "Mismatch: sad_ref: " << sad_ref << " sad_test: " << sad_test
                << " repeat: " << i;
        }
    }

    void speed_sad(int width, int height) {
        uint32_t sad_ref = 0;
        uint32_t sad_test = 0;

        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        prepare_data();

        for (uint32_t area_width = 4; area_width <= 128; area_width += 4) {
            const uint32_t area_height = area_width;
            const int num_loops = 1000000000 / (area_width * area_height);
            svt_av1_get_time(&start_time_seconds, &start_time_useconds);

            for (int i = 0; i < num_loops; ++i) {
                sad_ref = svt_aom_sad_16b_kernel_c(
                    src_, src_stride_, ref_, ref_stride_, height, width);
            }

            svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

            for (int i = 0; i < num_loops; ++i) {
                sad_test = test_func_(
                    src_, src_stride_, ref_, ref_stride_, height, width);
            }
            svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

            EXPECT_EQ(sad_ref, sad_test) << area_width << "x" << area_height;

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
            printf("Average Nanoseconds per Function Call\n");
            printf("    svt_aom_sad_16b_kernel_c  (%dx%d) : %6.2f\n",
                   area_width,
                   area_height,
                   1000000 * time_c / num_loops);
            printf(
                "    svt_aom_sad_16bit_kernel_opt(%dx%d) : %6.2f   "
                "(Comparison: %5.2fx)\n",
                area_width,
                area_height,
                1000000 * time_o / num_loops,
                time_c / time_o);
        }
    }

    SadSubsample16BitFunc test_func_;
};

BlkSize TEST_BLOCK_SAD_SIZES[] = {
    BlkSize(4, 4),    BlkSize(4, 8),    BlkSize(4, 16),   BlkSize(4, 24),
    BlkSize(4, 32),   BlkSize(4, 48),   BlkSize(4, 64),   BlkSize(4, 128),
    BlkSize(8, 4),    BlkSize(8, 8),    BlkSize(8, 16),   BlkSize(8, 24),
    BlkSize(8, 32),   BlkSize(8, 48),   BlkSize(8, 64),   BlkSize(8, 128),
    BlkSize(16, 4),   BlkSize(16, 8),   BlkSize(16, 16),  BlkSize(16, 24),
    BlkSize(16, 32),  BlkSize(16, 48),  BlkSize(16, 64),  BlkSize(16, 128),
    BlkSize(24, 4),   BlkSize(24, 8),   BlkSize(24, 16),  BlkSize(24, 24),
    BlkSize(24, 32),  BlkSize(24, 48),  BlkSize(24, 64),  BlkSize(24, 128),
    BlkSize(32, 4),   BlkSize(32, 8),   BlkSize(32, 16),  BlkSize(32, 24),
    BlkSize(32, 32),  BlkSize(32, 48),  BlkSize(32, 64),  BlkSize(32, 128),
    BlkSize(48, 4),   BlkSize(48, 8),   BlkSize(48, 16),  BlkSize(48, 24),
    BlkSize(48, 32),  BlkSize(48, 48),  BlkSize(48, 64),  BlkSize(48, 128),
    BlkSize(64, 4),   BlkSize(64, 8),   BlkSize(64, 16),  BlkSize(64, 24),
    BlkSize(64, 32),  BlkSize(64, 48),  BlkSize(64, 64),  BlkSize(64, 128),
    BlkSize(128, 4),  BlkSize(128, 8),  BlkSize(128, 16), BlkSize(128, 24),
    BlkSize(128, 32), BlkSize(128, 48), BlkSize(128, 64), BlkSize(128, 128)};

TEST_P(SADTestSubSample16bit, SADTestSubSample16bit) {
    test_sad_sizes(
        TEST_BLOCK_SAD_SIZES,
        sizeof(TEST_BLOCK_SAD_SIZES) / sizeof(TEST_BLOCK_SAD_SIZES[0]));
}

TEST_P(SADTestSubSample16bit, DISABLED_Speed) {
    speed_sad_sizes(
        TEST_BLOCK_SAD_SIZES,
        sizeof(TEST_BLOCK_SAD_SIZES) / sizeof(TEST_BLOCK_SAD_SIZES[0]));
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, SADTestSubSample16bit,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_aom_sad_16bit_kernel_avx2)));
#endif

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, SADTestSubSample16bit,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::Values(svt_aom_sad_16b_kernel_neon)));
#endif  // ARCH_AARCH64

typedef void (*PmeSadLoopKernel)(
    const svt_mv_cost_param *mv_cost_params, uint8_t *src, uint32_t src_stride,
    uint8_t *ref, uint32_t ref_stride, uint32_t block_height,
    uint32_t block_width, uint32_t *best_cost, int16_t *best_mvx,
    int16_t *best_mvy, int16_t search_position_start_x,
    int16_t search_position_start_y, int16_t search_area_width,
    int16_t search_area_height, int16_t search_step, int16_t mvx, int16_t mvy);

BlkSize TEST_BLOCK_SIZES_LARGE[] = {
    BlkSize(64, 128), BlkSize(128, 128), BlkSize(128, 64)};

typedef std::tuple<TestPattern, SearchArea, PmeSadLoopKernel>
    PmeSadLoopTestParam;

class PmeSadLoopTest
    : public ::testing::WithParamInterface<PmeSadLoopTestParam>,
      public SADTestBase {
  public:
    PmeSadLoopTest()
        : SADTestBase(TEST_GET_PARAM(0), std::get<0>(TEST_GET_PARAM(1)),
                      std::get<1>(TEST_GET_PARAM(1))) {
        test_func_ = TEST_GET_PARAM(2);
        SVTRandom rnd(INT16_MIN, INT16_MAX);

        search_step = 8;
        mvx = rnd.random();
        mvy = rnd.random();
        search_position_start_x = rnd.random();
        search_position_start_y = rnd.random();
        ref_mv = {{(int16_t)(76), (int16_t)(23)}};
        mv_jcost[0] = 11;
        mv_jcost[1] = 54;
        mv_jcost[2] = 5437;
        mv_jcost[3] = 342;

        for (int ddd = 0; ddd < MV_VALS; ddd++) {
            mv_cost[ddd] = rnd.Rand16();
        }
    }

  protected:
    PmeSadLoopKernel test_func_;
    int16_t search_step;
    int16_t mvx;
    int16_t mvy;
    int16_t search_position_start_x;
    int16_t search_position_start_y;
    svt_mv_cost_param mv_cost_params;
    Mv ref_mv;
    int32_t mv_jcost[MV_JOINTS];
    int mv_cost[MV_VALS];

    void check_sad(int width, int height) {
        prepare_data();

        PmeSadLoopKernel func_c_ = svt_pme_sad_loop_kernel_c;

        mv_cost_params.ref_mv = &ref_mv;
        mv_cost_params.full_ref_mv = {
            {(int16_t)GET_MV_RAWPEL(76), (int16_t)GET_MV_RAWPEL(23)}};
        mv_cost_params.mv_cost_type = MV_COST_ENTROPY;
        mv_cost_params.mvjcost = mv_jcost;
        mv_cost_params.mvcost[0] = &mv_cost[MV_MAX];
        mv_cost_params.mvcost[1] = &mv_cost[MV_MAX];
        mv_cost_params.error_per_bit = 20542;
        mv_cost_params.early_exit_th = 14130;
        mv_cost_params.sad_per_bit = 442;

        uint32_t best_sad0 = UINT32_MAX;
        int16_t best_mvx0 = 0;
        int16_t best_mvy0 = 0;
        func_c_(&mv_cost_params,
                src_aligned_,
                src_stride_,
                ref1_aligned_,
                ref1_stride_,
                height,
                width,
                &best_sad0,
                &best_mvx0,
                &best_mvy0,
                search_position_start_x,
                search_position_start_y,
                (search_area_width_ & 0xfffffff8),
                search_area_height_,
                search_step,
                mvx,
                mvy);
        uint32_t best_sad1 = UINT32_MAX;
        int16_t best_mvx1 = 0;
        int16_t best_mvy1 = 0;

        test_func_(&mv_cost_params,
                   src_aligned_,
                   src_stride_,
                   ref1_aligned_,
                   ref1_stride_,
                   height,
                   width,
                   &best_sad1,
                   &best_mvx1,
                   &best_mvy1,
                   search_position_start_x,
                   search_position_start_y,
                   (search_area_width_ & 0xfffffff8),
                   search_area_height_,
                   search_step,
                   mvx,
                   mvy);

        EXPECT_EQ(best_sad0, best_sad1)
            << "compare best_sad error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(best_mvx0, best_mvx1)
            << "compare x_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(best_mvy0, best_mvy1)
            << "compare y_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
    }

    void speed_sad(int width, int height) {
        const uint64_t num_loop = 100000;
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        PmeSadLoopKernel func_c_ = svt_pme_sad_loop_kernel_c;

        prepare_data();

        mv_cost_params.ref_mv = &ref_mv;
        mv_cost_params.full_ref_mv = {
            {(int16_t)GET_MV_RAWPEL(76), (int16_t)GET_MV_RAWPEL(23)}};
        mv_cost_params.mv_cost_type = MV_COST_ENTROPY;
        mv_cost_params.mvjcost = mv_jcost;
        mv_cost_params.mvcost[0] = &mv_cost[MV_MAX];
        mv_cost_params.mvcost[1] = &mv_cost[MV_MAX];
        mv_cost_params.error_per_bit = 20542;
        mv_cost_params.early_exit_th = 14130;
        mv_cost_params.sad_per_bit = 442;

        uint32_t best_sad0 = UINT32_MAX;
        uint32_t best_sad1 = UINT32_MAX;
        int16_t best_mvx0 = 0;
        int16_t best_mvy0 = 0;
        int16_t best_mvx1 = 0;
        int16_t best_mvy1 = 0;

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            func_c_(&mv_cost_params,
                    src_aligned_,
                    src_stride_,
                    ref1_aligned_,
                    ref1_stride_,
                    height,
                    width,
                    &best_sad0,
                    &best_mvx0,
                    &best_mvy0,
                    search_position_start_x,
                    search_position_start_y,
                    (search_area_width_ & 0xfffffff8),
                    search_area_height_,
                    search_step,
                    mvx,
                    mvy);
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            test_func_(&mv_cost_params,
                       src_aligned_,
                       src_stride_,
                       ref1_aligned_,
                       ref1_stride_,
                       height,
                       width,
                       &best_sad1,
                       &best_mvx1,
                       &best_mvy1,
                       search_position_start_x,
                       search_position_start_y,
                       (search_area_width_ & 0xfffffff8),
                       search_area_height_,
                       search_step,
                       mvx,
                       mvy);
        }

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

        EXPECT_EQ(best_sad0, best_sad1)
            << "compare best_sad error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(best_mvx0, best_mvx1)
            << "compare x_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";
        EXPECT_EQ(best_mvy0, best_mvy1)
            << "compare y_search_center error"
            << " block dim: [" << width << " x " << height << "] "
            << "search area [" << search_area_width_ << " x "
            << search_area_height_ << "]";

        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        printf("    pme_sad_loop_kernel(%dx%d) search area[%dx%d]: %5.2fx)\n",
               width,
               height,
               search_area_width_,
               search_area_height_,
               time_c / time_o);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(PmeSadLoopTest);

TEST_P(PmeSadLoopTest, PmeSadLoopTest) {
    test_sad_sizes(TEST_BLOCK_SIZES,
                   sizeof(TEST_BLOCK_SIZES) / sizeof(TEST_BLOCK_SIZES[0]));
    test_sad_sizes(
        TEST_BLOCK_SIZES_LARGE,
        sizeof(TEST_BLOCK_SIZES_LARGE) / sizeof(TEST_BLOCK_SIZES_LARGE[0]));
}

TEST_P(PmeSadLoopTest, DISABLED_PmeSadLoopSpeedTest) {
    speed_sad_sizes(TEST_BLOCK_SIZES,
                    sizeof(TEST_BLOCK_SIZES) / sizeof(TEST_BLOCK_SIZES[0]));
    speed_sad_sizes(
        TEST_BLOCK_SIZES_LARGE,
        sizeof(TEST_BLOCK_SIZES_LARGE) / sizeof(TEST_BLOCK_SIZES_LARGE[0]));
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, PmeSadLoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(svt_pme_sad_loop_kernel_avx2)));
#endif

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, PmeSadLoopTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PATTERNS),
                       ::testing::ValuesIn(TEST_LOOP_AREAS),
                       ::testing::Values(svt_pme_sad_loop_kernel_neon)));
#endif  // ARCH_AARCH64

}  // namespace
