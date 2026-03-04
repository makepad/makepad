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
 * @file PackUnPackTest.cc
 *
 * @brief Unit test for Pack UnPack functions:
 * - svt_enc_msb_pack2d_avx2_intrin_al
 * - svt_enc_msb_pack2d_sse2_intrin
 * - svt_compressed_packmsb_avx2_intrin
 * - svt_enc_msb_un_pack2d_sse2_intrin
 * - svt_unpack_and_2bcompress_neon
 * - svt_compressed_packmsb_neon
 * - svt_enc_msb_pack2d_neon
 *
 * @author Cidana-Ivy, Cidana-Wenyao
 *
 ******************************************************************************/
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <limits.h>
#include <new>

#include "definitions.h"
#include "pack_unpack_c.h"
#include "enc_intra_prediction.h"
#include "unit_test_utility.h"
#include "utility.h"
#include "random.h"
#include "util.h"
#include "common_dsp_rtcd.h"
#include "aom_dsp_rtcd.h"

#define MAX_TEST_SIZE 2048

using svt_av1_test_tool::SVTRandom;  // to generate the random

namespace {
const int RANDOM_TIME = 8;
typedef std::tuple<uint32_t, uint32_t> AreaSize;

AreaSize TEST_PACK_SIZES_EXTEND[] = {
    AreaSize(4, 4),      AreaSize(4, 8),      AreaSize(4, 16),
    AreaSize(8, 4),      AreaSize(8, 8),      AreaSize(8, 16),
    AreaSize(16, 4),     AreaSize(16, 8),     AreaSize(16, 16),
    AreaSize(32, 8),     AreaSize(32, 16),    AreaSize(32, 32),
    AreaSize(32, 64),    AreaSize(64, 16),    AreaSize(64, 32),
    AreaSize(64, 64),    AreaSize(128, 8),    AreaSize(128, 16),
    AreaSize(128, 62),   AreaSize(128, 80),   AreaSize(128, 128),
    AreaSize(176, 144),  AreaSize(640, 480),  AreaSize(800, 600),
    AreaSize(1280, 720), AreaSize(1920, 1080)};

// test svt_compressed_packmsb_sse4_1_intrin
// test svt_compressed_packmsb_avx2_intrin
typedef void (*svt_compressed_packmsb_fn)(
    uint8_t *in8_bit_buffer, uint32_t in8_stride, uint8_t *inn_bit_buffer,
    uint32_t inn_stride, uint16_t *out16_bit_buffer, uint32_t out_stride,
    uint32_t width, uint32_t height);

typedef std::tuple<AreaSize, svt_compressed_packmsb_fn>
    svt_compressed_packmsb_param;
class PackMsbTest
    : public ::testing::TestWithParam<svt_compressed_packmsb_param> {
  public:
    PackMsbTest()
        : area_width_(std::get<0>(TEST_GET_PARAM(0))),
          area_height_(std::get<1>(TEST_GET_PARAM(0))) {
        tst_fn = TEST_GET_PARAM(1);
        in8_stride_ = out_stride_ = MAX_TEST_SIZE;
        inn_stride_ = in8_stride_ >> 2;
        test_size_ = MAX_TEST_SIZE * MAX_TEST_SIZE;
        inn_bit_buffer_ = nullptr;
        in_8bit_buffer_ = nullptr;
        out_16bit_buffer1_ = nullptr;
        out_16bit_buffer2_ = nullptr;
    }

    void SetUp() override {
        inn_bit_buffer_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_ >> 2));
        in_8bit_buffer_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        out_16bit_buffer1_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, sizeof(uint16_t) * test_size_));
        out_16bit_buffer2_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, sizeof(uint16_t) * test_size_));
        memset(out_16bit_buffer1_, 0, sizeof(uint16_t) * test_size_);
        memset(out_16bit_buffer2_, 0, sizeof(uint16_t) * test_size_);
    }

    void TearDown() override {
        if (inn_bit_buffer_)
            svt_aom_free(inn_bit_buffer_);
        if (in_8bit_buffer_)
            svt_aom_free(in_8bit_buffer_);
        if (out_16bit_buffer1_)
            svt_aom_free(out_16bit_buffer1_);
        if (out_16bit_buffer2_)
            svt_aom_free(out_16bit_buffer2_);
    }

  protected:
    void check_output(uint32_t width, uint32_t height, uint16_t *out_1,
                      uint16_t *out_2) {
        int fail_count = 0;
        for (uint32_t j = 0; j < height; j++) {
            for (uint32_t k = 0; k < width; k++) {
                if (out_1[k + j * out_stride_] != out_2[k + j * out_stride_])
                    fail_count++;
            }
        }
        EXPECT_EQ(0, fail_count)
            << "compare result error"
            << "in test area for " << fail_count << "times";
    }

    void run_test() {
        for (int i = 0; i < RANDOM_TIME; i++) {
            svt_buf_random_u8(inn_bit_buffer_, test_size_ >> 2);
            svt_buf_random_u8(in_8bit_buffer_, test_size_);
            tst_fn(in_8bit_buffer_,
                   in8_stride_,
                   inn_bit_buffer_,
                   inn_stride_,
                   out_16bit_buffer1_,
                   out_stride_,
                   area_width_,
                   area_height_);

            svt_compressed_packmsb_c(in_8bit_buffer_,
                                     in8_stride_,
                                     inn_bit_buffer_,
                                     inn_stride_,
                                     out_16bit_buffer2_,
                                     out_stride_,
                                     area_width_,
                                     area_height_);

            check_output(area_width_,
                         area_height_,
                         out_16bit_buffer1_,
                         out_16bit_buffer2_);

            EXPECT_FALSE(HasFailure())
                << "svt_compressed_packmsb_avx2_intrin failed at " << i
                << "th test with size (" << area_width_ << "," << area_height_
                << ")";
        }
    }

    uint8_t *inn_bit_buffer_, *in_8bit_buffer_;
    uint32_t in8_stride_, inn_stride_, out_stride_;
    uint16_t *out_16bit_buffer1_, *out_16bit_buffer2_;
    uint32_t area_width_, area_height_;
    uint32_t test_size_;
    svt_compressed_packmsb_fn tst_fn;
};

