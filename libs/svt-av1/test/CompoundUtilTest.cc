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
 * @file CompoundUtilTest.cc
 *
 * @brief Unit test for util functions in wedge prediction:
 * - av1_build_compound_diffwtd_mask_{, d16}avx2
 * - svt_aom_blend_a64_mask_avx2/svt_aom_lowbd_blend_a64_d16_mask_avx2
 * - svt_aom_blend_a64_mask_sse4_1
 * -
 *svt_aom_highbd_blend_a64_mask_8bit_sse4_1/svt_aom_highbd_blend_a64_d16_mask_avx2
 * - svt_aom_blend_a64_hmask_sse4_1/svt_aom_blend_a64_vmask_sse4_1
 * -
 *svt_aom_highbd_blend_a64_hmask_16bit_sse4_1/svt_aom_highbd_blend_a64_vmask_16bit_sse4_1
 * - svt_aom_sse_avx2/svt_aom_highbd_sse_avx2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include "gtest/gtest.h"
#include "definitions.h"
#include "utility.h"
#include "aom_dsp_rtcd.h"
#include "random.h"
#include "convolve.h"
#include "util.h"
#include "enc_inter_prediction.h"

using std::make_tuple;
using svt_av1_test_tool::SVTRandom;

#define MAX_MASK_SQUARE (4 * MAX_SB_SQUARE)
#define MAKE_PARAM(func_type) std::tuple<func_type, func_type>

namespace {
template <typename SrcSample, typename DstSample, typename BlendFunc,
          typename BlendTestParam>
class CompBlendTest : public ::testing::TestWithParam<BlendTestParam> {
  public:
    CompBlendTest()
        : func_ref_(nullptr),
          func_tst_(nullptr),
          no_sub_(false),
          is_d16_(false) {
    }

    void SetUp() override {
        ref_dst_ = (DstSample *)svt_aom_memalign(
            32, MAX_SB_SQUARE * sizeof(DstSample));
        tst_dst_ = (DstSample *)svt_aom_memalign(
            32, MAX_SB_SQUARE * sizeof(DstSample));
        src0_ = (SrcSample *)svt_aom_memalign(
            32, MAX_SB_SQUARE * sizeof(SrcSample));
        src1_ = (SrcSample *)svt_aom_memalign(
            32, MAX_SB_SQUARE * sizeof(SrcSample));
        mask_ = (uint8_t *)svt_aom_memalign(32, MAX_MASK_SQUARE);
    }

    void TearDown() override {
        svt_aom_free(ref_dst_);
        svt_aom_free(tst_dst_);
        svt_aom_free(src0_);
        svt_aom_free(src1_);
        svt_aom_free(mask_);
    }

    void run_test() {
        const int iterations = 1000;
        /*
          // max number of bits used by the source for d16 blends is as follows
          (from libaom): static const int kSrcMaxBitsMask = (1 << 14) - 1;
          static const int kSrcMaxBitsMaskHBD = (1 << 16) - 1;
        */
        int max = is_d16_ ? (bd_ == 8 ? 14 : 16) : bd_;
        SVTRandom rnd(0, (1 << max) - 1);
        SVTRandom mask_rnd(0, 64);

        // generate random mask
        for (int i = 0; i < MAX_MASK_SQUARE; ++i)
            mask_[i] = mask_rnd.random();

        for (int k = 0; k < iterations; ++k) {
            for (int block_size = BLOCK_4X4; block_size < BLOCK_SIZES_ALL;
                 block_size += 1) {
                w_ = block_size_wide[block_size];
                h_ = block_size_high[block_size];

                // initial the input data
                for (int i = 0; i < h_; ++i) {
                    for (int j = 0; j < w_; ++j) {
                        src0_[i * src_stride_ + j] = rnd.random();
                        src1_[i * src_stride_ + j] = rnd.random();
                    }
                }

                int submax = 2;
                if (no_sub_)
                    submax = 1;
                for (int subh = 0; subh < submax; ++subh) {
                    for (int subw = 0; subw < submax; ++subw) {
                        memset(ref_dst_, 0, MAX_SB_SQUARE * sizeof(DstSample));
                        memset(tst_dst_, 0, MAX_SB_SQUARE * sizeof(DstSample));

                        run_blend(subw, subh);

                        // check output
                        for (int i = 0; i < h_; ++i) {
                            for (int j = 0; j < w_; ++j) {
                                ASSERT_EQ(tst_dst_[i * dst_stride_ + j],
                                          ref_dst_[i * dst_stride_ + j])
                                    << " Pixel mismatch at index "
                                    << "[" << j << "x" << i
                                    << "]\nblock size: " << block_size
                                    << "\nbit-depth: " << bd_
                                    << "\niterator: " << k;
                            }
                        }
                    }
                }
            }
        }
    }

