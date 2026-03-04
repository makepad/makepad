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

#include "gtest/gtest.h"

#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <algorithm>

#include "definitions.h"
#include "transforms.h"
#include "random.h"
#include "util.h"
#include "aom_dsp_rtcd.h"
#include "transforms.h"
#include "unit_test_utility.h"
#include "TxfmCommon.h"

#ifdef ARCH_X86_64
#include "av1_inv_txfm_ssse3.h"
#endif  // ARCH_X86_64

using svt_av1_test_tool::SVTRandom;  // to generate the random
namespace {

static bool is_tx_type_imp_32x32(const TxType tx_type) {
    switch (tx_type) {
    case DCT_DCT:
    case IDTX: return true;
    default: return false;
    }
}

static bool is_tx_type_imp_64x64(const TxType tx_type) {
    if (tx_type == DCT_DCT)
        return true;
    return false;
}

template <typename FuncType>
class InvTxfm2dAsmTestBase : public ::testing::Test {
  public:
    ~InvTxfm2dAsmTestBase() {
        delete u_bd_rnd_;
        delete s_bd_rnd_;
    }

    void SetUp() override {
        pixel_input_ = reinterpret_cast<int16_t *>(
            svt_aom_memalign(64, MAX_TX_SQUARE * sizeof(int16_t)));
        input_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(64, MAX_TX_SQUARE * sizeof(int32_t)));
        output_test_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(64, MAX_TX_SQUARE * sizeof(uint16_t)));
        output_ref_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(64, MAX_TX_SQUARE * sizeof(uint16_t)));
    }

    void TearDown() override {
        svt_aom_free(pixel_input_);
        svt_aom_free(input_);
        svt_aom_free(output_test_);
        svt_aom_free(output_ref_);
    }

  protected:
    // clear the coeffs according to eob position, note the coeffs are
    // linear.
    void clear_high_freq_coeffs(const TxSize tx_size, const TxType tx_type,
                                const int eob, const int max_eob) {
        const ScanOrder *scan_order = get_scan_order(tx_size, tx_type);
        const int16_t *scan = scan_order->scan;

        for (int i = eob; i < max_eob; ++i) {
            input_[scan[i]] = 0;
        }
    }

    // fill the pixel_input with random data and do forward transform,
    // Note that the forward transform do not re-pack the coefficients,
    // so we have to re-pack the coefficients after transform for
    // some tx_size;
    void populate_with_random(const int width, const int height,
                              const TxType tx_type, const TxSize tx_size) {
        using FwdTxfm2dFunc = void (*)(int16_t *input,
                                       int32_t *output,
                                       uint32_t stride,
                                       TxType tx_type,
                                       uint8_t bd);

        const FwdTxfm2dFunc fwd_txfm_func[TX_SIZES_ALL] = {
            svt_av1_transform_two_d_4x4_c,   svt_av1_transform_two_d_8x8_c,
            svt_av1_transform_two_d_16x16_c, svt_av1_transform_two_d_32x32_c,
            svt_av1_transform_two_d_64x64_c, svt_av1_fwd_txfm2d_4x8_c,
            svt_av1_fwd_txfm2d_8x4_c,        svt_av1_fwd_txfm2d_8x16_c,
            svt_av1_fwd_txfm2d_16x8_c,       svt_av1_fwd_txfm2d_16x32_c,
            svt_av1_fwd_txfm2d_32x16_c,      svt_av1_fwd_txfm2d_32x64_c,
            svt_av1_fwd_txfm2d_64x32_c,      svt_av1_fwd_txfm2d_4x16_c,
            svt_av1_fwd_txfm2d_16x4_c,       svt_av1_fwd_txfm2d_8x32_c,
            svt_av1_fwd_txfm2d_32x8_c,       svt_av1_fwd_txfm2d_16x64_c,
            svt_av1_fwd_txfm2d_64x16_c,
        };

        memset(output_ref_, 0, MAX_TX_SQUARE * sizeof(uint16_t));
        memset(output_test_, 0, MAX_TX_SQUARE * sizeof(uint16_t));
        memset(input_, 0, MAX_TX_SQUARE * sizeof(int32_t));
        memset(pixel_input_, 0, MAX_TX_SQUARE * sizeof(int16_t));
        for (int i = 0; i < height; i++) {
            for (int j = 0; j < width; j++) {
                pixel_input_[i * stride_ + j] =
                    static_cast<int16_t>(s_bd_rnd_->random());
                output_ref_[i * stride_ + j] = output_test_[i * stride_ + j] =
                    static_cast<uint16_t>(u_bd_rnd_->random());
            }
        }

        fwd_txfm_func[tx_size](
            pixel_input_, input_, stride_, tx_type, static_cast<uint8_t>(bd_));
        // post-process, re-pack the coeffcients
        switch (tx_size) {
        case TX_64X64: svt_handle_transform64x64_c(input_); break;
        case TX_64X32: svt_handle_transform64x32_c(input_); break;
        case TX_32X64: svt_handle_transform32x64_c(input_); break;
        case TX_64X16: svt_handle_transform64x16_c(input_); break;
        case TX_16X64: svt_handle_transform16x64_c(input_); break;
        default: break;
        }
        return;
    }

    FuncType ref_func_;
    FuncType test_func_;
    IsTxTypeImpFunc check_imp_func_;
    TxSize tx_size_;
    int bd_; /**< input param 8bit or 10bit */
    SVTRandom *u_bd_rnd_;
    SVTRandom *s_bd_rnd_;

    static const int stride_ = MAX_TX_SIZE;
    int16_t *pixel_input_;
    int32_t *input_;
    uint16_t *output_test_;
    uint16_t *output_ref_;
};

