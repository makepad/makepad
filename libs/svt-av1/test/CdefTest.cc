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
 * @file CdefTest.cc
 *
 * @brief Unit test for cdef tools:
 * * svt_aom_cdef_find_dir
 * * svt_aom_cdef_find_dir_dual
 * * svt_cdef_filter_block
 * * svt_aom_compute_cdef_dist_16bit
 * * svt_aom_copy_rect8_8bit_to_16bit
 * * svt_search_one_dual
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include <cstdlib>
#include <string>

#include "gtest/gtest.h"
#include "definitions.h"
#include "aom_dsp_rtcd.h"
#include "enc_cdef.h"
#include "util.h"
#include "random.h"
#include "unit_test_utility.h"
#include "utility.h"

using svt_av1_test_tool::SVTRandom;
using ::testing::make_tuple;
namespace {
// The last argument for cdef_dir_param_t refers to the simd level to set
// svt_cdef_filter_block_8xn_16 to. The function is used for AVX2 and AVX512
// only, but should point to different functions for each. 0: AVX2/AVX512 not
// used (don't need to set function) 1: set func to AVX2 2: set func to AVX512
using cdef_dir_param_t =
    ::testing::tuple<CdefFilterBlockFunc, CdefFilterBlockFunc, BlockSize, int,
                     int, int>;
/**
 * @brief Unit test for svt_cdef_filter_block
 *
 * Test strategy:
 * Feed src data generated randomly and all possible input,
 * then check the dst buffer from target function and reference
 * function.
 *
 * Expect result:
 * The dst buffer from targeted function
 * should be identical with the values from reference function.
 *
 * Test coverage:
 * Test cases:
 * primary_strength: [0, 15] << (bd_ - 8)
 * second_strength: 0, 1, 2, 4
 * primary_damping: [3, 6] + (bd_ - 8)
 * second_damping: [3, 6] + (bd_ - 8)
 * direction: [0, 7]
 * bitdepth: 8, 10, 12
 *
 */
class CDEFBlockTest : public ::testing::TestWithParam<cdef_dir_param_t> {
  public:
    CDEFBlockTest() : rnd_(0, (1 << 16) - 1) {
    }

    virtual ~CDEFBlockTest() {
    }

    virtual void SetUp() {
        cdef_tst_ = TEST_GET_PARAM(0);
        cdef_ref_ = TEST_GET_PARAM(1);
        bsize_ = TEST_GET_PARAM(2);
        boundary_ = TEST_GET_PARAM(3);
        bd_ = TEST_GET_PARAM(4);
#if defined(ARCH_X86_64)
        if (TEST_GET_PARAM(5)) {
            if (TEST_GET_PARAM(5) == 1)
                svt_cdef_filter_block_8xn_16 =
                    svt_cdef_filter_block_8xn_16_avx2;
#if EN_AVX512_SUPPORT
            else
                svt_cdef_filter_block_8xn_16 =
                    svt_cdef_filter_block_8xn_16_avx512;
#endif
        }
#endif

        memset(dst_ref_, 0, sizeof(dst_ref_));
        memset(dst_tst_, 0, sizeof(dst_tst_));
    }

    virtual void TearDown() {
    }

    void prepare_data(int level, int bits) {
        for (uint32_t i = 0; i < sizeof(src_) / sizeof(*src_); i++)
            src_[i] = clamp(
                (rnd_.random() & ((1 << bits) - 1)) + level, 0, (1 << bd_) - 1);

        if (boundary_) {
            if (boundary_ & 1) {  // Left
                for (int i = 0; i < ysize_; i++)
                    for (int j = 0; j < CDEF_HBORDER; j++)
                        src_[i * CDEF_BSTRIDE + j] = CDEF_VERY_LARGE;
            }
            if (boundary_ & 2) {  // Right
                for (int i = 0; i < ysize_; i++)
                    for (int j = CDEF_HBORDER + size_; j < CDEF_BSTRIDE; j++)
                        src_[i * CDEF_BSTRIDE + j] = CDEF_VERY_LARGE;
            }
            if (boundary_ & 4) {  // Above
                for (int i = 0; i < CDEF_VBORDER; i++)
                    for (int j = 0; j < CDEF_BSTRIDE; j++)
                        src_[i * CDEF_BSTRIDE + j] = CDEF_VERY_LARGE;
            }
            if (boundary_ & 8) {  // Below
                for (int i = CDEF_VBORDER + size_; i < ysize_; i++)
                    for (int j = 0; j < CDEF_BSTRIDE; j++)
                        src_[i * CDEF_BSTRIDE + j] = CDEF_VERY_LARGE;
            }
        }
    }