    virtual void run_blend(int subw, int subh) = 0;

  protected:
    static const int src_stride_ = MAX_SB_SIZE;
    static const int dst_stride_ = MAX_SB_SIZE;
    static const int mask_stride_ = 2 * MAX_SB_SIZE;
    DstSample *ref_dst_;
    DstSample *tst_dst_;
    SrcSample *src0_;
    SrcSample *src1_;
    BlendFunc func_ref_;
    BlendFunc func_tst_;
    uint8_t *mask_;
    int w_, h_;
    int bd_;
    const char *tst_fn_name;  // test function name
    bool no_sub_;
    bool is_d16_;
};

using LbdBlendA64MaskFunc = void (*)(uint8_t *, uint32_t, const uint8_t *,
                                     uint32_t, const uint8_t *, uint32_t,
                                     const uint8_t *, uint32_t, int, int, int,
                                     int);

class LbdCompBlendTest
    : public CompBlendTest<uint8_t, uint8_t, LbdBlendA64MaskFunc,
                           MAKE_PARAM(LbdBlendA64MaskFunc)> {
  public:
    LbdCompBlendTest() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
    }

    void run_blend(int subw, int subh) override {
        func_ref_(ref_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh);
        func_tst_(tst_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh);
    }
};

TEST_P(LbdCompBlendTest, BlendA64Mask) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, LbdCompBlendTest,
                         ::testing::ValuesIn({
                             make_tuple(svt_aom_blend_a64_mask_c,
                                        svt_aom_blend_a64_mask_sse4_1),
                         }));

INSTANTIATE_TEST_SUITE_P(AVX2, LbdCompBlendTest,
                         ::testing::ValuesIn({
                             make_tuple(svt_aom_blend_a64_mask_c,
                                        svt_aom_blend_a64_mask_avx2),
                         }));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, LbdCompBlendTest,
                         ::testing::ValuesIn({
                             make_tuple(svt_aom_blend_a64_mask_c,
                                        svt_aom_blend_a64_mask_neon),
                         }));
#endif  // ARCH_AARCH64

using LbdBlendA64D16MaskFunc = void (*)(uint8_t *, uint32_t,
                                        const CONV_BUF_TYPE *, uint32_t,
                                        const CONV_BUF_TYPE *, uint32_t,
                                        const uint8_t *, uint32_t, int, int,
                                        int, int, ConvolveParams *);

class LbdCompBlendD16Test
    : public CompBlendTest<uint16_t, uint8_t, LbdBlendA64D16MaskFunc,
                           MAKE_PARAM(LbdBlendA64D16MaskFunc)> {
  public:
    LbdCompBlendD16Test() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
        is_d16_ = true;
    }

    void run_blend(int subw, int subh) override {
        ConvolveParams conv_params;
        conv_params.round_0 = ROUND0_BITS;
        conv_params.round_1 = COMPOUND_ROUND1_BITS;
        func_tst_(tst_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh,
                  &conv_params);
        func_ref_(ref_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh,
                  &conv_params);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(LbdCompBlendD16Test);

TEST_P(LbdCompBlendD16Test, BlendA64MaskD16) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, LbdCompBlendD16Test,
    ::testing::ValuesIn({make_tuple(svt_aom_lowbd_blend_a64_d16_mask_c,
                                    svt_aom_lowbd_blend_a64_d16_mask_sse4_1)}));
INSTANTIATE_TEST_SUITE_P(
    AVX2, LbdCompBlendD16Test,
    ::testing::ValuesIn({make_tuple(svt_aom_lowbd_blend_a64_d16_mask_c,
                                    svt_aom_lowbd_blend_a64_d16_mask_avx2)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LbdCompBlendD16Test,
    ::testing::ValuesIn({make_tuple(svt_aom_lowbd_blend_a64_d16_mask_c,
                                    svt_aom_lowbd_blend_a64_d16_mask_neon)}));
#endif  // ARCH_AARCH64

using LbdBlendA64HMaskFunc = void (*)(uint8_t *, uint32_t, const uint8_t *,
                                      uint32_t, const uint8_t *, uint32_t,
                                      const uint8_t *, int, int);

class LbdCompBlendHMaskTest
    : public CompBlendTest<uint8_t, uint8_t, LbdBlendA64HMaskFunc,
                           MAKE_PARAM(LbdBlendA64HMaskFunc)> {
  public:
    LbdCompBlendHMaskTest() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
        no_sub_ = true;
    }

    void run_blend(int subw, int subh) override {
        (void)subw;
        (void)subh;

        if (w_ > 32)
            return;
        const uint8_t *const mask = svt_av1_get_obmc_mask(w_);

        func_ref_(ref_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_);
        func_tst_(tst_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_);
    }
};

TEST_P(LbdCompBlendHMaskTest, BlendA64Mask) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, LbdCompBlendHMaskTest,
    ::testing::ValuesIn({make_tuple(svt_aom_blend_a64_hmask_c,
                                    svt_aom_blend_a64_hmask_sse4_1)}));