using InvSqrTxfm2dFunc = void (*)(const int32_t *input, uint16_t *output_r,
                                  int32_t stride_r, uint16_t *output_w,
                                  int32_t stride_w, TxType tx_type, int32_t bd);
using InvSqrTxfmTestParam = testing::tuple<InvSqrTxfm2dFunc, InvSqrTxfm2dFunc,
                                           IsTxTypeImpFunc, TxSize, int>;

class InvTxfm2dAsmSqrTest
    : public InvTxfm2dAsmTestBase<InvSqrTxfm2dFunc>,
      public ::testing::WithParamInterface<InvSqrTxfmTestParam> {
  public:
    InvTxfm2dAsmSqrTest() : InvTxfm2dAsmTestBase() {
        ref_func_ = TEST_GET_PARAM(0);
        test_func_ = TEST_GET_PARAM(1);
        check_imp_func_ = TEST_GET_PARAM(2);
        tx_size_ = TEST_GET_PARAM(3);
        bd_ = TEST_GET_PARAM(4);
        u_bd_rnd_ = new SVTRandom(0, (1 << bd_) - 1);
        s_bd_rnd_ = new SVTRandom(-(1 << bd_) + 1, (1 << bd_) - 1);
    }

    void run_sqr_txfm_match_test() {
        const int width = tx_size_wide[tx_size_];
        const int height = tx_size_high[tx_size_];

        if (ref_func_ == nullptr || test_func_ == nullptr)
            return;
        for (int tx_type = DCT_DCT; tx_type < TX_TYPES; ++tx_type) {
            TxType type = static_cast<TxType>(tx_type);

            if (is_txfm_allowed(type, tx_size_) == false)
                continue;

            // Some tx_type is not implemented yet, so we will skip this;
            if (check_imp_func_(type) == false)
                continue;

            const int loops = 100;
            for (int k = 0; k < loops; k++) {
                populate_with_random(width, height, type, tx_size_);

                ref_func_(input_,
                          output_ref_,
                          stride_,
                          output_ref_,
                          stride_,
                          type,
                          bd_);
                test_func_(input_,
                           output_test_,
                           stride_,
                           output_test_,
                           stride_,
                           type,
                           bd_);

                ASSERT_EQ(0,
                          memcmp(output_ref_,
                                 output_test_,
                                 height * stride_ * sizeof(output_test_[0])))
                    << "width: " << width << " height: " << height
                    << " loop: " << k << " tx_type: " << tx_type
                    << " tx_size: " << tx_size_;
            }
        }
    }
};

TEST_P(InvTxfm2dAsmSqrTest, sqr_txfm_match_test) {
    run_sqr_txfm_match_test();
}