    void run_test(int pri_damping, int sec_damping,
                  uint8_t subsampling_factor) {
        int pri_strength, sec_strength;
        int dir;
        unsigned int pos = 0;
        const unsigned int max_pos =
            size_ * size_ >> static_cast<int>(bd_ == 8);
        for (dir = 0; dir < 8; dir++) {
            // primary strength range between [0, 15], scale the range and step
            // for high bitdepth; For example, 12-bit content can have strengths
            // value of 0, 16, 32
            for (pri_strength = 0; pri_strength <= 19 << (bd_ - 8);
                 pri_strength += (1 + 4 * !!boundary_) << (bd_ - 8)) {
                if (pri_strength == 16)
                    pri_strength = 19;
                /* second strength can only be 0, 1, 2, 4 for 8-bit */
                for (sec_strength = 0; sec_strength <= 4 << (bd_ - 8);
                     sec_strength += 1 << (bd_ - 8)) {
                    if (sec_strength == 3 << (bd_ - 8))
                        continue;
                    cdef_ref_(bd_ == 8 ? (uint8_t *)dst_ref_ : 0,
                              dst_ref_,
                              size_,
                              src_ + CDEF_HBORDER + CDEF_VBORDER * CDEF_BSTRIDE,
                              pri_strength,
                              sec_strength,
                              dir,
                              pri_damping,
                              sec_damping,
                              bsize_,
                              bd_ - 8,
                              subsampling_factor);
                    cdef_tst_(bd_ == 8 ? (uint8_t *)dst_tst_ : 0,
                              dst_tst_,
                              size_,
                              src_ + CDEF_HBORDER + CDEF_VBORDER * CDEF_BSTRIDE,
                              pri_strength,
                              sec_strength,
                              dir,
                              pri_damping,
                              sec_damping,
                              bsize_,
                              bd_ - 8,
                              subsampling_factor);
                    for (pos = 0; pos < max_pos; pos++) {
                        ASSERT_EQ(dst_ref_[pos], dst_tst_[pos])
                            << "Error: CDEFBlockTest, SIMD and C mismatch."
                            << std::endl
                            << "First error at " << pos % size_ << ","
                            << pos / size_ << " (" << dst_ref_[pos] << " : "
                            << dst_tst_[pos] << ") " << std::endl
                            << "pristrength: " << pri_strength << std::endl
                            << "pridamping: " << pri_damping << std::endl
                            << "secstrength: " << sec_strength << std::endl
                            << "secdamping: " << sec_damping << std::endl
                            << "bitdepth: " << bd_ << std::endl
                            << "size: " << bsize_ << std::endl
                            << "boundary: " << boundary_ << std::endl
                            << std::endl;
                    }
                }
            }
        }
    }

    void test_cdef(int iterations) {
        int pri_damping, sec_damping, bits, level, count;
        // for 8-bit content, damping ranges [3, 6] for luma, [2, 5] for chroma
        // for highbd content, range will add the additinal bitdepth. 12-bit
        // content will range [7, 10]
        const int scale = boundary_ > 0 ? 1 : 0;
        const int min_damping = 3 + bd_ - 8;
        const int max_damping = 7 - 3 * scale + bd_ - 8;
        for (pri_damping = min_damping; pri_damping < max_damping;
             pri_damping++) {
            for (sec_damping = min_damping; sec_damping < max_damping;
                 sec_damping++) {
                for (count = 0; count < iterations; count++) {
                    for (level = 0; level < (1 << bd_);
                         level += (2 + 6 * scale) << (bd_ - 8)) {
                        for (bits = 1; bits <= bd_; bits += 1 + 3 * scale) {
                            prepare_data(level, bits);
                            // Allowable subsampling values are: 1, 2
                            run_test(pri_damping, sec_damping, 1);

                            /*
                            The 2x subsampled avx2 kernel for 4x4 blocks is the
                            same as the non-subsampled kernel because all values
                            can be filtered at once, so subsampling would not
                            give a speed gain. Therefore, the C/AVX2 kernels for
                            4x4 2x subsampled blocks will produce different
                            outputs (expected), and are not tested.
                            */
                            if (bsize_ > BLOCK_4X4)
                                run_test(pri_damping, sec_damping, 2);
                        }
                    }
                }
            }
        }
    }

    void speed_cdef() {
        const int min_damping = 3 + bd_ - 8;
        const int pri_damping = min_damping;
        const int sec_damping = min_damping;
        int pri_strength, sec_strength;
        int dir;
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;
        const uint64_t num_loop = 100;

        prepare_data(0, 1);

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            for (dir = 0; dir < 8; dir++) {
                // primary strength range between [0, 15], scale the range and
                // step for high bitdepth; For example, 12-bit content can have
                // strengths value of 0, 16, 32
                for (pri_strength = 0; pri_strength <= 19 << (bd_ - 8);
                     pri_strength += (1 + 4 * !!boundary_) << (bd_ - 8)) {
                    if (pri_strength == 16)
                        pri_strength = 19;
                    /* second strength can only be 0, 1, 2, 4 for 8-bit */
                    for (sec_strength = 0; sec_strength <= 4 << (bd_ - 8);
                         sec_strength += 1 << (bd_ - 8)) {
                        if (sec_strength == 3 << (bd_ - 8))
                            continue;
                        cdef_ref_(
                            bd_ == 8 ? (uint8_t *)dst_ref_ : 0,
                            dst_ref_,
                            size_,
                            src_ + CDEF_HBORDER + CDEF_VBORDER * CDEF_BSTRIDE,
                            pri_strength,
                            sec_strength,
                            dir,
                            pri_damping,
                            sec_damping,
                            bsize_,
                            bd_ - 8,
                            1);  // subsampling - test only non-subsampled for
                                 // the speed
                    }
                }
            }
        }

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++) {
            for (dir = 0; dir < 8; dir++) {
                // primary strength range between [0, 15], scale the range and
                // step for high bitdepth; For example, 12-bit content can have
                // strengths value of 0, 16, 32
                for (pri_strength = 0; pri_strength <= 19 << (bd_ - 8);
                     pri_strength += (1 + 4 * !!boundary_) << (bd_ - 8)) {
                    if (pri_strength == 16)
                        pri_strength = 19;
                    /* second strength can only be 0, 1, 2, 4 for 8-bit */
                    for (sec_strength = 0; sec_strength <= 4 << (bd_ - 8);
                         sec_strength += 1 << (bd_ - 8)) {
                        if (sec_strength == 3 << (bd_ - 8))
                            continue;
                        cdef_tst_(
                            bd_ == 8 ? (uint8_t *)dst_tst_ : 0,
                            dst_tst_,
                            size_,
                            src_ + CDEF_HBORDER + CDEF_VBORDER * CDEF_BSTRIDE,
                            pri_strength,
                            sec_strength,
                            dir,
                            pri_damping,
                            sec_damping,
                            bsize_,
                            bd_ - 8,
                            1);  // subsampling - test only non-subsampled for
                                 // the speed
                    }
                }
            }
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

        printf("Average Nanoseconds per Function Call\n");
        printf("    svt_cdef_filter_block_c()   : %6.2f\n",
               1000000 * time_c / num_loop);
        printf(
            "    svt_cdef_filter_block_opt() : %6.2f   (Comparison: %5.2fx)\n",
            1000000 * time_o / num_loop,
            time_c / time_o);
    }