INSTANTIATE_TEST_SUITE_P(
    AVX2, LbdCompBlendHMaskTest,
    ::testing::ValuesIn({make_tuple(svt_aom_blend_a64_hmask_c,
                                    svt_av1_blend_a64_hmask_avx2)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LbdCompBlendHMaskTest,
    ::testing::ValuesIn({make_tuple(svt_aom_blend_a64_hmask_c,
                                    svt_aom_blend_a64_hmask_neon)}));
#endif  // ARCH_AARCH64

using LbdBlendA64VMaskFunc = void (*)(uint8_t *, uint32_t, const uint8_t *,
                                      uint32_t, const uint8_t *, uint32_t,
                                      const uint8_t *, int, int);

class LbdCompBlendVMaskTest
    : public CompBlendTest<uint8_t, uint8_t, LbdBlendA64VMaskFunc,
                           MAKE_PARAM(LbdBlendA64VMaskFunc)> {
  public:
    LbdCompBlendVMaskTest() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
        no_sub_ = true;
    }

    void run_blend(int subw, int subh) override {
        (void)subw;
        (void)subh;

        if (h_ > 32)
            return;
        const uint8_t *const mask = svt_av1_get_obmc_mask(h_);

        func_ref_(ref_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_);
        func_tst_(tst_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_);
    }
};

TEST_P(LbdCompBlendVMaskTest, BlendA64Mask) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, LbdCompBlendVMaskTest,
    ::testing::ValuesIn({make_tuple(svt_aom_blend_a64_vmask_c,
                                    svt_aom_blend_a64_vmask_sse4_1)}));