// clang-format off
#define SQR_FUNC_PAIRS(name, type, tx_size, is_tx_type_imp)                 \
    {reinterpret_cast<InvSqrTxfm2dFunc>(name##_c),                          \
     reinterpret_cast<InvSqrTxfm2dFunc>(name##_##type), is_tx_type_imp,     \
     tx_size, 8},                                                           \
    {reinterpret_cast<InvSqrTxfm2dFunc>(name##_c),                          \
     reinterpret_cast<InvSqrTxfm2dFunc>(name##_##type), is_tx_type_imp,     \
     tx_size, 10}

#define SQR_FUNC_PAIRS_DAV1D(name, type, tx_size, is_tx_type_imp)          \
    {reinterpret_cast<InvSqrTxfm2dFunc>(svt_av1_##name##_c),               \
     reinterpret_cast<InvSqrTxfm2dFunc>(svt_dav1d_##name##_##type),        \
     is_tx_type_imp, tx_size, 8},                                          \
    {reinterpret_cast<InvSqrTxfm2dFunc>(svt_av1_##name##_c),               \
     reinterpret_cast<InvSqrTxfm2dFunc>(svt_dav1d_##name##_##type),        \
     is_tx_type_imp, tx_size, 10}
// clang-format on

#ifdef ARCH_X86_64

static const InvSqrTxfmTestParam sqr_inv_txfm_c_sse4_1_func_pairs[10] = {
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_4x4, sse4_1, TX_4X4,
                   dct_adst_combine_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_8x8, sse4_1, TX_8X8, all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_16x16, sse4_1, TX_16X16,
                   all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_32x32, sse4_1, TX_32X32,
                   dct_adst_combine_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_64x64, sse4_1, TX_64X64,
                   is_tx_type_imp_64x64),
};

INSTANTIATE_TEST_SUITE_P(SSE4_1, InvTxfm2dAsmSqrTest,
                         ::testing::ValuesIn(sqr_inv_txfm_c_sse4_1_func_pairs));

static const InvSqrTxfmTestParam sqr_inv_txfm_c_avx2_func_pairs[10] = {
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_4x4, avx2, TX_4X4, all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_8x8, avx2, TX_8X8, all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_16x16, avx2, TX_16X16,
                   all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_32x32, avx2, TX_32X32,
                   is_tx_type_imp_32x32),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_64x64, avx2, TX_64X64,
                   is_tx_type_imp_64x64),
};

INSTANTIATE_TEST_SUITE_P(AVX2, InvTxfm2dAsmSqrTest,
                         ::testing::ValuesIn(sqr_inv_txfm_c_avx2_func_pairs));

static const InvSqrTxfmTestParam sqr_dav1d_inv_txfm_c_avx2_func_pairs[10] = {
    SQR_FUNC_PAIRS_DAV1D(inv_txfm2d_add_4x4, avx2, TX_4X4, all_txtype_imp),
    SQR_FUNC_PAIRS_DAV1D(inv_txfm2d_add_8x8, avx2, TX_8X8, all_txtype_imp),
    SQR_FUNC_PAIRS_DAV1D(inv_txfm2d_add_16x16, avx2, TX_16X16, all_txtype_imp),
    SQR_FUNC_PAIRS_DAV1D(inv_txfm2d_add_32x32, avx2, TX_32X32,
                         is_tx_type_imp_32x32),
    SQR_FUNC_PAIRS_DAV1D(inv_txfm2d_add_64x64, avx2, TX_64X64,
                         is_tx_type_imp_64x64),
};

INSTANTIATE_TEST_SUITE_P(
    dav1d_AVX2, InvTxfm2dAsmSqrTest,
    ::testing::ValuesIn(sqr_dav1d_inv_txfm_c_avx2_func_pairs));

#if EN_AVX512_SUPPORT
static const InvSqrTxfmTestParam sqr_inv_txfm_c_avx512_func_pairs[6] = {
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_16x16, avx512, TX_16X16,
                   all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_32x32, avx512, TX_32X32,
                   is_tx_type_imp_32x32),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_64x64, avx512, TX_64X64,
                   is_tx_type_imp_64x64),
};

INSTANTIATE_TEST_SUITE_P(AVX512, InvTxfm2dAsmSqrTest,
                         ::testing::ValuesIn(sqr_inv_txfm_c_avx512_func_pairs));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
static const InvSqrTxfmTestParam inv_txfm_c_neon_func_pairs[10] = {
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_4x4, neon, TX_4X4, all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_8x8, neon, TX_8X8, all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_16x16, neon, TX_16X16,
                   all_txtype_imp),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_32x32, neon, TX_32X32,
                   is_tx_type_imp_32x32),
    SQR_FUNC_PAIRS(svt_av1_inv_txfm2d_add_64x64, neon, TX_64X64,
                   is_tx_type_imp_64x64),
};

INSTANTIATE_TEST_SUITE_P(NEON, InvTxfm2dAsmSqrTest,
                         ::testing::ValuesIn(inv_txfm_c_neon_func_pairs));
#endif  // ARCH_AARCH64

using InvRectTxfm2dType1Func = void (*)(const int32_t *input,
                                        uint16_t *output_r, int32_t stride_r,
                                        uint16_t *output_w, int32_t stride_w,
                                        TxType tx_type, TxSize tx_size,
                                        int32_t eob, int32_t bd);
using InvRectTxfmType1TestParam =
    std::tuple<InvRectTxfm2dType1Func, InvRectTxfm2dType1Func, TxSize, int>;

class InvTxfm2dAsmType1Test
    : public InvTxfm2dAsmTestBase<InvRectTxfm2dType1Func>,
      public ::testing::WithParamInterface<InvRectTxfmType1TestParam> {
  public:
    InvTxfm2dAsmType1Test() {
        ref_func_ = TEST_GET_PARAM(0);
        test_func_ = TEST_GET_PARAM(1);
        tx_size_ = TEST_GET_PARAM(2);
        bd_ = TEST_GET_PARAM(3);
        u_bd_rnd_ = new SVTRandom(0, (1 << bd_) - 1);
        s_bd_rnd_ = new SVTRandom(-(1 << bd_) + 1, (1 << bd_) - 1);
    }

    void run_rect_type1_txfm_match_test() {
        const int width = tx_size_wide[tx_size_];
        const int height = tx_size_high[tx_size_];
        const int max_eob = av1_get_max_eob(tx_size_);

        for (int tx_type = DCT_DCT; tx_type < TX_TYPES; ++tx_type) {
            TxType type = static_cast<TxType>(tx_type);

            if (is_txfm_allowed(type, tx_size_) == false)
                continue;

            const int loops = 10 * max_eob;
            SVTRandom eob_rnd(1, max_eob - 1);
            for (int k = 0; k < loops; k++) {
                int eob = k < max_eob - 1 ? k + 1 : eob_rnd.random();
                // prepare data by forward transform and then
                // clear the values between eob and max_eob
                populate_with_random(width, height, type, tx_size_);
                clear_high_freq_coeffs(tx_size_, type, eob, max_eob);

                ref_func_(input_,
                          output_ref_,
                          stride_,
                          output_ref_,
                          stride_,
                          type,
                          tx_size_,
                          eob,
                          bd_);
                test_func_(input_,
                           output_test_,
                           stride_,
                           output_test_,
                           stride_,
                           type,
                           tx_size_,
                           eob,
                           bd_);

                ASSERT_EQ(0,
                          memcmp(output_ref_,
                                 output_test_,
                                 height * stride_ * sizeof(output_test_[0])))
                    << "loop: " << k << " tx_type: " << tx_type
                    << " tx_size: " << (int32_t)tx_size_ << " eob: " << eob;
            }
        }
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(InvTxfm2dAsmType1Test);

TEST_P(InvTxfm2dAsmType1Test, rect_type1_txfm_match_test) {
    run_rect_type1_txfm_match_test();
}

#ifdef ARCH_X86_64
static const InvRectTxfmType1TestParam rect_type1_ref_funcs_sse4_1[20] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_8x16_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_8X16, 8 },
    { svt_av1_inv_txfm2d_add_8x16_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_8X16, 10 },
    { svt_av1_inv_txfm2d_add_8x32_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_8X32, 8 },
    { svt_av1_inv_txfm2d_add_8x32_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_8X32, 10 },
    { svt_av1_inv_txfm2d_add_16x8_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_16X8, 8 },
    { svt_av1_inv_txfm2d_add_16x8_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_16X8, 10 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_16X32, 8 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_16X32, 10 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_16X64, 8 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_16X64, 10 },
    { svt_av1_inv_txfm2d_add_32x8_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_32X8, 8 },
    { svt_av1_inv_txfm2d_add_32x8_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_32X8, 10 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_32X16, 8 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_32X16, 10 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_32X64, 8 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_32X64, 10 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_64X16, 8 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_64X16, 10 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_64X32, 8 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_av1_highbd_inv_txfm_add_sse4_1,
      TX_64X32, 10 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(SSE4_1, InvTxfm2dAsmType1Test,
                         ::testing::ValuesIn(rect_type1_ref_funcs_sse4_1));

static const InvRectTxfmType1TestParam rect_type1_ref_funcs_avx2[20] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_8x16_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_8X16, 8 },
    { svt_av1_inv_txfm2d_add_8x16_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_8X16, 10 },
    { svt_av1_inv_txfm2d_add_8x32_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_8X32, 8 },
    { svt_av1_inv_txfm2d_add_8x32_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_8X32, 10 },
    { svt_av1_inv_txfm2d_add_16x8_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_16X8, 8 },
    { svt_av1_inv_txfm2d_add_16x8_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_16X8, 10 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_16X32, 8 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_16X32, 10 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_16X64, 8 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_16X64, 10 },
    { svt_av1_inv_txfm2d_add_32x8_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_32X8, 8 },
    { svt_av1_inv_txfm2d_add_32x8_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_32X8, 10 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_32X16, 8 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_32X16, 10 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_32X64, 8 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_32X64, 10 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_64X16, 8 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_64X16, 10 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_64X32, 8 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_dav1d_highbd_inv_txfm_add_avx2,
      TX_64X32, 10 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(AVX2, InvTxfm2dAsmType1Test,
                         ::testing::ValuesIn(rect_type1_ref_funcs_avx2));

#if EN_AVX512_SUPPORT
static const InvRectTxfmType1TestParam rect_type1_ref_funcs_avx512[12] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_16x32_c, svt_av1_inv_txfm2d_add_16x32_avx512,
      TX_16X32, 8 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_av1_inv_txfm2d_add_16x32_avx512,
      TX_16X32, 10 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_av1_inv_txfm2d_add_16x64_avx512,
      TX_16X64, 8 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_av1_inv_txfm2d_add_16x64_avx512,
      TX_16X64, 10 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_av1_inv_txfm2d_add_32x16_avx512,
      TX_32X16, 8 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_av1_inv_txfm2d_add_32x16_avx512,
      TX_32X16, 10 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_av1_inv_txfm2d_add_32x64_avx512,
      TX_32X64, 8 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_av1_inv_txfm2d_add_32x64_avx512,
      TX_32X64, 10 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_av1_inv_txfm2d_add_64x16_avx512,
      TX_64X16, 8 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_av1_inv_txfm2d_add_64x16_avx512,
      TX_64X16, 10 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_av1_inv_txfm2d_add_64x32_avx512,
      TX_64X32, 8 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_av1_inv_txfm2d_add_64x32_avx512,
      TX_64X32, 10 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(AVX512, InvTxfm2dAsmType1Test,
                         ::testing::ValuesIn(rect_type1_ref_funcs_avx512));
#endif

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
static const InvRectTxfmType1TestParam rect_type1_ref_funcs_neon[20] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_8x16_c, svt_av1_inv_txfm2d_add_8x16_neon,
      TX_8X16, 8 },
    { svt_av1_inv_txfm2d_add_8x16_c, svt_av1_inv_txfm2d_add_8x16_neon,
      TX_8X16, 10 },
    { svt_av1_inv_txfm2d_add_8x32_c, svt_av1_inv_txfm2d_add_8x32_neon,
      TX_8X32, 8 },
    { svt_av1_inv_txfm2d_add_8x32_c, svt_av1_inv_txfm2d_add_8x32_neon,
      TX_8X32, 10 },
    { svt_av1_inv_txfm2d_add_16x8_c, svt_av1_inv_txfm2d_add_16x8_neon,
      TX_16X8, 8 },
    { svt_av1_inv_txfm2d_add_16x8_c, svt_av1_inv_txfm2d_add_16x8_neon,
      TX_16X8, 10 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_av1_inv_txfm2d_add_16x32_neon,
      TX_16X32, 8 },
    { svt_av1_inv_txfm2d_add_16x32_c, svt_av1_inv_txfm2d_add_16x32_neon,
      TX_16X32, 10 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_av1_inv_txfm2d_add_16x64_neon,
      TX_16X64, 8 },
    { svt_av1_inv_txfm2d_add_16x64_c, svt_av1_inv_txfm2d_add_16x64_neon,
      TX_16X64, 10 },
    { svt_av1_inv_txfm2d_add_32x8_c, svt_av1_inv_txfm2d_add_32x8_neon,
      TX_32X8, 8 },
    { svt_av1_inv_txfm2d_add_32x8_c, svt_av1_inv_txfm2d_add_32x8_neon,
      TX_32X8, 10 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_av1_inv_txfm2d_add_32x16_neon,
      TX_32X16, 8 },
    { svt_av1_inv_txfm2d_add_32x16_c, svt_av1_inv_txfm2d_add_32x16_neon,
      TX_32X16, 10 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_av1_inv_txfm2d_add_32x64_neon,
      TX_32X64, 8 },
    { svt_av1_inv_txfm2d_add_32x64_c, svt_av1_inv_txfm2d_add_32x64_neon,
      TX_32X64, 10 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_av1_inv_txfm2d_add_64x16_neon,
      TX_64X16, 8 },
    { svt_av1_inv_txfm2d_add_64x16_c, svt_av1_inv_txfm2d_add_64x16_neon,
      TX_64X16, 10 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_av1_inv_txfm2d_add_64x32_neon,
      TX_64X32, 8 },
    { svt_av1_inv_txfm2d_add_64x32_c, svt_av1_inv_txfm2d_add_64x32_neon,
      TX_64X32, 10 },
};

INSTANTIATE_TEST_SUITE_P(NEON, InvTxfm2dAsmType1Test,
                         ::testing::ValuesIn(rect_type1_ref_funcs_neon));
#endif // ARCH_AARCH64

using InvRectTxfm2dType2Func = void (*)(const int32_t *input,
                                        uint16_t *output_r, int32_t stride_r,
                                        uint16_t *output_w, int32_t stride_w,
                                        TxType tx_type, TxSize tx_size,
                                        int32_t bd);

using InvRectTxfmType2TestParam =
    std::tuple<InvRectTxfm2dType2Func, InvRectTxfm2dType2Func, TxSize, int>;

class InvTxfm2dAsmType2Test
    : public InvTxfm2dAsmTestBase<InvRectTxfm2dType2Func>,
      public ::testing::WithParamInterface<InvRectTxfmType2TestParam> {
  public:
    InvTxfm2dAsmType2Test() {
        ref_func_ = TEST_GET_PARAM(0);
        test_func_ = TEST_GET_PARAM(1);
        tx_size_ = TEST_GET_PARAM(2);
        bd_ = TEST_GET_PARAM(3);
        u_bd_rnd_ = new SVTRandom(0, (1 << bd_) - 1);
        s_bd_rnd_ = new SVTRandom(-(1 << bd_) + 1, (1 << bd_) - 1);
    }

    void run_rect_type2_txfm_match_test() {
        const int width = tx_size_wide[tx_size_];
        const int height = tx_size_high[tx_size_];

        for (int tx_type = DCT_DCT; tx_type < TX_TYPES; ++tx_type) {
            TxType type = static_cast<TxType>(tx_type);

            if (is_txfm_allowed(type, tx_size_) == false)
                continue;

            const int loops = 100;
            for (int k = 0; k < loops; k++) {
                populate_with_random(width, height, type, tx_size_);

                ref_func_(input_,
                          output_ref_,
                          stride_,
                          output_ref_,
                          stride_,
                          type,
                          tx_size_,
                          bd_);
                test_func_(input_,
                           output_test_,
                           stride_,
                           output_test_,
                           stride_,
                           type,
                           tx_size_,
                           bd_);

                ASSERT_EQ(0,
                          memcmp(output_ref_,
                                 output_test_,
                                 height * stride_ * sizeof(output_test_[0])))
                    << "loop: " << k << " tx_type: " << tx_type
                    << " tx_size: " << tx_size_;
            }
        }
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(InvTxfm2dAsmType2Test);

TEST_P(InvTxfm2dAsmType2Test, rect_type2_txfm_match_test) {
    run_rect_type2_txfm_match_test();
}

#ifdef ARCH_X86_64
static const InvRectTxfmType2TestParam rect_type2_ref_funcs_sse4_1[8] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_4x8_c, svt_av1_inv_txfm2d_add_4x8_sse4_1, TX_4X8,
      8 },
    { svt_av1_inv_txfm2d_add_4x8_c, svt_av1_inv_txfm2d_add_4x8_sse4_1, TX_4X8,
      10 },
    { svt_av1_inv_txfm2d_add_8x4_c, svt_av1_inv_txfm2d_add_8x4_sse4_1, TX_8X4,
      8 },
    { svt_av1_inv_txfm2d_add_8x4_c, svt_av1_inv_txfm2d_add_8x4_sse4_1, TX_8X4,
      10 },
    { svt_av1_inv_txfm2d_add_4x16_c, svt_av1_inv_txfm2d_add_4x16_sse4_1, TX_4X16,
      8 },
    { svt_av1_inv_txfm2d_add_4x16_c, svt_av1_inv_txfm2d_add_4x16_sse4_1, TX_4X16,
      10 },
    { svt_av1_inv_txfm2d_add_16x4_c, svt_av1_inv_txfm2d_add_16x4_sse4_1, TX_16X4,
      8 },
    { svt_av1_inv_txfm2d_add_16x4_c, svt_av1_inv_txfm2d_add_16x4_sse4_1, TX_16X4,
      10 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(SSE4_1, InvTxfm2dAsmType2Test,
                         ::testing::ValuesIn(rect_type2_ref_funcs_sse4_1));

static const InvRectTxfmType2TestParam rect_type2_ref_funcs_avx2[8] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_4x8_c, svt_dav1d_inv_txfm2d_add_4x8_avx2, TX_4X8,
      8 },
    { svt_av1_inv_txfm2d_add_4x8_c, svt_dav1d_inv_txfm2d_add_4x8_avx2, TX_4X8,
      10 },
    { svt_av1_inv_txfm2d_add_8x4_c, svt_dav1d_inv_txfm2d_add_8x4_avx2, TX_8X4,
      8 },
    { svt_av1_inv_txfm2d_add_8x4_c, svt_dav1d_inv_txfm2d_add_8x4_avx2, TX_8X4,
      10 },
    { svt_av1_inv_txfm2d_add_4x16_c, svt_dav1d_inv_txfm2d_add_4x16_avx2, TX_4X16,
      8 },
    { svt_av1_inv_txfm2d_add_4x16_c, svt_dav1d_inv_txfm2d_add_4x16_avx2, TX_4X16,
      10 },
    { svt_av1_inv_txfm2d_add_16x4_c, svt_dav1d_inv_txfm2d_add_16x4_avx2, TX_16X4,
      8 },
    { svt_av1_inv_txfm2d_add_16x4_c, svt_dav1d_inv_txfm2d_add_16x4_avx2, TX_16X4,
      10 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(AVX2, InvTxfm2dAsmType2Test,
                         ::testing::ValuesIn(rect_type2_ref_funcs_avx2));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
static const InvRectTxfmType2TestParam rect_type2_ref_funcs_neon[8] = {
    // clang-format off
    { svt_av1_inv_txfm2d_add_4x8_c, svt_av1_inv_txfm2d_add_4x8_neon, TX_4X8,
      8 },
    { svt_av1_inv_txfm2d_add_4x8_c, svt_av1_inv_txfm2d_add_4x8_neon, TX_4X8,
      10 },
    { svt_av1_inv_txfm2d_add_4x16_c, svt_av1_inv_txfm2d_add_4x16_neon, TX_4X16,
      8 },
    { svt_av1_inv_txfm2d_add_4x16_c, svt_av1_inv_txfm2d_add_4x16_neon, TX_4X16,
      10 },
    { svt_av1_inv_txfm2d_add_8x4_c, svt_av1_inv_txfm2d_add_8x4_neon, TX_8X4,
      8 },
    { svt_av1_inv_txfm2d_add_8x4_c, svt_av1_inv_txfm2d_add_8x4_neon, TX_8X4,
      10 },
    { svt_av1_inv_txfm2d_add_16x4_c, svt_av1_inv_txfm2d_add_16x4_neon, TX_16X4,
      8 },
    { svt_av1_inv_txfm2d_add_16x4_c, svt_av1_inv_txfm2d_add_16x4_neon, TX_16X4,
      10 },
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(NEON, InvTxfm2dAsmType2Test,
                         ::testing::ValuesIn(rect_type2_ref_funcs_neon));
#endif  // ARCH_AARCH64

using LowbdInvTxfm2dAddFunc = void (*)(const TranLow *dqcoeff, uint8_t *dst_r,
                                       int32_t stride_r, uint8_t *dst_w,
                                       int32_t stride_w,
                                       const TxfmParam *txfm_param);

using InvTxfm2AddParam = std::tuple<LowbdInvTxfm2dAddFunc, int>;

class InvTxfm2dAddTest
    : public InvTxfm2dAsmTestBase<LowbdInvTxfm2dAddFunc>,
      public ::testing::WithParamInterface<InvTxfm2AddParam> {
  public:
    InvTxfm2dAddTest() {
        // unsigned bd_ bits random
        test_func_ = TEST_GET_PARAM(0);
        bd_ = TEST_GET_PARAM(1);
        u_bd_rnd_ = new SVTRandom(0, (1 << bd_) - 1);
        s_bd_rnd_ = new SVTRandom(-(1 << bd_) + 1, (1 << bd_) - 1);
    }

    void run_svt_av1_inv_txfm_add_test(const TxSize tx_size, int32_t lossless) {
        TxfmParam txfm_param;
        txfm_param.bd = bd_;
        txfm_param.lossless = lossless;
        txfm_param.tx_size = tx_size;
        txfm_param.eob = av1_get_max_eob(tx_size);

        if (bd_ > 8 && !lossless) {
            // Not support 10 bit with not lossless
            return;
        }

        const int txfm_support_matrix[19][16] = {
            //[Size][type]" // O - No; 1 - lossless; 2 - !lossless; 3 - any
            /*0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15*/
            {3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2},  // 0  TX_4X4,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 1  TX_8X8,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 2  TX_16X16,
            {3, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0},  // 3  TX_32X32,
            {3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},  // 4  TX_64X64,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 5  TX_4X8,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 6  TX_8X4,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 7  TX_8X16,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 8  TX_16X8,
            {3, 1, 3, 1, 1, 3, 1, 1, 1, 3, 3, 3, 1, 3, 1, 3},  // 9  TX_16X32,
            {3, 3, 1, 1, 3, 1, 1, 1, 1, 3, 3, 3, 3, 1, 3, 1},  // 10 TX_32X16,
            {3, 0, 1, 0, 0, 1, 0, 0, 0, 3, 3, 3, 0, 1, 0, 1},  // 11 TX_32X64,
            {3, 1, 0, 0, 1, 0, 0, 0, 0, 3, 3, 3, 1, 0, 1, 0},  // 12 TX_64X32,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 13 TX_4X16,
            {3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},  // 14 TX_16X4,
            {3, 1, 3, 1, 1, 3, 1, 1, 1, 3, 3, 3, 1, 3, 1, 3},  // 15 TX_8X32,
            {3, 3, 1, 1, 3, 1, 1, 1, 1, 3, 3, 3, 3, 1, 3, 1},  // 16 TX_32X8,
            {3, 0, 3, 0, 0, 3, 0, 0, 0, 3, 3, 3, 0, 3, 0, 3},  // 17 TX_16X64,
            {3, 3, 0, 0, 3, 0, 0, 0, 0, 3, 3, 3, 3, 0, 3, 0}   // 18 TX_64X16,
            /*0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15*/
        };

        const int width = tx_size_wide[tx_size];
        const int height = tx_size_high[tx_size];

        for (int tx_type = DCT_DCT; tx_type < TX_TYPES; ++tx_type) {
            TxType type = static_cast<TxType>(tx_type);
            txfm_param.tx_type = type;

            if ((lossless && ((txfm_support_matrix[tx_size][type] & 1) == 0)) ||
                (!lossless && ((txfm_support_matrix[tx_size][type] & 2) == 0)))
                continue;

            const int loops = 10;
            for (int k = 0; k < loops; k++) {
                populate_with_random(width, height, type, tx_size);

                svt_av1_inv_txfm_add_c(input_,
                                       (uint8_t *)output_ref_,
                                       stride_,
                                       (uint8_t *)output_ref_,
                                       stride_,
                                       &txfm_param);
                test_func_(input_,
                           (uint8_t *)output_test_,
                           stride_,
                           (uint8_t *)output_test_,
                           stride_,
                           &txfm_param);

                ASSERT_EQ(0,
                          memcmp(output_ref_,
                                 output_test_,
                                 height * stride_ * sizeof(output_test_[0])))
                    << "loop: " << k << " tx_type: " << (int)tx_type
                    << " tx_size: " << (int)tx_size;
            }
        }
    }
};
GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(InvTxfm2dAddTest);

TEST_P(InvTxfm2dAddTest, svt_av1_inv_txfm_add) {
    // Reset all pointers to C
    svt_aom_setup_common_rtcd_internal(0);

    for (int i = TX_4X4; i < TX_SIZES_ALL; i++) {
        const TxSize tx_size = static_cast<TxSize>(i);
        run_svt_av1_inv_txfm_add_test(tx_size, 0);
        run_svt_av1_inv_txfm_add_test(tx_size, 1);
    }
}

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSSE3, InvTxfm2dAddTest,
    ::testing::Combine(::testing::Values(svt_av1_inv_txfm_add_ssse3),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT))));

INSTANTIATE_TEST_SUITE_P(
    AVX2, InvTxfm2dAddTest,
    ::testing::Combine(::testing::Values(svt_av1_inv_txfm_add_avx2),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT))));