TEST_P(PackMsbTest, PackMsbTest) {
    run_test();
};

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, PackMsbTest,
    ::testing::Combine(
        ::testing::ValuesIn(TEST_PACK_SIZES_EXTEND),
        ::testing::Values(svt_compressed_packmsb_sse4_1_intrin)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, PackMsbTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PACK_SIZES_EXTEND),
                       ::testing::Values(svt_compressed_packmsb_avx2_intrin)));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, PackMsbTest,
    ::testing::Combine(::testing::ValuesIn(TEST_PACK_SIZES_EXTEND),
                       ::testing::Values(svt_compressed_packmsb_neon)));

#endif  // ARCH_AARCH64

typedef void (*svt_unpack_and_2bcompress_fn)(
    uint16_t *in16b_buffer, uint32_t in16b_stride, uint8_t *out8b_buffer,
    uint32_t out8b_stride, uint8_t *out2b_buffer, uint32_t out2b_stride,
    uint32_t width, uint32_t height);

typedef std::tuple<AreaSize, svt_unpack_and_2bcompress_fn>
    Unpack2bCompressParam;

class Unpack2bCompress
    : public ::testing::TestWithParam<Unpack2bCompressParam> {
  public:
    Unpack2bCompress()
        : area_width_(std::get<0>(TEST_GET_PARAM(0))),
          area_height_(std::get<1>(TEST_GET_PARAM(0))) {
        tst_fn = TEST_GET_PARAM(1);
        out8_stride_ = in_stride_ = MAX_TEST_SIZE;
        out2_stride_ = out8_stride_ >> 2;
        test_size_ = MAX_TEST_SIZE * MAX_TEST_SIZE;
        in_16bit_ = nullptr;
        out_2bit_buffer_ref_ = nullptr;
        out_2bit_buffer_mod_ = nullptr;
        out_8bit_buffer_ref_ = nullptr;
        out_8bit_buffer_mod_ = nullptr;
    }

    void SetUp() override {
        out_2bit_buffer_ref_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_ >> 2));
        out_2bit_buffer_mod_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_ >> 2));
        out_8bit_buffer_ref_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        out_8bit_buffer_mod_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        in_16bit_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, sizeof(uint16_t) * test_size_));

        memset(out_2bit_buffer_ref_, 0, test_size_ >> 2);
        memset(out_2bit_buffer_mod_, 0, test_size_ >> 2);
        memset(out_8bit_buffer_ref_, 0, test_size_);
        memset(out_8bit_buffer_mod_, 0, test_size_);
    }

    void TearDown() override {
        if (out_2bit_buffer_ref_)
            svt_aom_free(out_2bit_buffer_ref_);
        if (out_2bit_buffer_mod_)
            svt_aom_free(out_2bit_buffer_mod_);
        if (out_8bit_buffer_ref_)
            svt_aom_free(out_8bit_buffer_ref_);
        if (out_8bit_buffer_mod_)
            svt_aom_free(out_8bit_buffer_mod_);
        if (in_16bit_)
            svt_aom_free(in_16bit_);
    }

  protected:
    void check_output(uint32_t width, uint32_t height, uint8_t *out_1,
                      uint8_t *out_2, uint32_t out_stride) {
        int fail_count = 0;
        for (uint32_t j = 0; j < height; j++) {
            for (uint32_t k = 0; k < width; k++) {
                if (out_1[k + j * out_stride] != out_2[k + j * out_stride])
                    fail_count++;
            }
        }
        EXPECT_EQ(0, fail_count)
            << "compare result error"
            << "in test area for " << fail_count << "times";
    }

    void run_test() {
        for (int i = 0; i < RANDOM_TIME; i++) {
            svt_buf_random_u16_with_bd(in_16bit_, test_size_, 10);

            tst_fn(in_16bit_,
                   in_stride_,
                   out_8bit_buffer_mod_,
                   out8_stride_,
                   out_2bit_buffer_mod_,
                   out2_stride_,
                   area_width_,
                   area_height_);

            svt_unpack_and_2bcompress_c(in_16bit_,
                                        in_stride_,
                                        out_8bit_buffer_ref_,
                                        out8_stride_,
                                        out_2bit_buffer_ref_,
                                        out2_stride_,
                                        area_width_,
                                        area_height_);

            // 2bit output
            check_output(area_width_ >> 2,
                         area_height_,
                         out_2bit_buffer_mod_,
                         out_2bit_buffer_ref_,
                         out2_stride_);

            // 8bit output
            check_output(area_width_,
                         area_height_,
                         out_8bit_buffer_mod_,
                         out_8bit_buffer_ref_,
                         out8_stride_);

            EXPECT_FALSE(HasFailure())
                << "svt_unpack_and_2bcompress_avx2 failed at " << i
                << "th test with size (" << area_width_ << "," << area_height_
                << ")";
        }
    }

    uint8_t *out_2bit_buffer_ref_, *out_2bit_buffer_mod_, *out_8bit_buffer_ref_,
        *out_8bit_buffer_mod_;
    uint32_t out8_stride_, out2_stride_, in_stride_;
    uint16_t *in_16bit_;
    uint32_t area_width_, area_height_;
    uint32_t test_size_;
    svt_unpack_and_2bcompress_fn tst_fn;
};