INSTANTIATE_TEST_SUITE_P(
    AVX2, LbdCompBlendVMaskTest,
    ::testing::ValuesIn({make_tuple(svt_aom_blend_a64_vmask_c,
                                    svt_av1_blend_a64_vmask_avx2)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, LbdCompBlendVMaskTest,
    ::testing::ValuesIn({make_tuple(svt_aom_blend_a64_vmask_c,
                                    svt_aom_blend_a64_vmask_neon)}));
#endif  // ARCH_AARCH64

using HbdBlendA64MaskFunc = void (*)(uint8_t *, uint32_t, const uint8_t *,
                                     uint32_t, const uint8_t *, uint32_t,
                                     const uint8_t *, uint32_t, int, int, int,
                                     int, int);

class HbdCompBlendTest
    : public CompBlendTest<uint16_t, uint16_t, HbdBlendA64MaskFunc,
                           MAKE_PARAM(HbdBlendA64MaskFunc)> {
  public:
    HbdCompBlendTest() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
    }

    void run_hbd_test(uint8_t bd) {
        bd_ = bd;
        run_test();
    }

    void run_blend(int subw, int subh) override {
        func_ref_((uint8_t *)ref_dst_,
                  dst_stride_,
                  (uint8_t *)src0_,
                  src_stride_,
                  (uint8_t *)src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh,
                  bd_);
        func_tst_((uint8_t *)tst_dst_,
                  dst_stride_,
                  (uint8_t *)src0_,
                  src_stride_,
                  (uint8_t *)src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh,
                  bd_);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(HbdCompBlendTest);

TEST_P(HbdCompBlendTest, BlendA64Mask) {
    run_hbd_test(8);
    run_hbd_test(10);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, HbdCompBlendTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_mask_c,
                             svt_aom_highbd_blend_a64_mask_8bit_sse4_1)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HbdCompBlendTest,
    ::testing::ValuesIn({make_tuple(svt_aom_highbd_blend_a64_mask_c,
                                    svt_aom_highbd_blend_a64_mask_neon)}));
#endif  // ARCH_AARCH64

using HbdBlendA64D16MaskFunc = void (*)(uint8_t *, uint32_t,
                                        const CONV_BUF_TYPE *, uint32_t,
                                        const CONV_BUF_TYPE *, uint32_t,
                                        const uint8_t *, uint32_t, int, int,
                                        int, int, ConvolveParams *, const int);

class HbdCompBlendD16Test
    : public CompBlendTest<uint16_t, uint16_t, HbdBlendA64D16MaskFunc,
                           MAKE_PARAM(HbdBlendA64D16MaskFunc)> {
  public:
    HbdCompBlendD16Test() {
        bd_ = 10;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
        is_d16_ = true;
    }

    void run_hbd_test(uint8_t bd) {
        bd_ = bd;
        run_test();
    }

    void run_blend(int subw, int subh) override {
        ConvolveParams conv_params;
        conv_params.round_0 = ROUND0_BITS;
        conv_params.round_1 = COMPOUND_ROUND1_BITS;
        func_tst_((uint8_t *)tst_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh,
                  &conv_params,
                  bd_);
        func_ref_((uint8_t *)ref_dst_,
                  dst_stride_,
                  src0_,
                  src_stride_,
                  src1_,
                  src_stride_,
                  mask_,
                  mask_stride_,
                  w_,
                  h_,
                  subw,
                  subh,
                  &conv_params,
                  bd_);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(HbdCompBlendD16Test);

TEST_P(HbdCompBlendD16Test, BlendA64MaskD16) {
    run_hbd_test(8);
    run_hbd_test(10);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, HbdCompBlendD16Test,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_d16_mask_c,
                             svt_aom_highbd_blend_a64_d16_mask_sse4_1)}));

INSTANTIATE_TEST_SUITE_P(
    AVX2, HbdCompBlendD16Test,
    ::testing::ValuesIn({make_tuple(svt_aom_highbd_blend_a64_d16_mask_c,
                                    svt_aom_highbd_blend_a64_d16_mask_avx2)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, HbdCompBlendD16Test,
    ::testing::ValuesIn({make_tuple(svt_aom_highbd_blend_a64_d16_mask_c,
                                    svt_aom_highbd_blend_a64_d16_mask_neon)}));
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
using HbdBlendA64HMaskFunc = void (*)(uint16_t *, uint32_t, const uint16_t *,
                                      uint32_t, const uint16_t *, uint32_t,
                                      const uint8_t *, int, int, int);

class HbdCompBlendHMaskTest
    : public CompBlendTest<uint16_t, uint16_t, HbdBlendA64HMaskFunc,
                           MAKE_PARAM(HbdBlendA64HMaskFunc)> {
  public:
    HbdCompBlendHMaskTest() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
        no_sub_ = true;
    }

    void run_hbd_test(uint8_t bd) {
        bd_ = bd;
        run_test();
    }

    void run_blend(int subw, int subh) override {
        (void)subw;
        (void)subh;

        if (w_ > 32)
            return;
        const uint8_t *const mask = svt_av1_get_obmc_mask(w_);

        func_ref_((uint16_t *)ref_dst_,
                  dst_stride_,
                  (uint16_t *)src0_,
                  src_stride_,
                  (uint16_t *)src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_,
                  bd_);
        func_tst_((uint16_t *)tst_dst_,
                  dst_stride_,
                  (uint16_t *)src0_,
                  src_stride_,
                  (uint16_t *)src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_,
                  bd_);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(HbdCompBlendHMaskTest);

TEST_P(HbdCompBlendHMaskTest, BlendA64Mask) {
    run_hbd_test(8);
    run_hbd_test(10);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, HbdCompBlendHMaskTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_hmask_16bit_c,
                             svt_aom_highbd_blend_a64_hmask_16bit_sse4_1)}));

INSTANTIATE_TEST_SUITE_P(AVX2, HbdCompBlendHMaskTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_hmask_16bit_c,
                             svt_av1_highbd_blend_a64_hmask_16bit_avx2)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, HbdCompBlendHMaskTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_hmask_16bit_c,
                             svt_aom_highbd_blend_a64_hmask_16bit_neon)}));
#endif  // ARCH_AARCH64

using HbdBlendA64VMaskFunc = void (*)(uint16_t *, uint32_t, const uint16_t *,
                                      uint32_t, const uint16_t *, uint32_t,
                                      const uint8_t *, int, int, int);