INSTANTIATE_TEST_SUITE_P(
    dav1d_AVX2, InvTxfm2dAddTest,
    ::testing::Combine(::testing::Values(svt_dav1d_inv_txfm_add_avx2),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT))));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    dav1d_NEON, InvTxfm2dAddTest,
    ::testing::Combine(::testing::Values(svt_dav1d_inv_txfm_add_neon),
                       ::testing::Values(static_cast<int>(EB_EIGHT_BIT),
                                         static_cast<int>(EB_TEN_BIT))));
#endif  // ARCH_AARCH64

using HandleTransformFunc = uint64_t (*)(int32_t *output);

using HandleTransformParam =
    std::tuple<HandleTransformFunc, HandleTransformFunc, TxSize>;

class HandleTransformTest
    : public ::testing::TestWithParam<HandleTransformParam> {
  public:
    HandleTransformTest()
        : ref_func_(TEST_GET_PARAM(0)),
          test_func_(TEST_GET_PARAM(1)),
          tx_size_(TEST_GET_PARAM(2)) {
    }

    void SetUp() override {
        input_ref_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(64, MAX_TX_SQUARE * sizeof(int32_t)));
        input_test_ = reinterpret_cast<int32_t *>(
            svt_aom_memalign(64, MAX_TX_SQUARE * sizeof(int32_t)));
    }

    void TearDown() override {
        svt_aom_free(input_ref_);
        svt_aom_free(input_test_);
    }

    void run_HandleTransform_match_test() {
        svt_buf_random_s32(input_ref_, MAX_TX_SQUARE);
        memcpy(input_test_, input_ref_, MAX_TX_SQUARE * sizeof(int32_t));

        const uint64_t energy_ref = ref_func_(input_ref_);
        const uint64_t energy_asm = test_func_(input_test_);

        ASSERT_EQ(energy_ref, energy_asm);

        for (int i = 0; i < MAX_TX_SIZE; i++) {
            for (int j = 0; j < MAX_TX_SIZE; j++) {
                ASSERT_EQ(input_ref_[i * MAX_TX_SIZE + j],
                          input_test_[i * MAX_TX_SIZE + j])
                    << " tx_size: " << tx_size_ << " " << j << " x " << i;
            }
        }
    }

    void run_handle_transform_speed_test() {
        double time_c, time_o;
        uint64_t start_time_seconds, start_time_useconds;
        uint64_t middle_time_seconds, middle_time_useconds;
        uint64_t finish_time_seconds, finish_time_useconds;

        const uint64_t num_loop = 10000000;
        uint64_t energy_ref, energy_asm;

        svt_buf_random_s32(input_ref_, MAX_TX_SQUARE);
        memcpy(input_test_, input_ref_, MAX_TX_SQUARE * sizeof(int32_t));

        svt_av1_get_time(&start_time_seconds, &start_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            energy_ref = ref_func_(input_ref_);

        svt_av1_get_time(&middle_time_seconds, &middle_time_useconds);

        for (uint64_t i = 0; i < num_loop; i++)
            energy_asm = test_func_(input_test_);

        svt_av1_get_time(&finish_time_seconds, &finish_time_useconds);
        time_c = svt_av1_compute_overall_elapsed_time_ms(start_time_seconds,
                                                         start_time_useconds,
                                                         middle_time_seconds,
                                                         middle_time_useconds);
        time_o = svt_av1_compute_overall_elapsed_time_ms(middle_time_seconds,
                                                         middle_time_useconds,
                                                         finish_time_seconds,
                                                         finish_time_useconds);

        ASSERT_EQ(energy_ref, energy_asm);

        for (int i = 0; i < MAX_TX_SIZE; i++) {
            for (int j = 0; j < MAX_TX_SIZE; j++) {
                ASSERT_EQ(input_ref_[i * MAX_TX_SIZE + j],
                          input_test_[i * MAX_TX_SIZE + j])
                    << " tx_size: " << tx_size_ << " " << j << " x " << i;
            }
        }

        printf("Average Nanoseconds per Function Call\n");
        printf("    HandleTransform%dx%d_c    : %6.2f\n",
               tx_size_wide[tx_size_],
               tx_size_high[tx_size_],
               1000000 * time_c / num_loop);
        printf(
            "    HandleTransform%dx%d : %6.2f   (Comparison: "
            "%5.2fx)\n",
            tx_size_wide[tx_size_],
            tx_size_high[tx_size_],
            1000000 * time_o / num_loop,
            time_c / time_o);
    }

  private:
    HandleTransformFunc ref_func_;
    HandleTransformFunc test_func_;
    TxSize tx_size_;
    int32_t *input_ref_;
    int32_t *input_test_;
};