  protected:
    int bsize_;
    int boundary_;
    int bd_;
    CdefFilterBlockFunc cdef_tst_;
    CdefFilterBlockFunc cdef_ref_;
    SVTRandom rnd_;
    static const int size_ = 8;
    static const int ysize_ = size_ + 2 * CDEF_VBORDER;
    DECLARE_ALIGNED(16, uint16_t, src_[ysize_ * CDEF_BSTRIDE]);
    DECLARE_ALIGNED(16, uint16_t, dst_tst_[size_ * size_]);
    DECLARE_ALIGNED(16, uint16_t, dst_ref_[size_ * size_]);
};

TEST_P(CDEFBlockTest, MatchTest) {
    test_cdef(1);
}

TEST_P(CDEFBlockTest, DISABLED_SpeedTest) {
    speed_cdef();
}

#if defined(ARCH_X86_64)

// VS compiling for 32 bit targets does not support vector types in
// structs as arguments, which makes the v256 type of the intrinsics
// hard to support, so optimizations for this target are disabled.
#if defined(_WIN64) || !defined(_MSC_VER) || defined(__clang__)
#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, CDEFBlockTest,
    ::testing::Combine(::testing::Values(&svt_cdef_filter_block_avx2),
                       ::testing::Values(&svt_cdef_filter_block_c),
                       ::testing::Values(BLOCK_4X4, BLOCK_4X8, BLOCK_8X4,
                                         BLOCK_8X8),
                       ::testing::Range(0, 16), ::testing::Range(8, 13, 2),
                       ::testing::Values(2)));
#endif

INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFBlockTest,
    ::testing::Combine(::testing::Values(&svt_cdef_filter_block_avx2),
                       ::testing::Values(&svt_cdef_filter_block_c),
                       ::testing::Values(BLOCK_4X4, BLOCK_4X8, BLOCK_8X4,
                                         BLOCK_8X8),
                       ::testing::Range(0, 16), ::testing::Range(8, 13, 2),
                       ::testing::Values(1)));

// SSE4_1 test invokes avx2 as reference; therefore svt_cdef_filter_block_8xn_16
// must be set to the avx2 version.
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, CDEFBlockTest,
    ::testing::Combine(::testing::Values(&svt_cdef_filter_block_avx2),
                       ::testing::Values(&svt_av1_cdef_filter_block_sse4_1),
                       ::testing::Values(BLOCK_4X4, BLOCK_4X8, BLOCK_8X4,
                                         BLOCK_8X8),
                       ::testing::Range(0, 16), ::testing::Range(8, 13, 2),
                       ::testing::Values(1)));

#endif  // defined(_WIN64) || !defined(_MSC_VER)
#endif  // defined(ARCH_X86_64)

#if defined(ARCH_AARCH64)
INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFBlockTest,
    ::testing::Combine(::testing::Values(&svt_cdef_filter_block_neon),
                       ::testing::Values(&svt_cdef_filter_block_c),
                       ::testing::Values(BLOCK_4X4, BLOCK_4X8, BLOCK_8X4,
                                         BLOCK_8X8),
                       ::testing::Range(0, 16), ::testing::Range(8, 13, 2),
                       ::testing::Values(0)));
#endif  // defined(ARCH_AARCH64)

using FindDirFunc = uint8_t (*)(const uint16_t *img, int stride, int32_t *var,
                                int coeff_shift);
using TestFindDirParam = ::testing::tuple<FindDirFunc, FindDirFunc>;

/**
 * @brief Unit test for svt_cdef_find_dir
 *
 * Test strategy:
 * Feed src data generated randomly, and check the best mse and
 * the directional contrast from target function and reference
 * function
 *
 * Expect result:
 * The best mse and directional contrast from targeted function
 * should be identical with the values from reference function.
 *
 * Test coverage:
 * Test cases:
 * bitdepth: 8, 10, 12
 *
 */
class CDEFFindDirTest : public ::testing::TestWithParam<TestFindDirParam> {
  public:
    CDEFFindDirTest() : rnd_(0, (1 << 16) - 1) {
    }

    virtual ~CDEFFindDirTest() {
    }

    virtual void SetUp() {
        func_tst_ = TEST_GET_PARAM(0);
        func_ref_ = TEST_GET_PARAM(1);
    }

    virtual void TearDown() {
    }