class HbdCompBlendVMaskTest
    : public CompBlendTest<uint16_t, uint16_t, HbdBlendA64VMaskFunc,
                           MAKE_PARAM(HbdBlendA64VMaskFunc)> {
  public:
    HbdCompBlendVMaskTest() {
        bd_ = 8;
        func_ref_ = TEST_GET_PARAM(0);
        func_tst_ = TEST_GET_PARAM(1);
        no_sub_ = true;
    }

    void run_hbd_test(uint8_t bd) {
        bd_ = bd;
        run_test();
    }

    void run_blend(int subw, int subh) override {
        (void)subw;
        (void)subh;

        if (h_ > 32)
            return;
        const uint8_t *const mask = svt_av1_get_obmc_mask(h_);

        func_ref_((uint16_t *)ref_dst_,
                  dst_stride_,
                  (uint16_t *)src0_,
                  src_stride_,
                  (uint16_t *)src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_,
                  bd_);
        func_tst_((uint16_t *)tst_dst_,
                  dst_stride_,
                  (uint16_t *)src0_,
                  src_stride_,
                  (uint16_t *)src1_,
                  src_stride_,
                  mask,
                  w_,
                  h_,
                  bd_);
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(HbdCompBlendVMaskTest);

TEST_P(HbdCompBlendVMaskTest, BlendA64Mask) {
    run_hbd_test(8);
    run_hbd_test(10);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(SSE4_1, HbdCompBlendVMaskTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_vmask_16bit_c,
                             svt_aom_highbd_blend_a64_vmask_16bit_sse4_1)}));

INSTANTIATE_TEST_SUITE_P(AVX2, HbdCompBlendVMaskTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_vmask_16bit_c,
                             svt_av1_highbd_blend_a64_vmask_16bit_avx2)}));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(NEON, HbdCompBlendVMaskTest,
                         ::testing::ValuesIn({make_tuple(
                             svt_aom_highbd_blend_a64_vmask_16bit_c,
                             svt_aom_highbd_blend_a64_vmask_16bit_neon)}));
#endif  // ARCH_AARCH64

#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

typedef void (*BuildCompDiffwtdMaskedFunc)(uint8_t *mask,
                                           DIFFWTD_MASK_TYPE mask_type,
                                           const uint8_t *src0, int src0_stride,
                                           const uint8_t *src1, int src1_stride,
                                           int h, int w);
typedef ::testing::tuple<BlockSize, BuildCompDiffwtdMaskedFunc>
    BuildCompDiffwtdMaskParam;