TEST_P(Unpack2bCompress, Unpack2bCompress) {
    run_test();
};

AreaSize TEST_PACK_SIZES_EXTEND2[] = {
    AreaSize(4, 4),      AreaSize(4, 8),       AreaSize(4, 16),
    AreaSize(8, 4),      AreaSize(8, 8),       AreaSize(8, 16),
    AreaSize(16, 4),     AreaSize(16, 8),      AreaSize(16, 16),
    AreaSize(32, 8),     AreaSize(32, 16),     AreaSize(32, 32),
    AreaSize(32, 64),    AreaSize(64, 16),     AreaSize(64, 32),
    AreaSize(64, 64),    AreaSize(128, 8),     AreaSize(128, 16),
    AreaSize(128, 62),   AreaSize(128, 80),    AreaSize(128, 128),
    AreaSize(176, 144),  AreaSize(640, 480),   AreaSize(800, 600),
    AreaSize(1280, 720), AreaSize(1920, 1080), AreaSize(32, 1),
    AreaSize(32, 2),     AreaSize(32, 3),      AreaSize(32, 5),
    AreaSize(64, 1),     AreaSize(64, 2),      AreaSize(64, 3),
    AreaSize(64, 5),     AreaSize(65, 3),      AreaSize(66, 5),
    AreaSize(129, 3),    AreaSize(129, 5)};