    void prepare_data(int depth, int bits, int level) {
        for (unsigned int i = 0; i < sizeof(src_) / sizeof(src_[0]); i++)
            src_[i] = clamp((rnd_.random() & ((1 << bits) - 1)) + level,
                            0,
                            (1 << depth) - 1);
    }

    void test_finddir() {
        int depth, bits, level, count;
        uint8_t res_ref = 0, res_tst = 0;
        int32_t var_ref = 0, var_tst = 0;

        for (depth = 8; depth <= 12; depth += 2) {
            for (count = 0; count < 512; count++) {
                const int shift = depth - 8;
                for (level = 0; level < (1 << depth); level += 1 << shift) {
                    for (bits = 1; bits <= depth; bits++) {
                        prepare_data(depth, bits, level);

                        res_ref = func_ref_(src_, size_, &var_ref, shift);
                        res_tst = func_tst_(src_, size_, &var_tst, shift);
                        ASSERT_EQ(res_tst, res_ref)
                            << "Error: CDEFFindDirTest, SIMD and C mismatch."
                            << "return " << res_tst << " : " << res_ref
                            << "depth: " << depth;
                        ASSERT_EQ(var_tst, var_ref)
                            << "Error: CDEFFindDirTest, SIMD and C mismatch."
                            << "var: " << var_tst << " : " << var_ref
                            << "depth: " << depth;
                    }
                }
            }
        }
    }

  protected:
    FindDirFunc func_tst_;
    FindDirFunc func_ref_;
    SVTRandom rnd_;
    static const int size_ = 8;
    DECLARE_ALIGNED(16, uint16_t, src_[size_ * size_]);
};

TEST_P(CDEFFindDirTest, MatchTest) {
    test_finddir();
}

#if defined(ARCH_X86_64)

// VS compiling for 32 bit targets does not support vector types in
// structs as arguments, which makes the v256 type of the intrinsics
// hard to support, so optimizations for this target are disabled.
#if defined(_WIN64) || !defined(_MSC_VER) || defined(__clang__)
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, CDEFFindDirTest,
    ::testing::Values(make_tuple(&svt_aom_cdef_find_dir_sse4_1,
                                 &svt_aom_cdef_find_dir_c)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFFindDirTest,
    ::testing::Values(make_tuple(&svt_aom_cdef_find_dir_avx2,
                                 &svt_aom_cdef_find_dir_c)));
#endif  // defined(_WIN64) || !defined(_MSC_VER)

#endif  // defined(ARCH_X86_64)

#if defined(ARCH_AARCH64)

INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFFindDirTest,
    ::testing::Values(make_tuple(&svt_aom_cdef_find_dir_neon,
                                 &svt_aom_cdef_find_dir_c)));

#endif

using FindDirDualFunc = void (*)(const uint16_t *img1, const uint16_t *img2,
                                 int stride, int32_t *var1, int32_t *var2,
                                 int32_t coeff_shift, uint8_t *out1,
                                 uint8_t *out2);
using TestFindDirDualParam = ::testing::tuple<FindDirDualFunc, FindDirDualFunc>;
/**
 * @brief Unit test for svt_cdef_find_dir_dual
 *
 * Test strategy:
 * Feed src data generated randomly, and check the best mse and
 * the directional contrast from target function and reference
 * function
 *
 * Expect result:
 * The best mse and directional contrast from targeted function
 * should be identical with the values from reference function.
 *
 * Test coverage:
 * Test cases:
 * bitdepth: 8, 10, 12
 *
 */
class CDEFFindDirDualTest
    : public ::testing::TestWithParam<TestFindDirDualParam> {
  public:
    CDEFFindDirDualTest() : rnd_(0, (1 << 16) - 1) {
    }

    virtual ~CDEFFindDirDualTest() {
    }

    virtual void SetUp() {
        func_tst_ = TEST_GET_PARAM(0);
        func_ref_ = TEST_GET_PARAM(1);
    }

    virtual void TearDown() {
    }

    void prepare_data(int depth, int bits, int level) {
        for (unsigned int i = 0; i < sizeof(src_) / sizeof(src_[0]); i++)
            src_[i] = clamp((rnd_.random() & ((1 << bits) - 1)) + level,
                            0,
                            (1 << depth) - 1);
        for (unsigned int i = 0; i < sizeof(src2_) / sizeof(src2_[0]); i++)
            src2_[i] = clamp((rnd_.random() & ((1 << bits) - 1)) + level,
                             0,
                             (1 << depth) - 1);
    }

    void test_finddir() {
        int depth, bits, level, count;
        uint8_t res_ref_1 = 0, res_tst_1 = 0, res_ref_2 = 0, res_tst_2 = 0;
        int32_t var_ref_1 = 0, var_tst_1 = 0, var_ref_2 = 0, var_tst_2 = 0;

        for (depth = 8; depth <= 12; depth += 2) {
            for (count = 0; count < 512; count++) {
                const int shift = depth - 8;
                for (level = 0; level < (1 << depth); level += 1 << shift) {
                    for (bits = 1; bits <= depth; bits++) {
                        prepare_data(depth, bits, level);

                        func_ref_(src_,
                                  src2_,
                                  size_,
                                  &var_ref_1,
                                  &var_ref_2,
                                  shift,
                                  &res_ref_1,
                                  &res_ref_2);
                        func_tst_(src_,
                                  src2_,
                                  size_,
                                  &var_tst_1,
                                  &var_tst_2,
                                  shift,
                                  &res_tst_1,
                                  &res_tst_2);
                        ASSERT_EQ(res_tst_1, res_ref_1)
                            << "Error: CDEFFindDirDualTest, SIMD and C "
                               "mismatch."
                            << "return " << res_tst_1 << " : " << res_ref_1
                            << "depth: " << depth;
                        ASSERT_EQ(res_tst_2, res_ref_2)
                            << "Error: CDEFFindDirDualTest, SIMD and C "
                               "mismatch."
                            << "return " << res_tst_2 << " : " << res_ref_2
                            << "depth: " << depth;
                        ASSERT_EQ(var_tst_1, var_ref_1)
                            << "Error: CDEFFindDirDualTest, SIMD and C "
                               "mismatch."
                            << "var: " << var_tst_1 << " : " << var_ref_1
                            << "depth: " << depth;
                        ASSERT_EQ(var_tst_2, var_ref_2)
                            << "Error: CDEFFindDirDualTest, SIMD and C "
                               "mismatch."
                            << "var: " << var_tst_2 << " : " << var_ref_2
                            << "depth: " << depth;
                    }
                }
            }
        }
    }

  protected:
    FindDirDualFunc func_tst_;
    FindDirDualFunc func_ref_;
    SVTRandom rnd_;
    static const int size_ = 8;
    DECLARE_ALIGNED(16, uint16_t, src_[size_ * size_]);
    DECLARE_ALIGNED(16, uint16_t, src2_[size_ * size_]);
};