class BuildCompDiffwtdMaskTest
    : public ::testing::TestWithParam<BuildCompDiffwtdMaskParam> {
  public:
    BuildCompDiffwtdMaskTest() : rnd_(0, 255) {};
    virtual ~BuildCompDiffwtdMaskTest() {
    }

    void TearDown() override {
    }

    void run_test(const DIFFWTD_MASK_TYPE type) {
        const int block_size = TEST_GET_PARAM(0);
        BuildCompDiffwtdMaskedFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, uint8_t, mask_ref[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, mask_test[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, src0[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, src1[MAX_SB_SQUARE]);
        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            for (int j = 0; j < width * height; j++) {
                src0[j] = rnd_.random();
                src1[j] = rnd_.random();
            }

            svt_av1_build_compound_diffwtd_mask_c(
                mask_ref, type, src0, width, src1, width, height, width);

            test_impl(mask_test, type, src0, width, src1, width, height, width);
            for (int r = 0; r < height; ++r) {
                for (int c = 0; c < width; ++c) {
                    ASSERT_EQ(mask_ref[c + r * width], mask_test[c + r * width])
                        << "[" << r << "," << c << "] " << i << " @ " << width
                        << "x" << height << " inv " << type;
                }
            }
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(BuildCompDiffwtdMaskTest, MatchTest) {
    run_test(DIFFWTD_38);
    run_test(DIFFWTD_38_INV);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, BuildCompDiffwtdMaskTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, BuildCompDiffwtdMaskTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, BuildCompDiffwtdMaskTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_neon)));
#endif  // ARCH_AARCH64

typedef void (*BuildCompDiffwtdMaskedHighbdFunc)(
    uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t *src0,
    int src0_stride, const uint8_t *src1, int src1_stride, int h, int w,
    int bd);
typedef ::testing::tuple<BlockSize, BuildCompDiffwtdMaskedHighbdFunc>
    BuildCompDiffwtdMaskHighbdParam;

class BuildCompDiffwtdMaskHighbdTest
    : public ::testing::TestWithParam<BuildCompDiffwtdMaskHighbdParam> {
  public:
    BuildCompDiffwtdMaskHighbdTest() : rnd_(0, 255) {};
    virtual ~BuildCompDiffwtdMaskHighbdTest() {
    }

    void TearDown() override {
    }

    void run_test(const DIFFWTD_MASK_TYPE type, int bd) {
        const int block_size = TEST_GET_PARAM(0);
        BuildCompDiffwtdMaskedHighbdFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, uint8_t, mask_ref[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, mask_test[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint16_t, src0[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint16_t, src1[MAX_SB_SQUARE]);
        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            for (int j = 0; j < width * height; j++) {
                src0[j] = rnd_.random();
                src1[j] = rnd_.random();
            }

            svt_av1_build_compound_diffwtd_mask_highbd_c(mask_ref,
                                                         type,
                                                         (uint8_t *)src0,
                                                         width,
                                                         (uint8_t *)src1,
                                                         width,
                                                         height,
                                                         width,
                                                         bd);

            test_impl(mask_test,
                      type,
                      (uint8_t *)src0,
                      width,
                      (uint8_t *)src1,
                      width,
                      height,
                      width,
                      bd);
            for (int r = 0; r < height; ++r) {
                for (int c = 0; c < width; ++c) {
                    ASSERT_EQ(mask_ref[c + r * width], mask_test[c + r * width])
                        << "[" << r << "," << c << "] " << i << " @ " << width
                        << "x" << height << " inv " << type;
                }
            }
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(BuildCompDiffwtdMaskHighbdTest, MatchTest) {
    run_test(DIFFWTD_38, 8);
    run_test(DIFFWTD_38_INV, 8);
    run_test(DIFFWTD_38, 10);
    run_test(DIFFWTD_38_INV, 10);
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSSE3, BuildCompDiffwtdMaskHighbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_highbd_ssse3)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, BuildCompDiffwtdMaskHighbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_highbd_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, BuildCompDiffwtdMaskHighbdTest,
    ::testing::Combine(
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_highbd_neon)));
#endif  // ARCH_AARCH64

// test svt_av1_build_compound_diffwtd_mask_d16_avx2
typedef void (*BuildCompDiffwtdMaskD16Func)(
    uint8_t *mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE *src0,
    int src0_stride, const CONV_BUF_TYPE *src1, int src1_stride, int h, int w,
    ConvolveParams *conv_params, int bd);

typedef ::testing::tuple<int, BuildCompDiffwtdMaskD16Func, BlockSize>
    BuildCompDiffwtdMaskD16Param;

class BuildCompDiffwtdMaskD16Test
    : public ::testing::TestWithParam<BuildCompDiffwtdMaskD16Param> {
  public:
    BuildCompDiffwtdMaskD16Test() : rnd_(16, false) {
    }

    ~BuildCompDiffwtdMaskD16Test() {
    }

    void TearDown() override {
    }

  protected:
    void run_test() {
        BuildCompDiffwtdMaskD16Func tst_func = TEST_GET_PARAM(1);
        const int block_size = TEST_GET_PARAM(2);
        const int bd = TEST_GET_PARAM(0);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, uint8_t, mask_ref[2 * MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, mask_test[2 * MAX_SB_SQUARE]);
        DECLARE_ALIGNED(32, uint16_t, src0[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(32, uint16_t, src1[MAX_SB_SQUARE]);

        ConvolveParams conv_params =
            get_conv_params_no_round(0 /*unused*/, 0, 0, NULL, 0, 1, bd);

        int in_precision = bd + 2 * FILTER_BITS - conv_params.round_0 -
                           conv_params.round_1 + 2;

        for (int i = 0; i < MAX_SB_SQUARE; i++) {
            src0[i] = rnd_.random() & ((1 << in_precision) - 1);
            src1[i] = rnd_.random() & ((1 << in_precision) - 1);
        }

        for (int mask_type = 0; mask_type < DIFFWTD_MASK_TYPES; mask_type++) {
            svt_av1_build_compound_diffwtd_mask_d16_c(
                mask_ref,
                (DIFFWTD_MASK_TYPE)mask_type,
                src0,
                width,
                src1,
                width,
                height,
                width,
                &conv_params,
                bd);

            tst_func(mask_test,
                     (DIFFWTD_MASK_TYPE)mask_type,
                     src0,
                     width,
                     src1,
                     width,
                     height,
                     width,
                     &conv_params,
                     bd);

            for (int r = 0; r < height; ++r) {
                for (int c = 0; c < width; ++c) {
                    ASSERT_EQ(mask_ref[c + r * width], mask_test[c + r * width])
                        << "Mismatch at unit tests for "
                           "BuildCompDiffwtdMaskD16Test\n"
                        << " Pixel mismatch at index "
                        << "[" << r << "," << c << "] "
                        << " @ " << width << "x" << height << " inv "
                        << mask_type;
                }
            }
        }
    }
    SVTRandom rnd_;
};  // class BuildCompDiffwtdMaskD16Test

TEST_P(BuildCompDiffwtdMaskD16Test, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE4_1, BuildCompDiffwtdMaskD16Test,
    ::testing::Combine(
        ::testing::Range(8, 11, 2),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_d16_sse4_1),
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, BuildCompDiffwtdMaskD16Test,
    ::testing::Combine(
        ::testing::Range(8, 11, 2),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_d16_avx2),
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, BuildCompDiffwtdMaskD16Test,
    ::testing::Combine(
        ::testing::Range(8, 11, 2),
        ::testing::Values(svt_av1_build_compound_diffwtd_mask_d16_neon),
        ::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL)));
#endif  // ARCH_AARCH64

typedef int64_t (*AomSseFunc)(const uint8_t *, int, const uint8_t *, int, int,
                              int);
typedef ::testing::tuple<BlockSize, AomSseFunc> AomSseParam;

class AomSseTest : public ::testing::TestWithParam<AomSseParam> {
  public:
    AomSseTest() : rnd_(0, 255) {};
    virtual ~AomSseTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        AomSseFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, uint8_t, a_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, b_[MAX_SB_SQUARE]);
        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(a_, 0, sizeof(a_));
            memset(b_, 0, sizeof(b_));
            for (int j = 0; j < width * height; j++) {
                a_[j] = rnd_.random();
                b_[j] = rnd_.random();
            }

            int64_t res_ref =
                svt_aom_sse_c(a_, width, b_, width, height, width);
            int64_t res_tst = test_impl(a_, width, b_, width, height, width);

            ASSERT_EQ(res_ref, res_tst);
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(AomSseTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, AomSseTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_sse_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AomSseTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_sse_neon)));

#if HAVE_NEON_DOTPROD
INSTANTIATE_TEST_SUITE_P(
    NEON_DOTPROD, AomSseTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_sse_neon_dotprod)));
#endif  // HAVE_NEON_DOTPROD
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH

class AomSseHighbdTest : public ::testing::TestWithParam<AomSseParam> {
  public:
    AomSseHighbdTest() : rnd_(0, 255) {};
    virtual ~AomSseHighbdTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        AomSseFunc test_impl = TEST_GET_PARAM(1);
        int run_times = 100;
        int width;
        int height;
        if (block_size < BLOCK_SIZES_ALL) {
            width = block_size_wide[block_size];
            height = block_size_high[block_size];
        } else {
            run_times = 10;
            // unusual sizes
            if (block_size > BLOCK_SIZES_ALL) {
                // block_size == BLOCK_SIZES_ALL +1
                width = 36;
                height = 36;
            } else {
                // block_size == BLOCK_SIZES_ALL
                width = 40;
                height = 40;
            }
        }

        DECLARE_ALIGNED(16, uint16_t, a_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint16_t, b_[MAX_SB_SQUARE]);
        for (int i = 0; i < run_times; ++i) {
            memset(a_, 0, sizeof(a_));
            memset(b_, 0, sizeof(b_));
            for (int j = 0; j < width * height; j++) {
                a_[j] = rnd_.random();
                b_[j] = rnd_.random();
            }

            int64_t res_ref = svt_aom_highbd_sse_c(
                (uint8_t *)a_, width, (uint8_t *)b_, width, height, width);
            int64_t res_tst = test_impl(
                (uint8_t *)a_, width, (uint8_t *)b_, width, height, width);

            ASSERT_EQ(res_ref, res_tst)
                << "Mismatch: ref = " << res_ref << ", test = " << res_tst
                << ", width = " << width << ", height = " << height << "\n";
        }
    }

  private:
    SVTRandom rnd_;
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(AomSseHighbdTest);

TEST_P(AomSseHighbdTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    AVX2, AomSseHighbdTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_aom_highbd_sse_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AomSseHighbdTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_aom_highbd_sse_neon)));
#if HAVE_SVE
INSTANTIATE_TEST_SUITE_P(
    SVE, AomSseHighbdTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4,
                                        (BlockSize)(BLOCK_SIZES_ALL + 2)),
                       ::testing::Values(svt_aom_highbd_sse_sve)));
