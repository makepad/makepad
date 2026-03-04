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
 * @file DeblockTest.cc
 *
 * @brief Unit test for cdef tools:
 * * svt_aom_lpf_{horizontal, vertical}_{4, 6, 8, 14}_sse2
 * * svt_aom_highbd_lpf_{horizontal, vertical}_{4, 6, 8, 14}_sse2
 *
 * @author Cidana-Wenyao
 *
 ******************************************************************************/
#include <cmath>
#include <cstdlib>
#include <string>

#include "gtest/gtest.h"
#include "definitions.h"
#include "aom_dsp_rtcd.h"
#include "random.h"
#include "util.h"
#include "utility.h"
#include "deblocking_filter.h"
#include "acm_random.h"
#include "dlf_sse2.h"
#include "deblocking_common.h"

using libaom_test::ACMRandom;
using ::testing::make_tuple;
namespace {

// define the common params
#define LOOP_PARAM \
    int p, const uint8_t *blimit, const uint8_t *limit, const uint8_t *thresh

// typedef the function type and test param
using LbdLoopFilterFunc = void (*)(uint8_t *s, LOOP_PARAM);
using HbdLoopFilterFunc = void (*)(uint16_t *s, LOOP_PARAM, int bd);

using HbdLpfTestParam =
    ::testing::tuple<HbdLoopFilterFunc, HbdLoopFilterFunc, int>;
using LdbLpfTestParam =
    ::testing::tuple<LbdLoopFilterFunc, LbdLoopFilterFunc, int>;

uint8_t get_outer_thresh(ACMRandom *rnd) {
    return static_cast<uint8_t>(rnd->PseudoUniform(3 * MAX_LOOP_FILTER + 5));
}

uint8_t get_inner_thresh(ACMRandom *rnd) {
    return static_cast<uint8_t>(rnd->PseudoUniform(MAX_LOOP_FILTER + 1));
}

uint8_t get_hev_thresh(ACMRandom *rnd) {
    return static_cast<uint8_t>(rnd->PseudoUniform(MAX_LOOP_FILTER + 1) >> 4);
}

/**
 * @brief Unit test for deblocking assembly functions
 *
 * Test strategy:
 * Feed src data generated randomly and all possible input,
 * then check the dst buffer from target function and reference
 * function.
 *
 * Expect result:
 * The src buffer modified by deblocking from targeted function
 * should be identical with the values from reference function.
 *
 * Test coverage:
 * Test cases:
 * blimit: [0, 3 * MAX_LOOP_FILTER + 4) as per spec 7.14
 * limit: [0, MAX_LOOP_FILTER]
 * hevThresh: [0, MAX_LOOP_FILTER] >> 4
 * bitdepth: 8, 10, 12
 *
 */
template <typename Sample, typename FuncType, typename TestParamType>
class LoopFilterTest : public ::testing::TestWithParam<TestParamType> {
  public:
    enum LpfType { SINGLE };
    virtual ~LoopFilterTest() {
    }

    void SetUp() override {
        lpf_tst_ = ::testing::get<0>(this->GetParam());
        lpf_ref_ = ::testing::get<1>(this->GetParam());
        bit_depth_ = ::testing::get<2>(this->GetParam());
        mask_ = (1 << bit_depth_) - 1;
    }

    void TearDown() override {
    }

    void init_buffer_with_value(uint8_t *buf, int length, uint8_t val) {
        for (int i = 0; i < length; ++i)
            buf[i] = val;
    }

    virtual void run_lpf(LOOP_PARAM, int bd) {
        (void)p;
        (void)blimit;
        (void)limit;
        (void)thresh;
        (void)bd;
    }