TEST_P(CDEFFindDirDualTest, MatchTest) {
    test_finddir();
}

#if defined(ARCH_X86_64)

// VS compiling for 32 bit targets does not support vector types in
// structs as arguments, which makes the v256 type of the intrinsics
// hard to support, so optimizations for this target are disabled.
#if defined(_WIN64) || !defined(_MSC_VER) || defined(__clang__)
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, CDEFFindDirDualTest,
    ::testing::Values(make_tuple(&svt_aom_cdef_find_dir_dual_sse4_1,
                                 &svt_aom_cdef_find_dir_dual_c)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFFindDirDualTest,
    ::testing::Values(make_tuple(&svt_aom_cdef_find_dir_dual_avx2,
                                 &svt_aom_cdef_find_dir_dual_c)));

#endif  // defined(_WIN64) || !defined(_MSC_VER)

#endif  // defined(ARCH_X86_64)

#if defined(ARCH_AARCH64)

INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFFindDirDualTest,
    ::testing::Values(make_tuple(&svt_aom_cdef_find_dir_dual_neon,
                                 &svt_aom_cdef_find_dir_dual_c)));

#endif  // defined(ARCH_AARCH64)

}  // namespace

/**
 * @brief Unit test for svt_aom_copy_rect8_8bit_to_16bit
 *
 * Test strategy:
 * Feed src data generated randomly, and check the dst data
 *
 * Expect result:
 * The dst data from targeted function should be identical
 * with the date from reference function.
 *
 * Test coverage:
 * Test cases:
 * hsize: [8, 64], step is 8
 * vsize: [8, 64], step is 8
 *
 */

using CDEFCopyRectFunc = void (*)(uint16_t *dst, int32_t dstride,
                                  const uint8_t *src, int32_t sstride,
                                  int32_t v, int32_t h);

class CDEFCopyRectTest : public ::testing::TestWithParam<CDEFCopyRectFunc> {
  public:
    CDEFCopyRectTest() : test_func_(GetParam()) {
    }

    void test_match() {
        SVTRandom rnd_(8, false);

        DECLARE_ALIGNED(16, uint8_t, src_data_[CDEF_INBUF_SIZE]);
        DECLARE_ALIGNED(16, uint16_t, dst_data_tst_[CDEF_INBUF_SIZE]);
        DECLARE_ALIGNED(16, uint16_t, dst_data_ref_[CDEF_INBUF_SIZE]);

        // prepare src data
        for (int i = 0; i < CDEF_INBUF_SIZE; ++i)
            src_data_[i] = rnd_.random();

        // assume the width or height are multiple of 8
        for (int hsize = 8; hsize <= 64; hsize += 8) {
            for (int vsize = 8; vsize <= 64; vsize += 8) {
                memset(dst_data_tst_, 0, sizeof(dst_data_tst_));
                memset(dst_data_ref_, 0, sizeof(dst_data_ref_));

                uint8_t *src_ =
                    src_data_ + CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
                uint16_t *dst_tst_ =
                    dst_data_tst_ + CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
                uint16_t *dst_ref_ =
                    dst_data_ref_ + CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;

                svt_aom_copy_rect8_8bit_to_16bit_c(
                    dst_ref_, CDEF_BSTRIDE, src_, CDEF_BSTRIDE, vsize, hsize);

                memset(dst_data_tst_, 0, sizeof(dst_data_tst_));
                test_func_(
                    dst_tst_, CDEF_BSTRIDE, src_, CDEF_BSTRIDE, vsize, hsize);
                for (int i = 0; i < vsize; ++i) {
                    for (int j = 0; j < hsize; ++j)
                        ASSERT_EQ(dst_ref_[i * CDEF_BSTRIDE + j],
                                  dst_tst_[i * CDEF_BSTRIDE + j])
                            << "copy_rect8_8bit_to_16bit_opt failed with pos("
                            << i << " " << j << ")";
                }
            }
        }
    }

  private:
    CDEFCopyRectFunc test_func_;
};