#endif  // HAVE_SVE
#endif  // ARCH_AARCH64
#endif  // CONFIG_ENABLE_HIGH_BIT_DEPTH

typedef void (*AomSubtractBlockFunc)(int, int, int16_t *, ptrdiff_t,
                                     const uint8_t *, ptrdiff_t,
                                     const uint8_t *, ptrdiff_t);
typedef ::testing::tuple<BlockSize, AomSubtractBlockFunc> AomSubtractBlockParam;

class AomSubtractBlockTest
    : public ::testing::TestWithParam<AomSubtractBlockParam> {
  public:
    AomSubtractBlockTest() : rnd_(0, 255) {};
    virtual ~AomSubtractBlockTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        AomSubtractBlockFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, int16_t, diff_ref_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, int16_t, diff_tst_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, src_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint8_t, pred_[MAX_SB_SQUARE]);
        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(src_, 0, sizeof(src_));
            memset(pred_, 0, sizeof(pred_));
            for (int j = 0; j < width * height; j++) {
                src_[j] = rnd_.random();
                pred_[j] = rnd_.random();
            }
            memset(diff_ref_, 0, sizeof(diff_ref_));
            memset(diff_tst_, 0, sizeof(diff_tst_));

            svt_aom_subtract_block_c(
                width, height, diff_ref_, width, src_, width, pred_, width);
            test_impl(
                width, height, diff_tst_, width, src_, width, pred_, width);

            ASSERT_EQ(0, memcmp(diff_ref_, diff_tst_, sizeof(diff_ref_)));
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(AomSubtractBlockTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, AomSubtractBlockTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_subtract_block_sse2)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, AomSubtractBlockTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_subtract_block_avx2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AomSubtractBlockTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_subtract_block_neon)));
#endif  // ARCH_AARCH64