TEST_P(HandleTransformTest, match_test) {
    run_HandleTransform_match_test();
}

TEST_P(HandleTransformTest, DISABLED_speed_test) {
    run_handle_transform_speed_test();
}

#ifdef ARCH_X86_64
static const HandleTransformParam HandleTransformArrAVX2[10] = {
    // clang-format off
    { svt_handle_transform16x64_c, svt_handle_transform16x64_avx2, TX_16X64 },
    { svt_handle_transform32x64_c, svt_handle_transform32x64_avx2, TX_32X64 },
    { svt_handle_transform64x16_c, svt_handle_transform64x16_avx2, TX_64X16 },
    { svt_handle_transform64x32_c, svt_handle_transform64x32_avx2, TX_64X32 },
    { svt_handle_transform64x64_c, svt_handle_transform64x64_avx2, TX_64X64 },
    { svt_handle_transform16x64_N2_N4_c, svt_handle_transform16x64_N2_N4_avx2,
      TX_16X64 },
    { svt_handle_transform32x64_N2_N4_c, svt_handle_transform32x64_N2_N4_avx2,
      TX_32X64 },
    { svt_handle_transform64x16_N2_N4_c, svt_handle_transform64x16_N2_N4_avx2,
      TX_64X16 },
    { svt_handle_transform64x32_N2_N4_c, svt_handle_transform64x32_N2_N4_avx2,
      TX_64X32 },
    { svt_handle_transform64x64_N2_N4_c, svt_handle_transform64x64_N2_N4_avx2,
      TX_64X64 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(AVX2, HandleTransformTest,
                         ::testing::ValuesIn(HandleTransformArrAVX2));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
static const HandleTransformParam HandleTransformArrNEON[10] = {
    // clang-format off
    { svt_handle_transform16x64_c, svt_handle_transform16x64_neon, TX_16X64 },
    { svt_handle_transform32x64_c, svt_handle_transform32x64_neon, TX_32X64 },
    { svt_handle_transform64x16_c, svt_handle_transform64x16_neon, TX_64X16 },
    { svt_handle_transform64x32_c, svt_handle_transform64x32_neon, TX_64X32 },
    { svt_handle_transform64x64_c, svt_handle_transform64x64_neon, TX_64X64 },
    { svt_handle_transform16x64_N2_N4_c, svt_handle_transform16x64_N2_N4_neon,
      TX_16X64 },
    { svt_handle_transform32x64_N2_N4_c, svt_handle_transform32x64_N2_N4_neon,
      TX_32X64 },
    { svt_handle_transform64x16_N2_N4_c, svt_handle_transform64x16_N2_N4_neon,
      TX_64X16 },
    { svt_handle_transform64x32_N2_N4_c, svt_handle_transform64x32_N2_N4_neon,
      TX_64X32 },
    { svt_handle_transform64x64_N2_N4_c, svt_handle_transform64x64_N2_N4_neon,
      TX_64X64 }
    // clang-format on
};

INSTANTIATE_TEST_SUITE_P(NEON, HandleTransformTest,
                         ::testing::ValuesIn(HandleTransformArrNEON));
#endif  // ARCH_AARCH64

}  // namespace