#ifdef ARCH_X86_64

INSTANTIATE_TEST_SUITE_P(
    SSE4_1, Unpack2bCompress,
    ::testing::Combine(::testing::ValuesIn(TEST_PACK_SIZES_EXTEND2),
                       ::testing::Values(svt_unpack_and_2bcompress_sse4_1)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, Unpack2bCompress,
    ::testing::Combine(::testing::ValuesIn(TEST_PACK_SIZES_EXTEND2),
                       ::testing::Values(svt_unpack_and_2bcompress_avx2)));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64

INSTANTIATE_TEST_SUITE_P(
    NEON, Unpack2bCompress,
    ::testing::Combine(::testing::ValuesIn(TEST_PACK_SIZES_EXTEND2),
                       ::testing::Values(svt_unpack_and_2bcompress_neon)));

#endif  // ARCH_AARCH64

// test svt_enc_msb_pack2d
// There is an implicit assumption that the width should be multiple of 4.
// Also there are special snippet to handle width of {4, 8, 16, 32, 64}, so use
// TEST_COMMON_SIZES to cover all the width;
AreaSize TEST_COMMON_SIZES[] = {
    AreaSize(4, 4),    AreaSize(4, 8),    AreaSize(8, 4),   AreaSize(8, 8),
    AreaSize(16, 16),  AreaSize(4, 16),   AreaSize(16, 4),  AreaSize(16, 8),
    AreaSize(8, 16),   AreaSize(32, 32),  AreaSize(32, 8),  AreaSize(16, 32),
    AreaSize(8, 32),   AreaSize(32, 16),  AreaSize(16, 64), AreaSize(64, 16),
    AreaSize(64, 64),  AreaSize(64, 32),  AreaSize(32, 64), AreaSize(128, 128),
    AreaSize(68, 64),  AreaSize(72, 64),  AreaSize(80, 64), AreaSize(96, 64),
    AreaSize(64, 128), AreaSize(128, 64), AreaSize(48, 64), AreaSize(24, 64)};

using Pack2d16BitFunc = void (*)(uint8_t *in8_bit_buffer, uint32_t in8_stride,
                                 uint8_t *inn_bit_buffer,
                                 uint16_t *out16_bit_buffer,
                                 uint32_t inn_stride, uint32_t out_stride,
                                 uint32_t width, uint32_t height);

using Pack2d16BitParam = std::tuple<AreaSize, Pack2d16BitFunc>;

class Pack2dTest : public ::testing::TestWithParam<Pack2d16BitParam> {
  public:
    Pack2dTest()
        : area_width_(std::get<0>(TEST_GET_PARAM(0))),
          area_height_(std::get<1>(TEST_GET_PARAM(0))),
          test_func_(TEST_GET_PARAM(1)) {
        in_stride_ = out_stride_ = MAX_SB_SIZE;
        test_size_ = MAX_SB_SQUARE;
        in_8bit_buffer_ = nullptr;
        inn_bit_buffer_ = nullptr;
        out_16bit_buffer_ref_ = nullptr;
        out_16bit_buffer_test_ = nullptr;
    }

    void SetUp() override {
        in_8bit_buffer_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        inn_bit_buffer_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));

        out_16bit_buffer_ref_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, sizeof(uint16_t) * test_size_));
        memset(out_16bit_buffer_ref_, 0, sizeof(uint16_t) * test_size_);
        out_16bit_buffer_test_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, sizeof(uint16_t) * test_size_));
        memset(out_16bit_buffer_test_, 0, sizeof(uint16_t) * test_size_);
    }

    void TearDown() override {
        if (in_8bit_buffer_)
            svt_aom_free(in_8bit_buffer_);
        if (inn_bit_buffer_)
            svt_aom_free(inn_bit_buffer_);

        if (out_16bit_buffer_ref_)
            svt_aom_free(out_16bit_buffer_ref_);

        if (out_16bit_buffer_test_)
            svt_aom_free(out_16bit_buffer_test_);
    }

  protected:
    void check_output(uint32_t width, uint32_t height, uint16_t *out_1,
                      uint16_t *out_2) {
        int fail_count = 0;
        for (uint32_t j = 0; j < height; j++) {
            for (uint32_t k = 0; k < width; k++) {
                if (out_1[k + j * out_stride_] != out_2[k + j * out_stride_])
                    fail_count++;
            }
        }
        EXPECT_EQ(0, fail_count)
            << "compare result error"
            << "in test area for " << fail_count << "times";
    }

    void run_2d_test() {
        for (int i = 0; i < RANDOM_TIME; i++) {
            svt_buf_random_u8(in_8bit_buffer_, test_size_);
            svt_buf_random_u8(inn_bit_buffer_, test_size_);

            svt_enc_msb_pack2_d(in_8bit_buffer_,
                                in_stride_,
                                inn_bit_buffer_,
                                out_16bit_buffer_ref_,
                                out_stride_,
                                out_stride_,
                                area_width_,
                                area_height_);

            test_func_(in_8bit_buffer_,
                       in_stride_,
                       inn_bit_buffer_,
                       out_16bit_buffer_test_,
                       out_stride_,
                       out_stride_,
                       area_width_,
                       area_height_);
            check_output(area_width_,
                         area_height_,
                         out_16bit_buffer_test_,
                         out_16bit_buffer_ref_);

            EXPECT_FALSE(HasFailure())
                << "svt_enc_msb_pack2d_{sse2,avx2}_intrin failed at " << i
                << "th test with size (" << area_width_ << "," << area_height_
                << ")";
        }
    }

    uint8_t *in_8bit_buffer_, *inn_bit_buffer_;
    uint32_t in_stride_, out_stride_;

    uint16_t *out_16bit_buffer_ref_;
    uint16_t *out_16bit_buffer_test_;

    uint32_t area_width_, area_height_;
    Pack2d16BitFunc test_func_;
    uint32_t test_size_;
};