    void init_input(Sample *s, Sample *ref_s, ACMRandom *rnd,
                    const uint8_t limit, const int mask, const int32_t p,
                    const int i) {
        uint16_t tmp_s[kNumCoeffs];

        for (int j = 0; j < kNumCoeffs;) {
            const uint8_t val = rnd->Rand8();
            if (val & 0x80) {  // 50% chance to choose a new value.
                tmp_s[j] = rnd->Rand16();
                j++;
            } else {  // 50% chance to repeat previous value in row X times.
                int k = 0;
                while (k++ < ((val & 0x1f) + 1) && j < kNumCoeffs) {
                    if (j < 1) {
                        tmp_s[j] = rnd->Rand16();
                    } else if (val & 0x20) {  // Increment by a value within the
                                              // limit.
                        tmp_s[j] =
                            static_cast<uint16_t>(tmp_s[j - 1] + (limit - 1));
                    } else {  // Decrement by a value within the limit.
                        tmp_s[j] =
                            static_cast<uint16_t>(tmp_s[j - 1] - (limit - 1));
                    }
                    j++;
                }
            }
        }

        for (int j = 0; j < kNumCoeffs;) {
            const uint8_t val = rnd->Rand8();
            if (val & 0x80) {
                j++;
            } else {  // 50% chance to repeat previous value in column X times.
                int k = 0;
                while (k++ < ((val & 0x1f) + 1) && j < kNumCoeffs) {
                    if (j < 1) {
                        tmp_s[j] = rnd->Rand16();
                    } else if (val & 0x20) {  // Increment by a value within the
                                              // limit.
                        tmp_s[(j % 32) * 32 + j / 32] = static_cast<uint16_t>(
                            tmp_s[((j - 1) % 32) * 32 + (j - 1) / 32] +
                            (limit - 1));
                    } else {  // Decrement by a value within the limit.
                        tmp_s[(j % 32) * 32 + j / 32] = static_cast<uint16_t>(
                            tmp_s[((j - 1) % 32) * 32 + (j - 1) / 32] -
                            (limit - 1));
                    }
                    j++;
                }
            }
        }

        for (int j = 0; j < kNumCoeffs; j++) {
            if (i % 2) {
                s[j] = tmp_s[j] & mask;
            } else {
                s[j] = tmp_s[p * (j % p) + j / p] & mask;
            }
            ref_s[j] = s[j];
        }
    }

    void run_test() {
        ACMRandom rnd(ACMRandom::DeterministicSeed());
        const int count_test_block = 10000;
        const int32_t p = kNumCoeffs / 32;
        DECLARE_ALIGNED(16, Sample, tst_s[kNumCoeffs]);
        DECLARE_ALIGNED(16, Sample, ref_s[kNumCoeffs]);
        int err_count_total = 0;
        int first_failure = -1;
        start_tst_ = tst_s + 8 + p * 8;
        start_ref_ = ref_s + 8 + p * 8;
        for (int i = 0; i < count_test_block; ++i) {
            int err_count = 0;
            // randomly generate the threshold, limits
            uint8_t tmp = get_outer_thresh(&rnd);
            DECLARE_ALIGNED(16, uint8_t, blimit[16]);
            init_buffer_with_value(blimit, 16, tmp);

            DECLARE_ALIGNED(16, uint8_t, limit[16]);
            tmp = get_inner_thresh(&rnd);
            init_buffer_with_value(limit, 16, tmp);

            DECLARE_ALIGNED(16, uint8_t, thresh[16]);
            tmp = get_hev_thresh(&rnd);
            init_buffer_with_value(thresh, 16, tmp);

            // Initial sample data
            init_input(tst_s, ref_s, &rnd, *limit, mask_, p, i);

            // run the filters
            run_lpf(p, blimit, limit, thresh, bit_depth_);

            // check the result
            for (int j = 0; j < kNumCoeffs; ++j)
                err_count += ref_s[j] != tst_s[j];
            if (err_count && !err_count_total)
                first_failure = i;

            err_count_total += err_count;
        }
        EXPECT_EQ(0, err_count_total)
            << "Error: Loop8Test6Param, C output doesn't match SIMD "
               "loopfilter output. "
            << "First failed at test case " << first_failure;
        start_tst_ = nullptr;
        start_ref_ = nullptr;
    }