TEST_P(CDEFCopyRectTest, test_match) {
    test_match();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, CDEFCopyRectTest,
    ::testing::Values(svt_aom_copy_rect8_8bit_to_16bit_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFCopyRectTest,
    ::testing::Values(svt_aom_copy_rect8_8bit_to_16bit_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFCopyRectTest,
    ::testing::Values(svt_aom_copy_rect8_8bit_to_16bit_neon));
#endif  // ARCH_AARCH64

/**
 * @brief Unit test for svt_aom_compute_cdef_dist_16bit
 *
 * Test strategy:
 * Feed cdef list, src buffer, dst buffer generated randomly to targeted
 * and reference functions, and compare mse returned.
 *
 * Expect result:
 * The best mse from targeted function should be identical
 * with the mse from reference function.
 *
 * Test coverage:
 * Test cases:
 * bitdepth: 8, 10, 12
 * BlockSize: {BLOCK_4X4, BLOCK_4X8, BLOCK_8X4, BLOCK_8X8}
 * Pli: 0, 1, 2
 *
 */

using ComputeCdefDist16BitFunc =
    uint64_t (*)(const uint16_t *dst, int32_t dstride, const uint16_t *src,
                 const CdefList *dlist, int32_t cdef_count, BlockSize bsize,
                 int32_t coeff_shift, uint8_t subsampling_factor);

class CDEFComputeCdefDist16Bit
    : public testing::TestWithParam<ComputeCdefDist16BitFunc> {
  public:
    CDEFComputeCdefDist16Bit() : test_func_(GetParam()) {
    }

    void test_match() {
        const int stride = 1 << MAX_SB_SIZE_LOG2;
        const int buf_size = 1 << (MAX_SB_SIZE_LOG2 * 2);
        DECLARE_ALIGNED(32, uint16_t, src_data_[buf_size]);
        DECLARE_ALIGNED(32, uint16_t, dst_data_[buf_size]);

        // compute cdef list
        for (int bd = 8; bd <= 12; ++bd) {
            // prepare src data
            SVTRandom rnd_(bd, false);
            for (int i = 0; i < buf_size; ++i) {
                src_data_[i] = rnd_.random();
                dst_data_[i] = rnd_.random();
            }

            const int coeff_shift = bd - 8;
            SVTRandom skip_rnd_(0, 1);
            for (int k = 0; k < 100; ++k) {
                CdefList dlist[MI_SIZE_128X128 * MI_SIZE_128X128];
                int cdef_count = 0;

                // generate the cdef list randomly
                for (int r = 0; r < MI_SIZE_128X128; r += 2) {
                    for (int c = 0; c < MI_SIZE_128X128; c += 2) {
                        // append non-skip block into dlist
                        if (!skip_rnd_.random()) {
                            dlist[cdef_count].by = (uint8_t)(r >> 1);
                            dlist[cdef_count].bx = (uint8_t)(c >> 1);
                            ++cdef_count;
                        }
                    }
                }

                const BlockSize test_bs[] = {
                    BLOCK_4X4, BLOCK_4X8, BLOCK_8X4, BLOCK_8X8};
                for (int i = 0; i < 4; ++i) {
                    for (int plane = 0; plane < 3; ++plane) {
                        // Allowable subsampling values are: 1, 2
                        for (uint8_t subsampling = 1; subsampling <= 2;
                             subsampling <<= 1) {
                            // Subsampling factor can only be 1 for 4x4 blocks.
                            if (i == 0 && subsampling == 2) {
                                continue;
                            }
                            const uint64_t ref_mse =
                                svt_aom_compute_cdef_dist_16bit_c(dst_data_,
                                                                  stride,
                                                                  src_data_,
                                                                  dlist,
                                                                  cdef_count,
                                                                  test_bs[i],
                                                                  coeff_shift,
                                                                  subsampling);

                            const uint64_t test_mse = test_func_(dst_data_,
                                                                 stride,
                                                                 src_data_,
                                                                 dlist,
                                                                 cdef_count,
                                                                 test_bs[i],
                                                                 coeff_shift,
                                                                 subsampling);
                            ASSERT_EQ(ref_mse, test_mse)
                                << "svt_aom_compute_cdef_dist_16bit_opt failed "
                                << "bitdepth: " << bd << " plane: " << plane
                                << " BlockSize " << test_bs[i]
                                << " loop: " << k;
                        }
                    }
                }
            }
        }
    }

  private:
    ComputeCdefDist16BitFunc test_func_;
};

TEST_P(CDEFComputeCdefDist16Bit, test_match) {
    test_match();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, CDEFComputeCdefDist16Bit,
    ::testing::Values(svt_aom_compute_cdef_dist_16bit_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFComputeCdefDist16Bit,
    ::testing::Values(svt_aom_compute_cdef_dist_16bit_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFComputeCdefDist16Bit,
    ::testing::Values(svt_aom_compute_cdef_dist_16bit_neon));

#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, CDEFComputeCdefDist16Bit,
    ::testing::Values(svt_aom_compute_cdef_dist_16bit_sve));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64

using ComputeCdefDist8BitFunc =
    uint64_t (*)(const uint8_t *dst8, int32_t dstride, const uint8_t *src8,
                 const CdefList *dlist, int32_t cdef_count, BlockSize bsize,
                 int32_t coeff_shift, uint8_t subsampling_factor);

class CDEFComputeCdefDist8BitTest
    : public ::testing::TestWithParam<ComputeCdefDist8BitFunc> {
  public:
    CDEFComputeCdefDist8BitTest() : test_func_(GetParam()) {
    }

    void test_match() {
        const int stride = 1 << MAX_SB_SIZE_LOG2;
        const int buf_size = 1 << (MAX_SB_SIZE_LOG2 * 2);
        DECLARE_ALIGNED(32, uint8_t, src_data_[buf_size]);
        DECLARE_ALIGNED(32, uint8_t, dst_data_[buf_size]);

        // compute cdef list
        for (int bd = 8; bd <= 12; ++bd) {
            // prepare src data
            SVTRandom rnd_(bd, false);
            for (int i = 0; i < buf_size; ++i) {
                src_data_[i] = rnd_.random() % 255;
                dst_data_[i] = rnd_.random() % 255;
            }

            const int coeff_shift = bd - 8;
            SVTRandom skip_rnd_(0, 1);
            for (int k = 0; k < 10; ++k) {
                CdefList dlist[MI_SIZE_128X128 * MI_SIZE_128X128];
                int cdef_count = 0;

                // generate the cdef list randomly
                for (int r = 0; r < MI_SIZE_128X128; r += 2) {
                    for (int c = 0; c < MI_SIZE_128X128; c += 2) {
                        // append non-skip block into dlist
                        if (!skip_rnd_.random()) {
                            dlist[cdef_count].by = (uint8_t)(r >> 1);
                            dlist[cdef_count].bx = (uint8_t)(c >> 1);
                            ++cdef_count;
                        }
                    }
                }

                const BlockSize test_bs[] = {
                    BLOCK_4X4, BLOCK_4X8, BLOCK_8X4, BLOCK_8X8};
                for (int i = 0; i < 4; ++i) {
                    for (int plane = 0; plane < 3; ++plane) {
                        // Allowable subsampling values are: 1, 2
                        for (uint8_t subsampling = 1; subsampling <= 2;
                             subsampling <<= 1) {
                            // Subsampling factor can only be 1 for 4x4 blocks.
                            if (i == 0 && subsampling == 2) {
                                continue;
                            }
                            const uint64_t ref_mse =
                                svt_aom_compute_cdef_dist_8bit_c(dst_data_,
                                                                 stride,
                                                                 src_data_,
                                                                 dlist,
                                                                 cdef_count,
                                                                 test_bs[i],
                                                                 coeff_shift,
                                                                 subsampling);

                            const uint64_t test_mse = test_func_(dst_data_,
                                                                 stride,
                                                                 src_data_,
                                                                 dlist,
                                                                 cdef_count,
                                                                 test_bs[i],
                                                                 coeff_shift,
                                                                 subsampling);
                            ASSERT_EQ(ref_mse, test_mse)
                                << "svt_aom_compute_cdef_dist_8bit_opt failed "
                                << "bitdepth: " << bd << " plane: " << plane
                                << " BlockSize " << test_bs[i]
                                << " loop: " << k;
                        }
                    }
                }
            }
        }
    }

  private:
    ComputeCdefDist8BitFunc test_func_;
};

TEST_P(CDEFComputeCdefDist8BitTest, test_match) {
    test_match();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, CDEFComputeCdefDist8BitTest,
    ::testing::Values(svt_aom_compute_cdef_dist_8bit_sse4_1));

INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFComputeCdefDist8BitTest,
    ::testing::Values(svt_aom_compute_cdef_dist_8bit_avx2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFComputeCdefDist8BitTest,
    ::testing::Values(svt_aom_compute_cdef_dist_8bit_neon));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, CDEFComputeCdefDist8BitTest,
    ::testing::Values(svt_aom_compute_cdef_dist_8bit_neon_dotprod));
#endif  // HAVE_NEON_DOTPROD
#endif  // ARCH_AARCH64

/**
 * @brief Unit test for svt_search_one_dual
 *
 * Test strategy:
 * Prepare the mse array filled randomly and check the best mse, best
 * strength for luma and chroma returned from refrence
 * function and targeted function.
 *
 * Expect result:
 * The best mse and strengths from targeted function should be identical
 * with the values from reference function.
 *
 * Test coverage:
 * Test cases:
 * nb_strength: [0 8)
 * end_gi: [2, 3, 4, 6, 48, TOTAL_STRENGTHS]
 *
 */

const int end_gi_sizes[6] = {2, 3, 4, 6, 48, TOTAL_STRENGTHS};
const int sb_count_v[2] = {100, 101};

typedef uint64_t (*SearchOneDualFunc)(int *lev0, int *lev1, int nb_strengths,
                                      uint64_t **mse[2], int sb_count,
                                      int start_gi, int end_gi);
typedef ::testing::tuple<SearchOneDualFunc, int, int> SearchOneDualParam;

class CDEFSearchOneDualTest
    : public ::testing::TestWithParam<SearchOneDualParam> {
  public:
    CDEFSearchOneDualTest()
        : test_func_(TEST_GET_PARAM(0)),
          sb_count_(TEST_GET_PARAM(1)),
          end_gi_(TEST_GET_PARAM(2)) {
    }

    void SetUp() override {
        mse_[0] =
            (uint64_t **)svt_aom_memalign(32, sizeof(*mse_[0]) * sb_count_);
        mse_[1] =
            (uint64_t **)svt_aom_memalign(32, sizeof(*mse_[1]) * sb_count_);
        for (int i = 0; i < sb_count_; i++) {
            mse_[0][i] =
                (uint64_t *)svt_aom_memalign(32, sizeof(**mse_[0]) * 64);
            mse_[1][i] =
                (uint64_t *)svt_aom_memalign(32, sizeof(**mse_[1]) * 64);
        }
    }

    void TearDown() override {
        for (int i = 0; i < sb_count_; i++) {
            svt_aom_free(mse_[0][i]);
            svt_aom_free(mse_[1][i]);
        }
        svt_aom_free(mse_[0]);
        svt_aom_free(mse_[1]);
    }

    void RunTest(int num_loop) {
        // setup environment
        const int start_gi = 0;
        const int nb_strengths = 8;
        int lvl_luma_ref[CDEF_MAX_STRENGTHS],
            lvl_chroma_ref[CDEF_MAX_STRENGTHS];
        int lvl_luma_tst[CDEF_MAX_STRENGTHS],
            lvl_chroma_tst[CDEF_MAX_STRENGTHS];
        SVTRandom rnd_(10, false);

        // generate mse randomly
        for (int i = 0; i < 2; ++i)
            for (int n = 0; n < sb_count_; ++n)
                for (int j = 0; j < 64; ++j)
                    mse_[i][n][j] = rnd_.random();

        memset(lvl_luma_ref, 0, sizeof(lvl_luma_ref));
        memset(lvl_chroma_ref, 0, sizeof(lvl_chroma_ref));
        memset(lvl_luma_tst, 0, sizeof(lvl_luma_tst));
        memset(lvl_chroma_tst, 0, sizeof(lvl_chroma_tst));

        for (int j = 0; j < nb_strengths; ++j) {
            uint64_t best_mse_ref, best_mse_tst;
            double time_c, time_o;
            uint64_t start_time_seconds, start_time_useconds;
            uint64_t middle_time_seconds, middle_time_useconds;
            uint64_t finish_time_seconds, finish_time_useconds;

            svt_av1_get_time(&start_time_seconds, &start_time_useconds);

            for (int k = 0; k < num_loop; k++) {
                best_mse_ref = svt_search_one_dual_c(lvl_luma_ref,
                                                     lvl_chroma_ref,
                                                     j,
                                                     mse_,
                                                     sb_count_,
                                                     start_gi,
                                                     end_gi_);
            }

            svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

            for (int k = 0; k < num_loop; k++) {
                best_mse_tst = test_func_(lvl_luma_tst,
                                          lvl_chroma_tst,
                                          j,
                                          mse_,
                                          sb_count_,
                                          start_gi,
                                          end_gi_);
            }

            svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);

            if (num_loop > 1) {
                time_c = svt_av1_compute_overall_elapsed_time_ms(
                    start_time_seconds,
                    start_time_useconds,
                    middle_time_seconds,
                    middle_time_useconds);
                time_o = svt_av1_compute_overall_elapsed_time_ms(
                    middle_time_seconds,
                    middle_time_useconds,
                    finish_time_seconds,
                    finish_time_useconds);

                printf("Average Nanoseconds per Function Call\n");
                printf("    svt_search_one_dual_c()       : %6.2f\n",
                       1000000 * time_c / num_loop);
                printf(
                    "    svt_search_one_dual_opt(strength: %d) : %6.2f   "
                    "(Comparison: "
                    "%5.2fx)\n",
                    j,
                    1000000 * time_o / num_loop,
                    time_c / time_o);
            } else {
                ASSERT_EQ(best_mse_tst, best_mse_ref)
                    << "svt_search_one_dual_opt return different best mse"
                    << std::endl
                    << "nb_strength: " << j << ", end_gi: " << end_gi_
                    << ", sb_count: " << sb_count_;
                for (int h = 0; h < CDEF_MAX_STRENGTHS; ++h) {
                    ASSERT_EQ(lvl_luma_ref[h], lvl_luma_tst[h])
                        << "best strength for luma does not match" << std::endl
                        << "nb_strength: " << j << ", end_gi: " << end_gi_
                        << ", sb_count: " << sb_count_ << ", pos " << h;
                    ASSERT_EQ(lvl_chroma_ref[h], lvl_chroma_tst[h])
                        << "best strength for chroma does not match"
                        << std::endl
                        << "nb_strength: " << j << ", end_gi: " << end_gi_
                        << ", sb_count: " << sb_count_ << ", pos " << h;
                }
            }
        }
    }

  private:
    SearchOneDualFunc test_func_;
    int sb_count_, end_gi_;
    uint64_t **mse_[2];
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(CDEFSearchOneDualTest);

TEST_P(CDEFSearchOneDualTest, test_match) {
    RunTest(1);
}

TEST_P(CDEFSearchOneDualTest, DISABLED_test_speed) {
    RunTest(10000);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, CDEFSearchOneDualTest,
    ::testing::Combine(::testing::Values(svt_search_one_dual_avx2),
                       ::testing::ValuesIn(sb_count_v),
                       ::testing::ValuesIn(end_gi_sizes)));

#if EN_AVX512_SUPPORT
INSTANTIATE_TEST_SUITE_P(
    AVX512, CDEFSearchOneDualTest,
    ::testing::Combine(::testing::Values(svt_search_one_dual_avx512),
                       ::testing::ValuesIn(sb_count_v),
                       ::testing::ValuesIn(end_gi_sizes)));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, CDEFSearchOneDualTest,
    ::testing::Combine(::testing::Values(svt_search_one_dual_neon),
                       ::testing::ValuesIn(sb_count_v),
                       ::testing::ValuesIn(end_gi_sizes)));
#endif  // ARCH_AARCH64