#if CONFIG_ENABLE_HIGH_BIT_DEPTH

typedef void (*AomHighbdSubtractBlockFunc)(int, int, int16_t *, ptrdiff_t,
                                           const uint8_t *, ptrdiff_t,
                                           const uint8_t *, ptrdiff_t, int);
typedef ::testing::tuple<BlockSize, AomHighbdSubtractBlockFunc>
    AomHighbdSubtractBlockParam;

class AomHighbdSubtractBlockTest
    : public ::testing::TestWithParam<AomHighbdSubtractBlockParam> {
  public:
    AomHighbdSubtractBlockTest() : rnd_(0, 255) {};
    virtual ~AomHighbdSubtractBlockTest() {
    }

    void TearDown() override {
    }

    void run_test() {
        const int block_size = TEST_GET_PARAM(0);
        AomHighbdSubtractBlockFunc test_impl = TEST_GET_PARAM(1);
        const int width = block_size_wide[block_size];
        const int height = block_size_high[block_size];
        DECLARE_ALIGNED(16, int16_t, diff_ref_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, int16_t, diff_tst_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint16_t, src_[MAX_SB_SQUARE]);
        DECLARE_ALIGNED(16, uint16_t, pred_[MAX_SB_SQUARE]);
        const int run_times = 100;
        for (int i = 0; i < run_times; ++i) {
            memset(src_, 0, sizeof(src_));
            memset(pred_, 0, sizeof(pred_));
            for (int j = 0; j < width * height; j++) {
                src_[j] = rnd_.random();
                pred_[j] = rnd_.random();
            }
            memset(diff_ref_, 0, sizeof(diff_ref_));
            memset(diff_tst_, 0, sizeof(diff_tst_));

            svt_aom_highbd_subtract_block_c(width,
                                            height,
                                            diff_ref_,
                                            width,
                                            (uint8_t *)src_,
                                            width,
                                            (uint8_t *)pred_,
                                            width,
                                            8);  // last parameter is unused
            test_impl(width,
                      height,
                      diff_tst_,
                      width,
                      (uint8_t *)src_,
                      width,
                      (uint8_t *)pred_,
                      width,
                      8);

            ASSERT_EQ(0, memcmp(diff_ref_, diff_tst_, sizeof(diff_ref_)));
        }
    }

  private:
    SVTRandom rnd_;
};

TEST_P(AomHighbdSubtractBlockTest, MatchTest) {
    run_test();
}

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, AomHighbdSubtractBlockTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_highbd_subtract_block_sse2)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, AomHighbdSubtractBlockTest,
    ::testing::Combine(::testing::Range(BLOCK_4X4, BLOCK_SIZES_ALL),
                       ::testing::Values(svt_aom_highbd_subtract_block_neon)));
#endif  // ARCH_AARCH64

#endif

}  // namespace