  protected:
    int bit_depth_;
    int mask_;
    FuncType lpf_tst_;
    FuncType lpf_ref_;
    Sample *start_ref_;
    Sample *start_tst_;
    // loop filter type
    LpfType lpf_type_;
    // Horizontally and Vertically need 32x32:
    // 8  Coeffs preceding filtered section
    // 16 Coefs within filtered section
    // 8  Coeffs following filtered section
    static const int kNumCoeffs = 32 * 32;
};

// class to test loop filter with low bitdepth
class LbdLoopFilterTest
    : public LoopFilterTest<uint8_t, LbdLoopFilterFunc, LdbLpfTestParam> {
  public:
    LbdLoopFilterTest() {
        lpf_type_ = SINGLE;
    }

    virtual ~LbdLoopFilterTest() {
    }

    void run_lpf(LOOP_PARAM, int bd) override {
        (void)bd;
        lpf_tst_(start_tst_, p, blimit, limit, thresh);
        lpf_ref_(start_ref_, p, blimit, limit, thresh);
    }
};

TEST_P(LbdLoopFilterTest, MatchTestRandomData) {
    run_test();
}

// class to test loop filter with high bitdepth
class HbdLoopFilterTest
    : public LoopFilterTest<uint16_t, HbdLoopFilterFunc, HbdLpfTestParam> {
  public:
    HbdLoopFilterTest() {
        lpf_type_ = SINGLE;
    }

    virtual ~HbdLoopFilterTest() {
    }

    void run_lpf(LOOP_PARAM, int bd) override {
        lpf_tst_(start_tst_, p, blimit, limit, thresh, bd);
        lpf_ref_(start_ref_, p, blimit, limit, thresh, bd);
    }
};

TEST_P(HbdLoopFilterTest, MatchTestRandomData) {
    run_test();
}

#ifdef ARCH_X86_64
// target and reference functions in different cases
/* clang-format off */
const HbdLpfTestParam kHbdLoop8Test6[] = {
    make_tuple(&svt_aom_highbd_lpf_horizontal_4_sse2,
               &svt_aom_highbd_lpf_horizontal_4_c, 8),
    make_tuple(&svt_aom_highbd_lpf_horizontal_6_sse2,
               &svt_aom_highbd_lpf_horizontal_6_c, 8),
    make_tuple(&svt_aom_highbd_lpf_horizontal_8_sse2,
               &svt_aom_highbd_lpf_horizontal_8_c, 8),
    make_tuple(&svt_aom_highbd_lpf_horizontal_14_sse2,
               &svt_aom_highbd_lpf_horizontal_14_c, 8),

    make_tuple(&svt_aom_highbd_lpf_vertical_4_sse2,
               &svt_aom_highbd_lpf_vertical_4_c, 8),
    make_tuple(&svt_aom_highbd_lpf_vertical_6_sse2,
               &svt_aom_highbd_lpf_vertical_6_c, 8),
    make_tuple(&svt_aom_highbd_lpf_vertical_8_sse2,
               &svt_aom_highbd_lpf_vertical_8_c, 8),
    make_tuple(&svt_aom_highbd_lpf_vertical_14_sse2,
               &svt_aom_highbd_lpf_vertical_14_c, 8),

    make_tuple(&svt_aom_highbd_lpf_horizontal_4_sse2,
               &svt_aom_highbd_lpf_horizontal_4_c, 10),
    make_tuple(&svt_aom_highbd_lpf_horizontal_6_sse2,
               &svt_aom_highbd_lpf_horizontal_6_c, 10),
    make_tuple(&svt_aom_highbd_lpf_horizontal_8_sse2,
               &svt_aom_highbd_lpf_horizontal_8_c, 10),
    make_tuple(&svt_aom_highbd_lpf_horizontal_14_sse2,
               &svt_aom_highbd_lpf_horizontal_14_c, 10),

    make_tuple(&svt_aom_highbd_lpf_vertical_4_sse2,
               &svt_aom_highbd_lpf_vertical_4_c, 10),
    make_tuple(&svt_aom_highbd_lpf_vertical_6_sse2,
               &svt_aom_highbd_lpf_vertical_6_c, 10),
    make_tuple(&svt_aom_highbd_lpf_vertical_8_sse2,
               &svt_aom_highbd_lpf_vertical_8_c, 10),
    make_tuple(&svt_aom_highbd_lpf_vertical_14_sse2,
               &svt_aom_highbd_lpf_vertical_14_c, 10),

    make_tuple(&svt_aom_highbd_lpf_horizontal_4_sse2,
               &svt_aom_highbd_lpf_horizontal_4_c, 12),
    make_tuple(&svt_aom_highbd_lpf_horizontal_6_sse2,
               &svt_aom_highbd_lpf_horizontal_6_c, 12),
    make_tuple(&svt_aom_highbd_lpf_horizontal_8_sse2,
               &svt_aom_highbd_lpf_horizontal_8_c, 12),
    make_tuple(&svt_aom_highbd_lpf_horizontal_14_sse2,
               &svt_aom_highbd_lpf_horizontal_14_c, 12),

    make_tuple(&svt_aom_highbd_lpf_vertical_4_sse2,
               &svt_aom_highbd_lpf_vertical_4_c, 12),
    make_tuple(&svt_aom_highbd_lpf_vertical_6_sse2,
               &svt_aom_highbd_lpf_vertical_6_c, 12),
    make_tuple(&svt_aom_highbd_lpf_vertical_8_sse2,
               &svt_aom_highbd_lpf_vertical_8_c, 12),
    make_tuple(&svt_aom_highbd_lpf_vertical_14_sse2,
               &svt_aom_highbd_lpf_vertical_14_c, 12)};

const LdbLpfTestParam kLoop8Test6[] = {
    make_tuple(&svt_aom_lpf_horizontal_4_sse2, &svt_aom_lpf_horizontal_4_c, 8),
    make_tuple(&svt_aom_lpf_vertical_4_sse2, &svt_aom_lpf_vertical_4_c, 8),
    make_tuple(&svt_aom_lpf_horizontal_6_sse2, &svt_aom_lpf_horizontal_6_c, 8),
    make_tuple(&svt_aom_lpf_vertical_6_sse2, &svt_aom_lpf_vertical_6_c, 8),
    make_tuple(&svt_aom_lpf_horizontal_8_sse2, &svt_aom_lpf_horizontal_8_c, 8),
    make_tuple(&svt_aom_lpf_vertical_8_sse2, &svt_aom_lpf_vertical_8_c, 8),
    make_tuple(&svt_aom_lpf_horizontal_14_sse2, &svt_aom_lpf_horizontal_14_c, 8),
    make_tuple(&svt_aom_lpf_vertical_14_sse2, &svt_aom_lpf_vertical_14_c, 8),
};
/* clang-format on */

INSTANTIATE_TEST_SUITE_P(SSE2, LbdLoopFilterTest,
                         ::testing::ValuesIn(kLoop8Test6));
INSTANTIATE_TEST_SUITE_P(SSE2, HbdLoopFilterTest,
                         ::testing::ValuesIn(kHbdLoop8Test6));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
const LdbLpfTestParam kLoop8Test6[] = {
    make_tuple(&svt_aom_lpf_horizontal_4_neon, &svt_aom_lpf_horizontal_4_c, 8),
    make_tuple(&svt_aom_lpf_vertical_4_neon, &svt_aom_lpf_vertical_4_c, 8),
    make_tuple(&svt_aom_lpf_horizontal_6_neon, &svt_aom_lpf_horizontal_6_c, 8),
    make_tuple(&svt_aom_lpf_vertical_6_neon, &svt_aom_lpf_vertical_6_c, 8),
    make_tuple(&svt_aom_lpf_horizontal_8_neon, &svt_aom_lpf_horizontal_8_c, 8),
    make_tuple(&svt_aom_lpf_vertical_8_neon, &svt_aom_lpf_vertical_8_c, 8),
    make_tuple(&svt_aom_lpf_horizontal_14_neon, &svt_aom_lpf_horizontal_14_c,
               8),
    make_tuple(&svt_aom_lpf_vertical_14_neon, &svt_aom_lpf_vertical_14_c, 8),
};

INSTANTIATE_TEST_SUITE_P(NEON, LbdLoopFilterTest,
                         ::testing::ValuesIn(kLoop8Test6));

const HbdLpfTestParam kHbdLoop8Test6[] = {
    make_tuple(&svt_aom_highbd_lpf_horizontal_4_neon,
               &svt_aom_highbd_lpf_horizontal_4_c, 8),
    make_tuple(&svt_aom_highbd_lpf_horizontal_6_neon,
               &svt_aom_highbd_lpf_horizontal_6_c, 8),
    make_tuple(&svt_aom_highbd_lpf_horizontal_8_neon,
               &svt_aom_highbd_lpf_horizontal_8_c, 8),
    make_tuple(&svt_aom_highbd_lpf_horizontal_14_neon,
               &svt_aom_highbd_lpf_horizontal_14_c, 8),

    make_tuple(&svt_aom_highbd_lpf_vertical_4_neon,
               &svt_aom_highbd_lpf_vertical_4_c, 8),
    make_tuple(&svt_aom_highbd_lpf_vertical_6_neon,
               &svt_aom_highbd_lpf_vertical_6_c, 8),
    make_tuple(&svt_aom_highbd_lpf_vertical_8_neon,
               &svt_aom_highbd_lpf_vertical_8_c, 8),
    make_tuple(&svt_aom_highbd_lpf_vertical_14_neon,
               &svt_aom_highbd_lpf_vertical_14_c, 8),

    make_tuple(&svt_aom_highbd_lpf_horizontal_4_neon,
               &svt_aom_highbd_lpf_horizontal_4_c, 10),
    make_tuple(&svt_aom_highbd_lpf_horizontal_6_neon,
               &svt_aom_highbd_lpf_horizontal_6_c, 10),
    make_tuple(&svt_aom_highbd_lpf_horizontal_8_neon,
               &svt_aom_highbd_lpf_horizontal_8_c, 10),
    make_tuple(&svt_aom_highbd_lpf_horizontal_14_neon,
               &svt_aom_highbd_lpf_horizontal_14_c, 10),

    make_tuple(&svt_aom_highbd_lpf_vertical_4_neon,
               &svt_aom_highbd_lpf_vertical_4_c, 10),
    make_tuple(&svt_aom_highbd_lpf_vertical_6_neon,
               &svt_aom_highbd_lpf_vertical_6_c, 10),
    make_tuple(&svt_aom_highbd_lpf_vertical_8_neon,
               &svt_aom_highbd_lpf_vertical_8_c, 10),
    make_tuple(&svt_aom_highbd_lpf_vertical_14_neon,
               &svt_aom_highbd_lpf_vertical_14_c, 10)};

INSTANTIATE_TEST_SUITE_P(NEON, HbdLoopFilterTest,
                         ::testing::ValuesIn(kHbdLoop8Test6));
#endif  // ARCH_AARCH64
}  // namespace