TEST_P(Pack2dTest, Pack2dTest) {
    run_2d_test();
};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, Pack2dTest,
    ::testing::Combine(::testing::ValuesIn(TEST_COMMON_SIZES),
                       ::testing::Values(svt_enc_msb_pack2d_sse2_intrin)));
INSTANTIATE_TEST_SUITE_P(
    AVX2, Pack2dTest,
    ::testing::Combine(::testing::ValuesIn(TEST_COMMON_SIZES),
                       ::testing::Values(svt_enc_msb_pack2d_avx2_intrin_al)));

#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, Pack2dTest,
    ::testing::Combine(::testing::ValuesIn(TEST_COMMON_SIZES),
                       ::testing::Values(svt_enc_msb_pack2d_neon)));
#endif  // ARCH_AARCH64

using UnPack2d16BitFunc = void (*)(uint16_t *in16_bit_buffer,
                                   uint32_t in_stride, uint8_t *out8_bit_buffer,
                                   uint8_t *outn_bit_buffer,
                                   uint32_t out8_stride, uint32_t outn_stride,
                                   uint32_t width, uint32_t height);

using UnPack2d16BitParam = std::tuple<AreaSize, UnPack2d16BitFunc>;

// test svt_enc_un_pack8_bit_data_avx2_intrin
// Similar assumption that the width is multiple of 4, using
// TEST_COMMON_SIZES to cover all the special width.
class UnPack2d16BitTest : public ::testing::TestWithParam<UnPack2d16BitParam> {
  public:
    UnPack2d16BitTest()
        : area_width_(std::get<0>(TEST_GET_PARAM(0))),
          area_height_(std::get<1>(TEST_GET_PARAM(0))),
          test_func_(TEST_GET_PARAM(1)) {
        in_stride_ = out_stride_ = MAX_SB_SIZE;
        test_size_ = MAX_SB_SQUARE;
        out_8bit_buffer_ref_ = nullptr;
        out_8bit_buffer_test_ = nullptr;
        out_nbit_buffer_ref_ = nullptr;
        out_nbit_buffer_test_ = nullptr;
        in_16bit_buffer_ = nullptr;
    }

    void SetUp() override {
        out_8bit_buffer_ref_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        out_8bit_buffer_test_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        out_nbit_buffer_ref_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        out_nbit_buffer_test_ =
            reinterpret_cast<uint8_t *>(svt_aom_memalign(32, test_size_));
        in_16bit_buffer_ = reinterpret_cast<uint16_t *>(
            svt_aom_memalign(32, sizeof(uint16_t) * test_size_));
        memset(out_8bit_buffer_ref_, 0, test_size_);
        memset(out_8bit_buffer_test_, 0, test_size_);
        memset(out_nbit_buffer_ref_, 0, test_size_);
        memset(out_nbit_buffer_test_, 0, test_size_);
    }

    void TearDown() override {
        if (in_16bit_buffer_)
            svt_aom_free(in_16bit_buffer_);
        if (out_8bit_buffer_ref_)
            svt_aom_free(out_8bit_buffer_ref_);
        if (out_8bit_buffer_test_)
            svt_aom_free(out_8bit_buffer_test_);
        if (out_nbit_buffer_ref_)
            svt_aom_free(out_nbit_buffer_ref_);
        if (out_nbit_buffer_test_)
            svt_aom_free(out_nbit_buffer_test_);
    }

  protected:
    void check_output(uint32_t width, uint32_t height, uint8_t *out_1,
                      uint8_t *out_2) {
        int fail_count = 0;
        for (uint32_t j = 0; j < height; j++) {
            for (uint32_t k = 0; k < width; k++) {
                if (out_1[k + j * out_stride_] != out_2[k + j * out_stride_])
                    fail_count++;
            }
        }
        EXPECT_EQ(0, fail_count)
            << "compare result error"
            << "in test area for " << fail_count << "times";
    }

    void run_2d_test() {
        for (int i = 0; i < RANDOM_TIME; i++) {
            svt_buf_random_u16(in_16bit_buffer_, test_size_);

            svt_enc_msb_un_pack2_d(in_16bit_buffer_,
                                   in_stride_,
                                   out_8bit_buffer_ref_,
                                   out_nbit_buffer_ref_,
                                   out_stride_,
                                   out_stride_,
                                   area_width_,
                                   area_height_);
            test_func_(in_16bit_buffer_,
                       in_stride_,
                       out_8bit_buffer_test_,
                       out_nbit_buffer_test_,
                       out_stride_,
                       out_stride_,
                       area_width_,
                       area_height_);
            check_output(area_width_,
                         area_height_,
                         out_8bit_buffer_ref_,
                         out_8bit_buffer_test_);
            check_output(area_width_,
                         area_height_,
                         out_nbit_buffer_ref_,
                         out_nbit_buffer_test_);

            EXPECT_FALSE(HasFailure())
                << "svt_enc_msb_un_pack2d_sse2_intrin failed at " << i
                << "th test with size (" << area_width_ << "," << area_height_
                << ")";
        }
    }

    uint16_t *in_16bit_buffer_;
    uint32_t in_stride_, out_stride_;
    uint8_t *out_8bit_buffer_ref_, *out_8bit_buffer_test_;
    uint8_t *out_nbit_buffer_ref_, *out_nbit_buffer_test_;
    uint32_t area_width_, area_height_;
    UnPack2d16BitFunc test_func_;
    uint32_t test_size_;
};

TEST_P(UnPack2d16BitTest, RunTest) {
    run_2d_test();
};

#ifdef ARCH_X86_64
INSTANTIATE_TEST_SUITE_P(
    SSE2, UnPack2d16BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_COMMON_SIZES),
                       ::testing::Values(svt_enc_msb_un_pack2d_sse2_intrin)));

INSTANTIATE_TEST_SUITE_P(
    AVX2, UnPack2d16BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_COMMON_SIZES),
                       ::testing::Values(svt_enc_msb_un_pack2d_avx2_intrin)));
#endif  // ARCH_X86_64

#ifdef ARCH_AARCH64
INSTANTIATE_TEST_SUITE_P(
    NEON, UnPack2d16BitTest,
    ::testing::Combine(::testing::ValuesIn(TEST_COMMON_SIZES),
                       ::testing::Values(svt_enc_msb_un_pack2d_neon)));
#endif  // ARCH_AARCH64

}  // namespace
